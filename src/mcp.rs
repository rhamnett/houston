//! An MCP server, so an LLM agent can drive Houston: read and change every
//! setting, store secrets, post islands, run drills and checks. Each tool is a
//! [`Command`].
//!
//! Two transports:
//!
//! - Streamable HTTP at `POST /mcp` on the local API (same bearer token). It
//!   answers with plain JSON and keeps no sessions.
//! - stdio, `houston mcp`, for clients that launch a command: a bridge that
//!   reads the token from the Keychain and relays each line to `/mcp`.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::Config;
use crate::control::Command;

/// Newest first; a client asking for another version gets the newest.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Houston shows notifications (islands) in the MacBook notch, from \
CloudWatch, SQS, Linear and its local API. Call get_settings before update_settings: patches \
are JSON merge patches, so arrays (cloudwatch.accounts, alarm_rules, linear.teams) are replaced \
whole. Saving restarts only the sources whose settings changed. Secrets are write-only: set them \
with set_secret, never in settings. Disabling the API or changing its port also cuts off this \
connection.";

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

fn read_only(mut tool: Value) -> Value {
    tool["annotations"] = json!({ "readOnlyHint": true });
    tool
}

const SETTINGS_SHAPE: &str = "Top-level keys: do_not_disturb (bool, radio silence), \
follow_focus (bool, go quiet during a macOS Focus), sounds (bool), aws_cli (path or null), \
api {enabled, port}, demo {autoplay: drills on/off}, \
cloudwatch {enabled, interval_secs, accounts: [{auth: {type: \"profile\"} | {type: \"access_key\", \
access_key_id}, profile, region, label, alarm_prefix, include: [exact alarm names; empty = all]}]}, \
sqs {enabled, queue_url, auth, profile, region, wait_secs}, \
linear {enabled, interval_secs, assigned_to_me, teams: [team keys], max_priority: 1-4}, \
iot {enabled, endpoint, profile, region, topics} (not implemented yet), \
alarm_rules: [{pattern (glob, first match wins), severity: info|success|warning|critical, \
sticky, silent, mute, runbook_url}].";

pub fn tools() -> Vec<Value> {
    let index = json!({
        "account": { "type": "integer", "minimum": 0,
            "description": "Index into settings.cloudwatch.accounts, from 0." }
    });
    vec![
        read_only(tool(
            "get_settings",
            "Every setting (secrets blanked), which secrets are set, and the config file path.",
            json!({}),
            &[],
        )),
        tool(
            "update_settings",
            &format!(
                "Change settings with a JSON merge patch: send only what changes, null removes \
                 a key (back to its default), arrays replace the whole list. Validated, saved \
                 and applied at once, restarting only the affected sources. {SETTINGS_SHAPE}"
            ),
            json!({ "patch": { "type": "object",
                "description": "e.g. {\"sounds\": false, \"linear\": {\"enabled\": true, \"teams\": [\"APO\"]}}" } }),
            &["patch"],
        ),
        tool(
            "set_secret",
            "Store a secret in the macOS Keychain, or remove it with an empty value. Secrets \
             can't be read back. Names: linear-api-key, or aws-secret:<access key id> for an \
             access key already used by an account (add the account first).",
            json!({
                "name": { "type": "string" },
                "value": { "type": "string", "description": "Empty removes it." },
            }),
            &["name", "value"],
        ),
        read_only(tool(
            "list_aws_profiles",
            "Profiles in ~/.aws/config, with their region and whether they use SSO (SSO \
             sessions expire within hours, so prefer an access key for unattended polling).",
            json!({}),
            &[],
        )),
        read_only(tool(
            "get_status",
            "The islands on the board (with ids and buttons), radio silence and Focus, the \
             running sources and their status lines, and the drill triggers.",
            json!({}),
            &[],
        )),
        tool(
            "set_radio_silence",
            "Radio silence: islands still arrive, without peeking or sounds.",
            json!({ "on": { "type": "boolean" } }),
            &["on"],
        ),
        tool("clear_board", "Dismiss every island.", json!({}), &[]),
        tool(
            "dismiss_island",
            "Dismiss one island by its id from get_status (source/key).",
            json!({ "id": { "type": "string" } }),
            &["id"],
        ),
        tool(
            "press_button",
            "Press a button on an island, as if clicked: e.g. ack, snooze, take (Linear), or a \
             link, which opens in the browser.",
            json!({
                "id": { "type": "string", "description": "The island id from get_status." },
                "action": { "type": "string", "description": "The button's id." },
            }),
            &["id", "action"],
        ),
        tool(
            "run_drill",
            "Play a demo island: tour, alarm, resolve, deploy, metric, message or incident.",
            json!({ "trigger": { "type": "string",
                "enum": crate::sources::DEMO_TRIGGERS.iter().map(|(t, _)| *t).collect::<Vec<_>>() } }),
            &["trigger"],
        ),
        read_only(tool(
            "test_cloudwatch",
            "Poll one saved CloudWatch account once and say what came back.",
            index.clone(),
            &["account"],
        )),
        read_only(tool(
            "discover_alarms",
            "Every alarm in a saved CloudWatch account (matching its prefix), in any state. \
             Put the names to watch in that account's include list.",
            index,
            &["account"],
        )),
        read_only(tool(
            "test_linear",
            "Check the saved Linear key and teams.",
            json!({}),
            &[],
        )),
        read_only(tool(
            "test_alarm_rule",
            "Which alarm rule (if any) an alarm name hits.",
            json!({ "name": { "type": "string" } }),
            &["name"],
        )),
        tool(
            "post_island",
            "Show an island, or update it (same key). Fields: key, title (required), subtitle, \
             body, icon (emoji), severity (info|success|warning|critical), compact, sticky, \
             ttl_secs (default 15), silent, revision, fields: [{label, value}], actions: [{id, \
             label, role?: default|primary|destructive, url?, snooze_mins?}], kind: {type: \
             \"message\", sender?} | {type: \"alert\", from?, to} | {type: \"progress\", value: \
             0-1} | {type: \"metric\", value, unit?, threshold?, series?: [numbers]} | {type: \
             \"timer\", started_at: unix ms}.",
            json!({ "island": { "type": "object" } }),
            &["island"],
        ),
        tool(
            "remove_island",
            "Remove an island this API posted, by its key.",
            json!({ "key": { "type": "string" } }),
            &["key"],
        ),
    ]
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn tool_result(outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(value) => json!({
            "content": [{ "type": "text",
                "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
            "structuredContent": value,
        }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

/// Answer one JSON-RPC message; `None` for a notification. `run` carries out
/// a tool call.
pub fn handle(body: &[u8], run: impl FnOnce(Command) -> Result<Value, String>) -> Option<Value> {
    let Ok(msg) = serde_json::from_slice::<Value>(body) else {
        return Some(rpc_error(&Value::Null, -32700, "Parse error"));
    };
    let Some(id) = msg.get("id").cloned() else {
        return None; // notifications/initialized and friends
    };
    let method = msg["method"].as_str().unwrap_or_default();
    let params = &msg["params"];
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOL_VERSIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&PROTOCOL_VERSIONS[0]);
            rpc_result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "houston", "title": "Houston",
                        "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => rpc_result(&id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            if !tools().iter().any(|t| t["name"] == name) {
                return Some(rpc_error(&id, -32602, &format!("Unknown tool {name}")));
            }
            let arguments = match &params["arguments"] {
                Value::Null => json!({}),
                a => a.clone(),
            };
            let call = json!({ "name": name, "arguments": arguments });
            let outcome = serde_json::from_value::<Command>(call)
                .map_err(|e| format!("Bad arguments for {name}: {e}"))
                .and_then(run);
            rpc_result(&id, tool_result(outcome))
        }
        _ => rpc_error(&id, -32601, &format!("Method not found: {method}")),
    })
}

/// `houston mcp`: relay stdio to the running app's `/mcp`. Returns the exit code.
pub fn stdio_bridge() -> i32 {
    let config = Config::load();
    if !config.api.enabled {
        eprintln!("houston mcp: the local API is off; turn it on in Mission settings");
        return 1;
    }
    let url = format!("http://127.0.0.1:{}/mcp", config.api.port);
    let auth = format!("Bearer {}", config.api.token);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(150)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = agent
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .send(line.as_bytes())
            .map_err(|e| e.to_string())
            .and_then(|mut r| r.body_mut().read_to_string().map_err(|e| e.to_string()));
        let reply = match answer {
            Ok(body) if body.trim().is_empty() => continue,
            Ok(body) => body,
            Err(e) => {
                // Only requests get an answer; a notification just goes missing.
                let id = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|m| m.get("id").cloned());
                let Some(id) = id else { continue };
                rpc_error(
                    &id,
                    -32000,
                    &format!("Houston isn't answering on {url} ({e}). Is it running?"),
                )
                .to_string()
            }
        };
        if writeln!(out, "{}", reply.trim())
            .and_then(|_| out.flush())
            .is_err()
        {
            break;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(msg: Value) -> Option<Value> {
        handle(msg.to_string().as_bytes(), |c| Ok(json!(format!("{c:?}"))))
    }

    #[test]
    fn initializes_and_lists() {
        let r = call(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18"}}))
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "houston");
        let r = call(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "1999-01-01"}}))
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert!(call(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let r = call(json!({"jsonrpc": "2.0", "id": "x", "method": "tools/list"})).unwrap();
        assert_eq!(r["id"], "x");
        assert!(r["result"]["tools"].as_array().unwrap().len() > 10);
    }

    #[test]
    fn calls_tools() {
        let r = call(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "set_radio_silence", "arguments": {"on": true}}}))
        .unwrap();
        assert!(r["result"]["isError"].is_null(), "{r}");
        let r = call(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "set_radio_silence", "arguments": {"on": "yes"}}}))
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        let r = call(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "launch_rocket"}}))
        .unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn tools_are_well_formed() {
        let tools = tools();
        let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), tools.len(), "duplicate tool names");
        for t in &tools {
            let name = t["name"].as_str().unwrap();
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name}"
            );
            assert!(t["description"].as_str().unwrap().len() > 20, "{name}");
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object", "{name}");
            for r in schema["required"].as_array().unwrap() {
                assert!(
                    schema["properties"].get(r.as_str().unwrap()).is_some(),
                    "{name} requires {r} but doesn't describe it"
                );
            }
            let read_only = t["annotations"]["readOnlyHint"] == true;
            let reads = ["get_", "list_", "test_", "discover_"]
                .iter()
                .any(|p| name.starts_with(p));
            assert_eq!(read_only, reads, "{name}: read-only hint");
        }
    }

    #[test]
    fn edge_messages() {
        let r = call(json!({"jsonrpc": "2.0", "id": 5, "method": "ping"})).unwrap();
        assert_eq!(r["result"], json!({}));
        let r = call(json!({"jsonrpc": "2.0", "id": 6, "method": "prompts/list"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(b"not json", |_| unreachable!()).unwrap();
        assert_eq!(r["error"]["code"], -32700);
        // No arguments at all is the same as {}.
        let r = call(json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": {"name": "get_status"}}))
        .unwrap();
        assert!(r["result"]["isError"].is_null(), "{r}");
        assert_eq!(r["result"]["structuredContent"], "GetStatus");
        // A missing required argument is a tool error the agent can read.
        let r = call(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {"name": "dismiss_island", "arguments": {}}}))
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("id")
        );
        // Failures from the app come back as tool errors too.
        let r = handle(
            json!({"jsonrpc": "2.0", "id": 10, "method": "tools/call",
                "params": {"name": "clear_board"}})
            .to_string()
            .as_bytes(),
            |_| Err("Houston is shutting down.".into()),
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(
            r["result"]["content"][0]["text"],
            "Houston is shutting down."
        );
    }

    /// Every advertised tool is a command, with the arguments it says it needs.
    #[test]
    fn every_tool_parses() {
        for t in tools() {
            let mut args = json!({});
            for (key, schema) in t["inputSchema"]["properties"].as_object().unwrap() {
                args[key] = match schema["type"].as_str() {
                    Some("boolean") => json!(true),
                    Some("integer") => json!(0),
                    Some("object") if key == "island" => json!({"key": "k", "title": "t"}),
                    Some("object") => json!({}),
                    _ => json!("tour"),
                };
            }
            let call = json!({"name": t["name"], "arguments": args});
            assert!(
                serde_json::from_value::<Command>(call.clone()).is_ok(),
                "{call}"
            );
        }
    }
}
