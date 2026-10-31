//! End to end: launch a real, sandboxed Houston and fly it over MCP (HTTP
//! and the `houston mcp` stdio bridge) and the REST API.
//!
//! The sandbox has its own data folder (`HOUSTON_DATA_DIR`: config.json and
//! log), its own Keychain service and a spare port, so your settings and
//! secrets are never touched. HOME is left alone: macOS finds the login
//! Keychain through it. It needs a logged-in macOS session (the app puts an overlay and a
//! menu bar icon up while it runs), so it is ignored by default:
//!
//! ```text
//! cargo test --test e2e -- --ignored --nocapture
//! ```

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_houston");

fn spare_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A one-release GitHub "latest release" feed on a spare port, for the
/// update check. Answers every request the same; the URL to point at.
fn fake_release_feed(tag: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/latest", listener.local_addr().unwrap());
    let body = json!({
        "tag_name": tag,
        "html_url": format!("https://github.com/rhamnett/houston/releases/tag/{tag}"),
        "body": "## Highlights\n- Faster than light",
        "assets": [{"name": format!("Houston-{}-macos.zip", tag.trim_start_matches('v')),
            "browser_download_url": "https://example.com/Houston.zip"}],
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

fn security(args: &[&str]) -> bool {
    Command::new("/usr/bin/security")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

struct Sandbox {
    home: PathBuf,
    service: String,
    token: String,
    port: u16,
    agent: ureq::Agent,
    app: Child,
}

impl Sandbox {
    fn launch() -> Self {
        let id = format!("{}-{}", std::process::id(), spare_port());
        let home = std::env::temp_dir().join(format!("houston-e2e-{id}"));
        let dir = home.clone();
        std::fs::create_dir_all(&dir).unwrap();
        let port = spare_port();
        std::fs::write(
            dir.join("config.json"),
            json!({
                "sounds": false,
                "follow_focus": false,
                "demo": { "autoplay": false },
                "api": { "enabled": true, "port": port },
            })
            .to_string(),
        )
        .unwrap();
        // Made by `security` with -A so the app can read it without a prompt.
        let service = format!("local.houston.e2e.{id}");
        let token = format!("e2e-{id}-token");
        assert!(
            security(&[
                "add-generic-password",
                "-s",
                &service,
                "-a",
                "api-token",
                "-w",
                &token,
                "-A",
                "-U",
            ]),
            "couldn't add the sandbox token to the Keychain"
        );
        let app = Command::new(BIN)
            .env("HOUSTON_DATA_DIR", &home)
            .env("AWS_CONFIG_FILE", home.join("no-aws-config"))
            .env("HOUSTON_KEYCHAIN_SERVICE", &service)
            .env("HOUSTON_UPDATE_URL", fake_release_feed("v99.0.0"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("launch houston");
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        let sandbox = Self {
            home,
            service,
            token,
            port,
            agent,
            app,
        };
        sandbox.wait_for_port(port, true);
        sandbox
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn healthy(&self, port: u16) -> bool {
        self.agent
            .get(format!("http://127.0.0.1:{port}/v1/health"))
            .call()
            .is_ok_and(|r| r.status() == 200)
    }

    fn wait_for_port(&self, port: u16, up: bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if self.healthy(port) == up {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "port {port} never went {}\nlog:\n{}",
            if up { "up" } else { "down" },
            self.log()
        );
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.home.join("houston.log")).unwrap_or_default()
    }

    fn config_file(&self) -> String {
        std::fs::read_to_string(self.home.join("config.json")).unwrap()
    }

    fn config(&self) -> Value {
        serde_json::from_str(&self.config_file()).unwrap()
    }

    /// One REST call; the status and JSON body.
    fn rest(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let auth = format!("Bearer {}", self.token);
        let url = self.url(path);
        let response = match (method, body) {
            ("GET", _) => self.agent.get(&url).header("Authorization", &auth).call(),
            ("DELETE", _) => self
                .agent
                .delete(&url)
                .header("Authorization", &auth)
                .call(),
            ("POST", b) => self
                .agent
                .post(&url)
                .header("Authorization", &auth)
                .send(b.unwrap_or(json!({})).to_string()),
            ("PUT", b) => self
                .agent
                .put(&url)
                .header("Authorization", &auth)
                .send(b.unwrap_or(json!({})).to_string()),
            ("PATCH", b) => self
                .agent
                .patch(&url)
                .header("Authorization", &auth)
                .send(b.unwrap_or(json!({})).to_string()),
            _ => panic!("{method}"),
        };
        let mut response = response.expect("request");
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn rpc(&self, msg: Value) -> Value {
        let (status, v) = self.rest("POST", "/mcp", Some(msg));
        assert_eq!(status, 200, "{v}");
        v
    }

    /// Call a tool over MCP; `Ok(structured result)` or `Err(error text)`.
    fn tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        let v = self.rpc(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}}));
        let result = &v["result"];
        assert!(result.is_object(), "{name}: {v}");
        if result["isError"] == true {
            Err(result["content"][0]["text"].as_str().unwrap().to_string())
        } else {
            Ok(result["structuredContent"].clone())
        }
    }

    fn ok(&self, name: &str, arguments: Value) -> Value {
        self.tool(name, arguments)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn err(&self, name: &str, arguments: Value) -> String {
        match self.tool(name, arguments) {
            Ok(v) => panic!("{name} should have failed: {v}"),
            Err(e) => e,
        }
    }

    fn island_ids(&self) -> Vec<String> {
        self.ok("get_status", json!({}))["islands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect()
    }

    /// Islands arrive through the event loop; give it a moment.
    fn wait_for_islands(&self, want: &[&str]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut ids = self.island_ids();
            ids.sort();
            if ids == want {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "board is {ids:?}, wanted {want:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Run `houston mcp` against the sandbox and send it `lines`; its replies.
    fn stdio(&self, lines: &[Value]) -> Vec<Value> {
        let mut bridge = Command::new(BIN)
            .arg("mcp")
            .env("HOUSTON_DATA_DIR", &self.home)
            .env("HOUSTON_KEYCHAIN_SERVICE", &self.service)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("houston mcp");
        let mut stdin = bridge.stdin.take().unwrap();
        for line in lines {
            writeln!(stdin, "{line}").unwrap();
        }
        drop(stdin); // EOF: the bridge exits after answering
        let out = BufReader::new(bridge.stdout.take().unwrap());
        let replies = out
            .lines()
            .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
            .collect();
        assert!(bridge.wait().unwrap().success());
        replies
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.app.kill();
        let _ = self.app.wait();
        for account in ["api-token", "linear-api-key"] {
            security(&[
                "delete-generic-password",
                "-s",
                &self.service,
                "-a",
                account,
            ]);
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
#[ignore = "launches a sandboxed Houston; needs a macOS login session"]
fn full_mission_over_mcp_and_rest() {
    let mut h = Sandbox::launch();

    // Handshake and catalogue, as an MCP client does it.
    let init = h.rpc(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "e2e", "version": "1"}}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "houston");
    assert_eq!(
        h.rest(
            "POST",
            "/mcp",
            Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        )
        .0,
        202
    );
    let tools = h.rpc(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 17);

    // Settings: what the seeded file says, with secrets only as flags.
    let s = h.ok("get_settings", json!({}));
    assert_eq!(s["settings"]["sounds"], false);
    assert_eq!(s["settings"]["api"]["port"], h.port);
    assert_eq!(s["settings"]["api"]["token"], "", "the token is never sent");
    assert_eq!(s["secrets"]["api_token"], true);
    assert_eq!(s["secrets"]["linear_api_key"], false);

    // A patch is saved to disk and leaves the rest alone.
    let r = h.ok(
        "update_settings",
        json!({"patch": {
            "linear": {"teams": ["APO"], "max_priority": 1},
            "alarm_rules": [{"pattern": "prod-*", "severity": "warning", "sticky": true}],
        }}),
    );
    assert!(
        r["restarted"]
            .as_array()
            .unwrap()
            .contains(&json!("linear")),
        "{r}"
    );
    let file = h.config();
    assert_eq!(file["linear"]["teams"], json!(["APO"]));
    assert_eq!(file["linear"]["max_priority"], 1);
    assert_eq!(file["alarm_rules"][0]["pattern"], "prod-*");
    assert_eq!(file["sounds"], false, "untouched");
    assert_eq!(
        h.ok("test_alarm_rule", json!({"name": "prod-api-5xx"}))["index"],
        0
    );
    assert!(h.ok("test_alarm_rule", json!({"name": "dev-api"}))["index"].is_null());

    // Refused patches change nothing.
    let before = h.config_file();
    assert!(
        h.err(
            "update_settings",
            json!({"patch": {"linear": {"api_key": "lin_x"}}})
        )
        .contains("set_secret")
    );
    assert!(
        h.err("update_settings", json!({"patch": {"api": {"port": 80}}}))
            .contains("1024")
    );
    assert!(
        h.err("update_settings", json!({"patch": {"sounds": "loud"}}))
            .contains("Couldn't read")
    );
    assert!(
        h.err("update_settings", json!({"patch": "sounds"}))
            .contains("object")
    );
    assert_eq!(h.config_file(), before);

    // Secrets: into the Keychain, flagged, never echoed or written to the file.
    let secret = "lin_e2e_do_not_leak";
    let r = h.ok(
        "set_secret",
        json!({"name": "linear-api-key", "value": secret}),
    );
    assert_eq!(r["set"], true);
    let s = h.ok("get_settings", json!({}));
    assert_eq!(s["secrets"]["linear_api_key"], true);
    assert!(!s.to_string().contains(secret) && !r.to_string().contains(secret));
    assert!(!h.config_file().contains(secret));
    assert_eq!(
        h.ok("set_secret", json!({"name": "linear-api-key", "value": ""}))["set"],
        false
    );
    assert_eq!(
        h.ok("get_settings", json!({}))["secrets"]["linear_api_key"],
        false
    );
    assert!(
        h.err("set_secret", json!({"name": "api-token", "value": "x"}))
            .contains("regenerated")
    );
    assert!(
        h.err(
            "set_secret",
            json!({"name": "aws-secret:AKIAIOSFODNN7EXAMPLE", "value": "x"})
        )
        .contains("No account")
    );

    // The board: post, press, dismiss, clear.
    let island = |key: &str| {
        json!({"island": {"key": key, "title": format!("e2e {key}"), "silent": true, "sticky": true,
            "actions": [{"id": "ack", "label": "Ack"}]}})
    };
    h.ok("post_island", island("one"));
    h.wait_for_islands(&["api/one"]);
    let status = h.ok("get_status", json!({}));
    assert_eq!(status["islands"][0]["island"]["title"], "e2e one");
    assert_eq!(
        h.ok("press_button", json!({"id": "api/one", "action": "ack"}))["pressed"],
        "ack"
    );
    assert!(
        h.err("press_button", json!({"id": "api/one", "action": "launch"}))
            .contains("no launch button")
    );
    h.ok("dismiss_island", json!({"id": "api/one"}));
    h.wait_for_islands(&[]);
    assert!(
        h.err("dismiss_island", json!({"id": "api/one"}))
            .contains("No island")
    );
    h.ok("post_island", island("two"));
    h.ok("post_island", island("three"));
    h.wait_for_islands(&["api/three", "api/two"]);
    h.ok("remove_island", json!({"key": "two"}));
    h.wait_for_islands(&["api/three"]);
    h.ok("clear_board", json!({}));
    h.wait_for_islands(&[]);
    assert!(
        h.err("post_island", json!({"island": {"key": "", "title": "x"}}))
            .contains("key")
    );

    // Radio silence follows through to the file.
    assert_eq!(
        h.ok("set_radio_silence", json!({"on": true}))["quiet"],
        true
    );
    assert_eq!(h.config()["do_not_disturb"], true);
    assert_eq!(h.ok("get_status", json!({}))["radio_silence"], true);
    h.ok("set_radio_silence", json!({"on": false}));
    assert_eq!(h.config()["do_not_disturb"], false);

    // Checks report bad input as tool errors.
    assert!(
        h.err("run_drill", json!({"trigger": "launch"}))
            .contains("No drill")
    );
    assert!(
        h.err("discover_alarms", json!({"account": 9}))
            .contains("No CloudWatch account 9")
    );
    assert!(
        h.err("test_cloudwatch", json!({"account": 9}))
            .contains("No CloudWatch account 9")
    );
    assert!(
        h.ok("list_aws_profiles", json!({}))["profiles"]
            .as_array()
            .unwrap()
            .is_empty(),
        "AWS_CONFIG_FILE points nowhere"
    );

    // REST: the same settings, the same guards.
    let (status, v) = h.rest("PATCH", "/v1/settings", Some(json!({"sounds": true})));
    assert_eq!(status, 200, "{v}");
    assert_eq!(h.config()["sounds"], true);
    assert_eq!(
        h.rest("PUT", "/v1/secrets/api-token", Some(json!({"value": "x"})))
            .0,
        422
    );
    assert_eq!(h.rest("GET", "/v1/status", None).0, 200);
    let anon = h.agent.get(h.url("/v1/settings")).call().unwrap();
    assert_eq!(anon.status(), 401);
    let page = h
        .agent
        .get(h.url("/v1/settings"))
        .header("Authorization", &format!("Bearer {}", h.token))
        .header("Origin", "https://evil.example")
        .call()
        .unwrap();
    assert_eq!(page.status(), 403);

    // The stdio bridge, as Claude Desktop runs it.
    let replies = h.stdio(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25"}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "get_settings", "arguments": {}}}),
    ]);
    assert_eq!(
        replies.len(),
        2,
        "one reply per request, none for the notification"
    );
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        replies[1]["result"]["structuredContent"]["settings"]["sounds"],
        true
    );

    // Update check: a newer release is offered; Skip remembers it.
    assert_eq!(h.ok("check_for_updates", json!({}))["checking"], true);
    h.wait_for_islands(&["updates/update"]);
    let offer = &h.ok("get_status", json!({}))["islands"][0]["island"];
    assert_eq!(offer["title"], "Houston 99.0.0 is ready for launch");
    assert_eq!(
        offer["actions"][0]["url"],
        "https://example.com/Houston.zip"
    );
    h.ok(
        "press_button",
        json!({"id": "updates/update", "action": "skip"}),
    );
    h.wait_for_islands(&[]);
    assert_eq!(
        std::fs::read_to_string(h.home.join("skipped-update")).unwrap(),
        "v99.0.0"
    );
    // Turning checks off stops the source, and the tool says so.
    h.ok(
        "update_settings",
        json!({"patch": {"updates": {"check": false}}}),
    );
    assert!(h.err("check_for_updates", json!({})).contains("off"));

    // Moving the API: the answer still arrives, then only the new port works.
    let old = h.port;
    let new = spare_port();
    let r = h.ok("update_settings", json!({"patch": {"api": {"port": new}}}));
    assert_eq!(r["restarted"], json!(["api"]));
    h.wait_for_port(new, true);
    h.wait_for_port(old, false);
    h.port = new;
    assert_eq!(
        h.ok("get_settings", json!({}))["settings"]["api"]["port"],
        new
    );
    let replies = h.stdio(&[json!({"jsonrpc": "2.0", "id": 3, "method": "ping"})]);
    assert_eq!(
        replies[0]["result"],
        json!({}),
        "the bridge follows the new port"
    );
}

#[test]
#[ignore = "runs the stdio bridge with no app listening"]
fn bridge_explains_when_houston_is_not_running() {
    let id = format!("{}-nobody", std::process::id());
    let home = std::env::temp_dir().join(format!("houston-e2e-{id}"));
    let dir = home.clone();
    std::fs::create_dir_all(&dir).unwrap();
    let port = spare_port(); // free, so nothing answers
    std::fs::write(
        dir.join("config.json"),
        json!({"api": {"port": port}}).to_string(),
    )
    .unwrap();
    let service = format!("local.houston.e2e.{id}");
    assert!(security(&[
        "add-generic-password",
        "-s",
        &service,
        "-a",
        "api-token",
        "-w",
        "x",
        "-A",
        "-U",
    ]));
    let mut bridge = Command::new(BIN)
        .arg("mcp")
        .env("HOUSTON_DATA_DIR", &home)
        .env("AWS_CONFIG_FILE", home.join("no-aws-config"))
        .env("HOUSTON_KEYCHAIN_SERVICE", &service)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"})
    )
    .unwrap();
    drop(stdin);
    let lines: Vec<String> = BufReader::new(bridge.stdout.take().unwrap())
        .lines()
        .map(Result::unwrap)
        .collect();
    let _ = bridge.wait();
    security(&["delete-generic-password", "-s", &service, "-a", "api-token"]);
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let v: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(v["id"], 4);
    assert_eq!(v["error"]["code"], -32000);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Is it running?")
    );
}
