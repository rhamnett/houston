//! A small HTTP API on 127.0.0.1 for scripts, CI and agents:
//!
//! - `GET    /v1/health`              no token needed
//! - `POST   /v1/islands`             an island as JSON; same key updates it
//! - `DELETE /v1/islands/{key}`       remove one
//! - `POST   /v1/aws/alarm`           an EventBridge or SNS CloudWatch alarm payload,
//!   or a Security Hub findings event
//! - settings and remote control: see [`rest_command`]
//! - `POST   /mcp`                    the MCP server (`mcp.rs`)
//!
//! Every other request needs `Authorization: Bearer <token>` from the config.
//! Requests a web page makes (an `Origin` that isn't localhost) are refused.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::AlarmRule;
use crate::control::Command;
use crate::island::Island;
use crate::source::{Inbound, Sink, Source};
use crate::{aws, mcp};

const MAX_BODY: usize = 256 * 1024;
const MAX_HEADER_LINES: usize = 64;

pub struct LocalApi {
    port: u16,
    token: String,
    rules: Vec<AlarmRule>,
}

struct Request {
    method: String,
    path: String,
    bearer: Option<String>,
    origin: Option<String>,
    body: Vec<u8>,
}

impl LocalApi {
    pub fn new(port: u16, token: String, rules: Vec<AlarmRule>) -> Self {
        Self { port, token, rules }
    }
}

/// Bind, waiting briefly for a restarted API to let go of the port.
fn bind(port: u16) -> Option<TcpListener> {
    for _ in 0..20 {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
            return Some(l);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn read_request(stream: &TcpStream) -> Result<Request, String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or("empty request")?.to_string();
    let path = parts.next().ok_or("no path")?.to_string();
    let mut length = 0usize;
    let mut bearer = None;
    let mut origin = None;
    for _ in 0..MAX_HEADER_LINES {
        line.clear();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().map_err(|_| "bad content-length")?,
            "authorization" => bearer = value.strip_prefix("Bearer ").map(|t| t.trim().to_string()),
            "origin" => origin = Some(value.to_string()),
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Err(format!("body over {MAX_BODY} bytes"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Request {
        method,
        path,
        bearer,
        origin,
        body,
    })
}

/// Send `body` as JSON, or nothing when it is `None`.
fn respond(mut stream: &TcpStream, status: u16, body: Option<&Value>) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        422 => "Unprocessable Entity",
        _ => "Error",
    };
    let body = body.map(Value::to_string).unwrap_or_default();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Same machine, not a browser tab: no `Origin`, or a localhost one.
fn local_origin(origin: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let host = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// Compare without leaking how many leading bytes matched.
fn same_token(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn body_json(req: &Request) -> Result<Value, String> {
    if req.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(&req.body).map_err(|e| format!("body: {e}"))
}

/// The settings and remote-control routes, as commands (the MCP tools):
///
/// - `GET    /v1/settings`, `PATCH /v1/settings` (a JSON merge patch)
/// - `PUT    /v1/secrets/{name}` `{"value": …}`, `DELETE /v1/secrets/{name}`
/// - `GET    /v1/status`, `GET /v1/aws/profiles`
/// - `PUT    /v1/radio-silence` `{"on": true}`
/// - `POST   /v1/board/clear`, `/v1/board/dismiss` `{"id"}`,
///   `/v1/board/press` `{"id", "action"}`
/// - `POST   /v1/drills/{trigger}`
/// - `POST   /v1/cloudwatch/accounts/{i}/test`, `…/{i}/discover`
/// - `POST   /v1/linear/test`, `/v1/alarm-rules/test` `{"name"}`
/// - `POST   /v1/commands/{tool}`: any MCP tool, the body its arguments
fn rest_command(req: &Request) -> Option<Result<Command, String>> {
    let path = req.path.split('?').next().unwrap_or_default();
    let segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(percent_decode)
        .collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let (name, arguments): (&str, Result<Value, String>) =
        match (req.method.as_str(), segments.as_slice()) {
            ("GET", ["v1", "settings"]) => ("get_settings", Ok(json!({}))),
            ("PATCH", ["v1", "settings"]) => (
                "update_settings",
                body_json(req).map(|patch| json!({ "patch": patch })),
            ),
            ("PUT", ["v1", "secrets", secret]) => (
                "set_secret",
                body_json(req).map(|b| json!({ "name": secret, "value": b["value"] })),
            ),
            ("DELETE", ["v1", "secrets", secret]) => {
                ("set_secret", Ok(json!({ "name": secret, "value": "" })))
            }
            ("GET", ["v1", "status"]) => ("get_status", Ok(json!({}))),
            ("GET", ["v1", "aws", "profiles"]) => ("list_aws_profiles", Ok(json!({}))),
            ("PUT", ["v1", "radio-silence"]) => ("set_radio_silence", body_json(req)),
            ("POST", ["v1", "board", "clear"]) => ("clear_board", Ok(json!({}))),
            ("POST", ["v1", "board", "dismiss"]) => ("dismiss_island", body_json(req)),
            ("POST", ["v1", "board", "press"]) => ("press_button", body_json(req)),
            ("POST", ["v1", "drills", trigger]) => ("run_drill", Ok(json!({ "trigger": trigger }))),
            (
                "POST",
                [
                    "v1",
                    "cloudwatch",
                    "accounts",
                    i,
                    check @ ("test" | "discover"),
                ],
            ) => {
                let tool = if *check == "test" {
                    "test_cloudwatch"
                } else {
                    "discover_alarms"
                };
                (
                    tool,
                    i.parse::<usize>()
                        .map(|i| json!({ "account": i }))
                        .map_err(|_| format!("{i} is not an account index")),
                )
            }
            ("POST", ["v1", "linear", "test"]) => ("test_linear", Ok(json!({}))),
            ("POST", ["v1", "alarm-rules", "test"]) => ("test_alarm_rule", body_json(req)),
            ("POST", ["v1", "commands", tool]) => (tool, body_json(req)),
            _ => return None,
        };
    Some(arguments.and_then(|arguments| {
        serde_json::from_value(json!({ "name": name, "arguments": arguments }))
            .map_err(|e| format!("{name}: {e}"))
    }))
}

/// Carry out a command: islands here, everything else on the main thread.
fn execute(command: Command, sink: &Sink) -> Result<Value, String> {
    match command {
        Command::PostIsland { island } => {
            let island = island.validated()?;
            let key = island.key.clone();
            sink.upsert(island);
            Ok(json!({ "key": key }))
        }
        Command::RemoveIsland { key } => {
            sink.remove(&key);
            Ok(json!({ "key": key }))
        }
        command => sink.request(command),
    }
}

/// Route one request; returns the status and JSON body (none for 202 to MCP
/// notifications).
fn handle(req: &Request, token: &str, rules: &[AlarmRule], sink: &Sink) -> (u16, Option<Value>) {
    if !local_origin(req.origin.as_deref()) {
        return (
            403,
            Some(json!({"error": "requests from web pages are refused"})),
        );
    }
    if req.method == "GET" && req.path == "/v1/health" {
        return (200, Some(json!({"ok": true, "version": crate::VERSION})));
    }
    if !req.bearer.as_deref().is_some_and(|b| same_token(b, token)) {
        return (401, Some(json!({"error": "missing or wrong bearer token"})));
    }
    if req.path == "/mcp" {
        if req.method != "POST" {
            return (
                405,
                Some(json!({"error": "POST JSON-RPC messages; no event stream"})),
            );
        }
        return match mcp::handle(&req.body, |c| execute(c, sink)) {
            Some(answer) => (200, Some(answer)),
            None => (202, None),
        };
    }
    if let Some(command) = rest_command(req) {
        return match command {
            Err(e) => (400, Some(json!({"error": e}))),
            Ok(command) => match execute(command, sink) {
                Ok(v) => (200, Some(v)),
                Err(e) => (422, Some(json!({"error": e}))),
            },
        };
    }
    let (status, body) = handle_islands(req, rules, sink);
    (status, Some(body))
}

fn handle_islands(req: &Request, rules: &[AlarmRule], sink: &Sink) -> (u16, Value) {
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/v1/islands") => {
            let island = serde_json::from_slice::<Island>(&req.body)
                .map_err(|e| e.to_string())
                .and_then(Island::validated);
            match island {
                Ok(island) => {
                    let key = island.key.clone();
                    sink.upsert(island);
                    (202, json!({"ok": true, "key": key}))
                }
                Err(e) => (400, json!({"error": e})),
            }
        }
        ("POST", "/v1/aws/alarm") => {
            let body = String::from_utf8_lossy(&req.body);
            if let Some(alarm) = aws::alarm_from_message(&body) {
                let key = alarm.key();
                let muted = match alarm.island_with(None, rules) {
                    Some(island) => {
                        sink.upsert(island);
                        false
                    }
                    None => true,
                };
                return (202, json!({"ok": true, "key": key, "muted": muted}));
            }
            let findings = aws::findings_from_message(&body);
            if findings.is_empty() {
                return (
                    422,
                    json!({"error": "not a CloudWatch alarm or Security Hub findings payload"}),
                );
            }
            let keys: Vec<String> = findings.iter().map(|i| i.key.clone()).collect();
            findings.into_iter().for_each(|i| sink.upsert(i));
            (202, json!({"ok": true, "keys": keys}))
        }
        ("DELETE", path) if path.starts_with("/v1/islands/") => {
            let key = percent_decode(&path["/v1/islands/".len()..]);
            sink.remove(&key);
            (202, json!({"ok": true, "key": key}))
        }
        _ => (404, json!({"error": "no such endpoint"})),
    }
}

impl Source for LocalApi {
    fn id(&self) -> String {
        "api".into()
    }

    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>) {
        let Some(listener) = bind(self.port) else {
            sink.status(format!("local API: port {} is in use", self.port));
            return;
        };
        let port = self.port;
        sink.status(format!("local API on http://127.0.0.1:{port}"));
        let stop = Arc::new(AtomicBool::new(false));
        let rules = Arc::new(RwLock::new(self.rules.clone()));
        let accept = {
            let stop = stop.clone();
            let rules = rules.clone();
            let sink = sink.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        break; // dropping the listener frees the port
                    }
                    let Ok(stream) = stream else { continue };
                    let sink = sink.clone();
                    let token = self.token.clone();
                    let rules = rules.read().map(|r| r.clone()).unwrap_or_default();
                    std::thread::spawn(move || {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        // Answers can wait on AWS; never on a stalled client.
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                        match read_request(&stream) {
                            Ok(req) => {
                                let (status, body) = handle(&req, &token, &rules, &sink);
                                respond(&stream, status, body.as_ref());
                            }
                            Err(e) => respond(&stream, 400, Some(&json!({"error": e}))),
                        }
                    });
                }
            })
        };
        for msg in inbox {
            match msg {
                // Islands posted here have no one to call back, so a button
                // (Acknowledge, Resolve, ...) answers the island: it goes.
                Inbound::Action { key, action } => {
                    crate::log::line(&format!("api: action {action} on {key}"));
                    sink.remove(&key);
                }
                Inbound::Rules(new) => {
                    if let Ok(mut r) = rules.write() {
                        *r = new;
                    }
                }
                Inbound::Trigger(_) => {}
            }
        }
        // Restarted or quitting: wake the accept loop so it sees `stop`.
        stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", port));
        let _ = accept.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UserEvent;
    use crate::source::{Inbound, SourceEvent};
    use std::sync::mpsc::{self, Sender};

    const TOKEN: &str = "t0ken-for-tests";

    /// A real `LocalApi` on a spare port, with a stand-in for the app: it
    /// answers commands and passes islands to `events`.
    struct Server {
        port: u16,
        events: Receiver<SourceEvent>,
        /// Each command the app was asked to carry out, as `Debug` text.
        commands: Receiver<String>,
        inbox: Option<Sender<Inbound>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Server {
        fn start() -> Self {
            let port = TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let (sink, from_api) = Sink::channel("api");
            let (inbox, inbox_rx) = mpsc::channel();
            let api = Box::new(LocalApi::new(port, TOKEN.into(), Vec::new()));
            let thread = std::thread::spawn(move || api.run(sink, inbox_rx));
            let (events_tx, events) = mpsc::channel();
            let (commands_tx, commands) = mpsc::channel();
            std::thread::spawn(move || {
                for event in from_api {
                    match event {
                        UserEvent::Source { event, .. } => {
                            let _ = events_tx.send(event);
                        }
                        UserEvent::Control { command, reply } => {
                            let _ = commands_tx.send(format!("{command:?}"));
                            let answer = match command {
                                Command::SetRadioSilence { on } => Ok(json!({"radio_silence": on})),
                                Command::UpdateSettings { patch }
                                    if patch.get("fail").is_some() =>
                                {
                                    Err("nope".to_string())
                                }
                                Command::UpdateSettings { patch } => Ok(json!({"patch": patch})),
                                other => Ok(json!({"ran": format!("{other:?}")})),
                            };
                            let _ = reply.send(answer);
                        }
                        _ => {}
                    }
                }
            });
            let server = Self {
                port,
                events,
                commands,
                inbox: Some(inbox),
                thread: Some(thread),
            };
            for _ in 0..50 {
                if server.call("GET", "/v1/health", &[], "").0 == 200 {
                    return server;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("the API never came up on {port}");
        }

        /// One raw HTTP/1.1 request; the status and body.
        fn call(&self, method: &str, path: &str, headers: &[&str], body: &str) -> (u16, String) {
            let Ok(mut stream) = TcpStream::connect(("127.0.0.1", self.port)) else {
                return (0, String::new());
            };
            let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
            for h in headers {
                request.push_str(&format!("{h}\r\n"));
            }
            request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
            stream.write_all(request.as_bytes()).unwrap();
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
            let status = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let body = response
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            (status, body)
        }

        fn authed(&self, method: &str, path: &str, body: &str) -> (u16, Value) {
            let auth = format!("Authorization: Bearer {TOKEN}");
            let (status, body) = self.call(method, path, &[&auth], body);
            (status, serde_json::from_str(&body).unwrap_or(Value::Null))
        }

        fn mcp(&self, msg: Value) -> (u16, Value) {
            self.authed("POST", "/mcp", &msg.to_string())
        }

        fn tool(&self, name: &str, arguments: Value) -> Value {
            let (status, v) = self.mcp(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}}));
            assert_eq!(status, 200, "{v}");
            v["result"].clone()
        }

        fn next_event(&self) -> SourceEvent {
            loop {
                match self
                    .events
                    .recv_timeout(Duration::from_secs(2))
                    .expect("an event")
                {
                    SourceEvent::Status(_) => continue,
                    event => return event,
                }
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.inbox.take(); // the source returns when its inbox closes
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    #[test]
    fn health_needs_no_token_and_the_rest_do() {
        let api = Server::start();
        let (status, body) = api.call("GET", "/v1/health", &[], "");
        assert_eq!(status, 200);
        assert!(body.contains(r#""ok":true"#));
        assert_eq!(api.call("GET", "/v1/status", &[], "").0, 401);
        assert_eq!(
            api.call("GET", "/v1/status", &["Authorization: Bearer wrong"], "")
                .0,
            401
        );
        assert_eq!(api.call("POST", "/mcp", &[], "{}").0, 401);
        assert!(api.commands.try_recv().is_err(), "nothing reached the app");
    }

    #[test]
    fn web_pages_are_refused_even_with_the_token() {
        let api = Server::start();
        let auth = format!("Authorization: Bearer {TOKEN}");
        let evil = ["Origin: https://evil.example", auth.as_str()];
        assert_eq!(api.call("POST", "/mcp", &evil, "{}").0, 403);
        assert_eq!(api.call("GET", "/v1/health", &evil, "").0, 403);
        let local = ["Origin: http://localhost:6274", auth.as_str()];
        assert_eq!(api.call("GET", "/v1/status", &local, "").0, 200);
    }

    #[test]
    fn islands_post_and_remove() {
        let api = Server::start();
        let (status, v) = api.authed("POST", "/v1/islands", r#"{"key":"build","title":"Built"}"#);
        assert_eq!((status, &v["key"]), (202, &json!("build")));
        match api.next_event() {
            SourceEvent::Upsert(island) => {
                assert_eq!(island.title, "Built");
                assert_eq!(island.source, "api", "the API's sink names the source");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            api.authed("POST", "/v1/islands", r#"{"title":"no key"}"#).0,
            400
        );
        assert_eq!(api.authed("DELETE", "/v1/islands/deploy%2Fweb", "").0, 202);
        assert!(matches!(api.next_event(), SourceEvent::Remove(k) if k == "deploy/web"));
    }

    #[test]
    fn rest_settings_go_to_the_app() {
        let api = Server::start();
        let (status, v) = api.authed("PATCH", "/v1/settings", r#"{"sounds":false}"#);
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["patch"], json!({"sounds": false}));
        assert!(api.commands.recv().unwrap().starts_with("UpdateSettings"));

        let (status, v) = api.authed("PATCH", "/v1/settings", r#"{"fail":1}"#);
        assert_eq!((status, v["error"].as_str()), (422, Some("nope")));
        assert_eq!(api.authed("PATCH", "/v1/settings", "{not json").0, 400);
        assert_eq!(
            api.authed("PUT", "/v1/radio-silence", r#"{"on":"yes"}"#).0,
            400
        );
        let (status, v) = api.authed("PUT", "/v1/radio-silence", r#"{"on":true}"#);
        assert_eq!((status, &v["radio_silence"]), (200, &json!(true)));
        let (status, v) = api.authed("POST", "/v1/drills/alarm", "");
        assert_eq!(status, 200);
        assert!(v["ran"].as_str().unwrap().contains("alarm"));
        assert_eq!(api.authed("GET", "/v1/nowhere", "").0, 404);
    }

    #[test]
    fn mcp_over_http() {
        let api = Server::start();
        let (status, v) = api.mcp(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}}}));
        assert_eq!(status, 200);
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");

        let auth = format!("Authorization: Bearer {TOKEN}");
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
        assert_eq!(
            api.call("POST", "/mcp", &[&auth], &note),
            (202, String::new())
        );
        assert_eq!(api.call("GET", "/mcp", &[&auth], "").0, 405);

        let r = api.tool("set_radio_silence", json!({"on": true}));
        assert_eq!(r["structuredContent"]["radio_silence"], true, "{r}");
        assert_eq!(api.commands.recv().unwrap(), "SetRadioSilence { on: true }");

        let r = api.tool("update_settings", json!({"patch": {"fail": 1}}));
        assert_eq!(
            (r["isError"].as_bool(), r["content"][0]["text"].as_str()),
            (Some(true), Some("nope"))
        );

        // Islands are posted by the API itself, without the app's main thread.
        let r = api.tool(
            "post_island",
            json!({"island": {"key": "k", "title": "From MCP"}}),
        );
        assert_eq!(r["structuredContent"]["key"], "k", "{r}");
        assert!(matches!(api.next_event(), SourceEvent::Upsert(i) if i.title == "From MCP"));
        let r = api.tool("post_island", json!({"island": {"key": "", "title": "x"}}));
        assert_eq!(r["isError"], true, "{r}");

        let (_, v) = api.mcp(json!({"jsonrpc": "2.0", "id": 9, "method": "resources/list"}));
        assert_eq!(v["error"]["code"], -32601);
        let (status, body) = api.call("POST", "/mcp", &[&auth], "{oops");
        assert_eq!(status, 200);
        assert!(body.contains("-32700"), "{body}");
    }

    #[test]
    fn oversized_bodies_are_refused() {
        let api = Server::start();
        let auth = format!("Authorization: Bearer {TOKEN}");
        let len = format!("Content-Length: {}", MAX_BODY + 1);
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", api.port)) else {
            panic!("no API");
        };
        write!(stream, "POST /mcp HTTP/1.1\r\n{auth}\r\n{len}\r\n\r\n").unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    }

    #[test]
    fn new_alarm_rules_apply_without_a_restart() {
        let api = Server::start();
        let alarm = r#"{"version":"0","detail-type":"CloudWatch Alarm State Change","source":"aws.cloudwatch","account":"111122223333","region":"us-east-1","detail":{"alarmName":"db cpu","state":{"value":"ALARM","reason":"high","timestamp":"t1"},"previousState":{"value":"OK"}}}"#;
        let (status, v) = api.authed("POST", "/v1/aws/alarm", alarm);
        assert_eq!((status, &v["muted"]), (202, &json!(false)));
        assert!(matches!(api.next_event(), SourceEvent::Upsert(_)));

        let mute: AlarmRule =
            serde_json::from_value(json!({"pattern": "db *", "mute": true})).unwrap();
        api.inbox
            .as_ref()
            .unwrap()
            .send(Inbound::Rules(vec![mute]))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let (_, v) = api.authed("POST", "/v1/aws/alarm", alarm);
            if v["muted"] == true {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "rules never applied");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(api.call("GET", "/v1/health", &[], "").0, 200, "same server");
    }

    #[test]
    fn a_button_press_removes_the_island() {
        let api = Server::start();
        api.inbox
            .as_ref()
            .unwrap()
            .send(Inbound::Action {
                key: "deploy".into(),
                action: "ack".into(),
            })
            .unwrap();
        assert!(matches!(api.next_event(), SourceEvent::Remove(k) if k == "deploy"));
    }

    #[test]
    fn stopping_frees_the_port() {
        let api = Server::start();
        let port = api.port;
        drop(api);
        assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    #[test]
    fn token_compare() {
        assert!(same_token("abc", "abc"));
        assert!(!same_token("abd", "abc"));
        assert!(!same_token("ab", "abc"));
    }

    fn req(method: &str, path: &str, body: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            bearer: None,
            origin: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn routes_to_commands() {
        let c = rest_command(&req("PATCH", "/v1/settings", r#"{"sounds":false}"#));
        assert!(matches!(c, Some(Ok(Command::UpdateSettings { .. }))));
        let c = rest_command(&req("DELETE", "/v1/secrets/aws-secret%3AAKIAX", ""));
        assert!(
            matches!(c, Some(Ok(Command::SetSecret { ref name, ref value })) if name == "aws-secret:AKIAX" && value.is_empty())
        );
        let c = rest_command(&req("POST", "/v1/cloudwatch/accounts/1/discover", ""));
        assert!(matches!(
            c,
            Some(Ok(Command::DiscoverAlarms { account: 1 }))
        ));
        let c = rest_command(&req("POST", "/v1/cloudwatch/accounts/x/test", ""));
        assert!(matches!(c, Some(Err(_))));
        let c = rest_command(&req(
            "POST",
            "/v1/commands/set_radio_silence",
            r#"{"on":true}"#,
        ));
        assert!(matches!(c, Some(Ok(Command::SetRadioSilence { on: true }))));
        assert!(rest_command(&req("POST", "/v1/islands", "{}")).is_none());
    }

    #[test]
    fn refuses_web_pages() {
        assert!(local_origin(None));
        assert!(local_origin(Some("http://localhost:3000")));
        assert!(local_origin(Some("http://127.0.0.1")));
        assert!(!local_origin(Some("https://evil.example")));
        assert!(!local_origin(Some("http://127.0.0.1.evil.example")));
        assert!(!local_origin(Some("null")));
    }

    #[test]
    fn decodes_keys() {
        assert_eq!(percent_decode("deploy%2Fapi%20v2"), "deploy/api v2");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("bad%zz%4"), "bad%zz%4");
    }
}
