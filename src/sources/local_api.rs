//! A small HTTP API on 127.0.0.1 for scripts, CI and other tools:
//!
//! - `GET    /v1/health`              no token needed
//! - `POST   /v1/islands`             an island as JSON; same key updates it
//! - `DELETE /v1/islands/{key}`       remove one
//! - `POST   /v1/aws/alarm`           an EventBridge or SNS CloudWatch alarm payload,
//!   or a Security Hub findings event
//!
//! Every other request needs `Authorization: Bearer <token>` from the config.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use serde_json::json;

use crate::aws;
use crate::config::AlarmRule;
use crate::island::Island;
use crate::source::{Inbound, Sink, Source};

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
    body: Vec<u8>,
}

impl LocalApi {
    pub fn new(port: u16, token: String, rules: Vec<AlarmRule>) -> Self {
        Self { port, token, rules }
    }
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
        body,
    })
}

fn respond(mut stream: &TcpStream, status: u16, body: &serde_json::Value) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        _ => "Error",
    };
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
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

/// Route one request; returns the status and JSON body.
fn handle(
    req: &Request,
    token: &str,
    rules: &[AlarmRule],
    sink: &Sink,
) -> (u16, serde_json::Value) {
    if req.method == "GET" && req.path == "/v1/health" {
        return (
            200,
            json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}),
        );
    }
    if !req.bearer.as_deref().is_some_and(|b| same_token(b, token)) {
        return (401, json!({"error": "missing or wrong bearer token"}));
    }
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
        let listener = match TcpListener::bind(("127.0.0.1", self.port)) {
            Ok(l) => l,
            Err(e) => {
                sink.status(format!("local API on port {}: {e}", self.port));
                return;
            }
        };
        sink.status(format!("local API on http://127.0.0.1:{}", self.port));
        // Islands posted here have no one to call back; log custom actions.
        std::thread::spawn(move || {
            for msg in inbox {
                if let Inbound::Action { key, action } = msg {
                    crate::log::line(&format!("api: action {action} on {key}"));
                }
            }
        });
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let sink = sink.clone();
            let token = self.token.clone();
            let rules = self.rules.clone();
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                match read_request(&stream) {
                    Ok(req) => {
                        let (status, body) = handle(&req, &token, &rules, &sink);
                        respond(&stream, status, &body);
                    }
                    Err(e) => respond(&stream, 400, &json!({"error": e})),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare() {
        assert!(same_token("abc", "abc"));
        assert!(!same_token("abd", "abc"));
        assert!(!same_token("ab", "abc"));
    }

    #[test]
    fn decodes_keys() {
        assert_eq!(percent_decode("deploy%2Fapi%20v2"), "deploy/api v2");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("bad%zz%4"), "bad%zz%4");
    }
}
