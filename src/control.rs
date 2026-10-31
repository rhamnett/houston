//! Remote control: everything Mission settings and the tray menu can do, as
//! commands for the local API and the MCP server (`mcp.rs`). The API thread
//! sends a [`Command`] to the main thread and waits for the answer; slow ones
//! (an AWS or Linear check) answer from a background thread.
//!
//! Secrets follow the same rules as Mission settings: they can be replaced or
//! removed, never read back.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::{AwsAccount, Config};
use crate::island::Island;
use crate::secrets::Secret;
use crate::source::Inbound;
use crate::{App, aws, sources};

pub type Reply = Sender<Result<Value, String>>;

/// One command, shaped like an MCP `tools/call`: `{"name": …, "arguments": {…}}`.
#[derive(Debug, Deserialize)]
#[serde(tag = "name", content = "arguments", rename_all = "snake_case")]
pub enum Command {
    GetSettings {},
    /// A JSON merge patch (RFC 7396) over the settings.
    UpdateSettings {
        patch: Value,
    },
    /// Keychain account name; an empty value removes the secret.
    SetSecret {
        name: String,
        #[serde(default)]
        value: String,
    },
    ListAwsProfiles {},
    GetStatus {},
    SetRadioSilence {
        on: bool,
    },
    ClearBoard {},
    DismissIsland {
        id: String,
    },
    PressButton {
        id: String,
        action: String,
    },
    RunDrill {
        trigger: String,
    },
    TestCloudwatch {
        account: usize,
    },
    DiscoverAlarms {
        account: usize,
    },
    TestLinear {},
    TestAlarmRule {
        name: String,
    },
    CheckForUpdates {},
    /// Handled by the API thread itself: it owns the `api` source's sink.
    PostIsland {
        island: Box<Island>,
    },
    RemoveIsland {
        key: String,
    },
}

/// RFC 7396: objects merge key by key, `null` deletes, anything else replaces.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    let Value::Object(target) = target else {
        return;
    };
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

/// The settings `config` would have after `patch`, without secrets. Refuses
/// patches that carry a secret, or that make the settings unreadable or
/// invalid, before anything is saved.
pub fn patched(config: &Config, patch: &Value) -> Result<Value, String> {
    if !patch.is_object() {
        return Err("The patch must be a JSON object.".into());
    }
    let mut value = serde_json::to_value(config.without_secrets()).map_err(|e| e.to_string())?;
    merge_patch(&mut value, patch);
    let secret_in_patch = [&value["api"]["token"], &value["linear"]["api_key"]]
        .iter()
        .any(|v| v.as_str().is_some_and(|s| !s.is_empty()));
    if secret_in_patch {
        return Err("Secrets aren't settings: use set_secret.".into());
    }
    let next: Config = serde_json::from_value(value.clone())
        .map_err(|e| format!("Couldn't read the settings: {e}"))?;
    next.validate()?;
    Ok(value)
}

/// Whether the API may write the Keychain secret `name`.
pub fn check_secret(config: &Config, name: &str) -> Result<(), String> {
    if name == Secret::ApiToken.account() {
        return Err(
            "The API token can only be regenerated, in Mission settings > Local API.".into(),
        );
    }
    if let Some(id) = name.strip_prefix("aws-secret:") {
        if !config.access_key_ids().iter().any(|k| k == id) {
            return Err(format!(
                "No account uses access key {id}: add it with update_settings first."
            ));
        }
        return Ok(());
    }
    if Secret::ALL.iter().any(|s| s.account() == name) {
        Ok(())
    } else {
        Err(format!(
            "No secret called {name}: use linear-api-key, github-token or aws-secret:<access key id>."
        ))
    }
}

/// Run `work` off the main thread and send its answer.
fn background(reply: Reply, work: impl FnOnce() -> Result<Value, String> + Send + 'static) {
    std::thread::spawn(move || {
        let _ = reply.send(work());
    });
}

impl App {
    pub(crate) fn on_control(&mut self, command: Command, reply: Reply) {
        let result = match command {
            Command::TestCloudwatch { account } => match self.account(account) {
                Ok(account) => {
                    let cli = self.config.aws_cli.clone();
                    return background(reply, move || {
                        sources::probe_cloudwatch(&account, cli.as_deref(), None)
                            .map(|text| json!({ "result": text }))
                    });
                }
                Err(e) => Err(e),
            },
            Command::DiscoverAlarms { account } => match self.account(account) {
                Ok(account) => {
                    let cli = self.config.aws_cli.clone();
                    return background(reply, move || {
                        sources::discover_alarms(&account, cli.as_deref(), None)
                            .map(|alarms| json!({ "alarms": alarms }))
                    });
                }
                Err(e) => Err(e),
            },
            Command::TestLinear {} => {
                let linear = self.config.linear.clone();
                return background(reply, move || {
                    sources::probe_linear(&linear).map(|text| json!({ "result": text }))
                });
            }
            command => self.run(command),
        };
        let _ = reply.send(result);
        self.render();
    }

    fn account(&self, index: usize) -> Result<AwsAccount, String> {
        let accounts = &self.config.cloudwatch.accounts;
        accounts.get(index).cloned().ok_or_else(|| {
            format!(
                "No CloudWatch account {index}: there are {} (numbered from 0).",
                accounts.len()
            )
        })
    }

    fn settings(&self) -> Value {
        json!({
            "settings": self.config.without_secrets(),
            "secrets": self.secret_flags(),
            "config_file": Config::path(),
        })
    }

    fn status(&self) -> Value {
        let islands: Vec<Value> = self
            .store
            .sorted()
            .iter()
            .map(|e| json!({ "id": e.id, "updated_ms": e.updated_ms, "island": e.island }))
            .collect();
        json!({
            "islands": islands,
            "radio_silence": self.config.do_not_disturb,
            "quiet": self.quiet(),
            "focus": self.focus_label(),
            "sources": self.registry.ids(),
            "statuses": self.statuses,
            "watching": self.watching,
            "drills": self.config.demo.autoplay,
            "drill_triggers": sources::DEMO_TRIGGERS.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        })
    }

    /// Mission settings shows what was saved, if it's open.
    fn refresh_prefs(&self) {
        self.reply(self.prefs_state(true));
    }

    fn run(&mut self, command: Command) -> Result<Value, String> {
        match command {
            Command::GetSettings {} => Ok(self.settings()),
            Command::UpdateSettings { patch } => {
                let value = patched(&self.config, &patch)?;
                let restarted = self.apply_prefs(value, &HashMap::new())?;
                self.refresh_prefs();
                Ok(json!({ "restarted": restarted, "settings": self.config.without_secrets() }))
            }
            Command::SetSecret { name, value } => {
                check_secret(&self.config, &name)?;
                let current = serde_json::to_value(self.config.without_secrets())
                    .map_err(|e| e.to_string())?;
                let edits = HashMap::from([(name.clone(), value.clone())]);
                let restarted = self.apply_prefs(current, &edits)?;
                self.refresh_prefs();
                Ok(json!({ "name": name, "set": !value.trim().is_empty(), "restarted": restarted }))
            }
            Command::ListAwsProfiles {} => Ok(json!({ "profiles": aws::profiles() })),
            Command::GetStatus {} => Ok(self.status()),
            Command::SetRadioSilence { on } => {
                self.set_dnd(on);
                self.refresh_prefs();
                Ok(json!({ "radio_silence": on, "quiet": self.quiet() }))
            }
            Command::ClearBoard {} => {
                self.store.clear();
                Ok(json!({ "cleared": true }))
            }
            Command::DismissIsland { id } => {
                if !self.store.dismiss(&id) {
                    return Err(format!("No island {id} on the board."));
                }
                Ok(json!({ "dismissed": id }))
            }
            Command::PressButton { id, action } => {
                let entry = self
                    .store
                    .get(&id)
                    .ok_or_else(|| format!("No island {id} on the board."))?;
                if !entry.island.actions.iter().any(|a| a.id == action) {
                    let ids: Vec<&str> =
                        entry.island.actions.iter().map(|a| a.id.as_str()).collect();
                    return Err(format!(
                        "Island {id} has no {action} button; it has {ids:?}."
                    ));
                }
                self.act(&id, &action);
                Ok(json!({ "pressed": action, "id": id }))
            }
            Command::RunDrill { trigger } => {
                if !sources::DEMO_TRIGGERS.iter().any(|(t, _)| *t == trigger) {
                    return Err(format!("No drill {trigger}."));
                }
                self.registry
                    .send(sources::DEMO_ID, Inbound::Trigger(trigger.clone()));
                Ok(json!({ "started": trigger }))
            }
            Command::TestAlarmRule { name } => {
                let alarm = aws::AlarmState {
                    name,
                    ..Default::default()
                };
                let index = alarm.rule_index(&self.config.alarm_rules);
                Ok(json!({
                    "index": index,
                    "rule": index.map(|i| &self.config.alarm_rules[i]),
                }))
            }
            Command::CheckForUpdates {} => {
                if !self.check_for_updates() {
                    return Err("Update checks are off: set updates.check to true.".into());
                }
                Ok(json!({
                    "checking": true,
                    "current": env!("CARGO_PKG_VERSION"),
                    "note": "The answer arrives as an island with key \"update\" from source \"updates\".",
                }))
            }
            Command::PostIsland { .. } | Command::RemoveIsland { .. } => {
                Err("Islands are posted through the API.".into())
            }
            // Answered in the background by `on_control`.
            Command::TestCloudwatch { .. }
            | Command::DiscoverAlarms { .. }
            | Command::TestLinear {} => Err("Checks run in the background.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_patches() {
        let mut v = json!({"a": 1, "b": {"c": 2, "d": 3}, "e": [1, 2]});
        merge_patch(
            &mut v,
            &json!({"b": {"c": null, "x": true}, "e": [9], "f": "new"}),
        );
        assert_eq!(
            v,
            json!({"a": 1, "b": {"d": 3, "x": true}, "e": [9], "f": "new"})
        );
    }

    #[test]
    fn merge_patch_edges() {
        let mut v = json!({"a": 1});
        merge_patch(&mut v, &json!({"a": {"b": 2}}));
        assert_eq!(v, json!({"a": {"b": 2}}), "a scalar becomes an object");
        merge_patch(&mut v, &json!({"a": null, "z": null}));
        assert_eq!(v, json!({}), "null removes, even keys that weren't there");
        let mut v = json!([1, 2]);
        merge_patch(&mut v, &json!({"k": [3]}));
        assert_eq!(v, json!({"k": [3]}), "a non-object target is replaced");
        let mut v = json!({"a": 1});
        merge_patch(&mut v, &json!({}));
        assert_eq!(v, json!({"a": 1}), "an empty patch changes nothing");
    }

    #[test]
    fn patches_apply_over_the_current_settings() {
        let config = Config::default();
        let v = patched(
            &config,
            &json!({"sounds": false, "linear": {"teams": ["APO"]}}),
        )
        .unwrap();
        assert_eq!(v["sounds"], false);
        assert_eq!(v["linear"]["teams"], json!(["APO"]));
        assert_eq!(v["linear"]["max_priority"], 2, "untouched keys stay");
        assert_eq!(v["api"]["port"], 7878);

        let v = patched(&config, &json!({"cloudwatch": {"accounts": []}})).unwrap();
        assert_eq!(
            v["cloudwatch"]["accounts"],
            json!([]),
            "arrays replace whole"
        );

        let tweaked = Config {
            sounds: false,
            ..Config::default()
        };
        let v = patched(&tweaked, &json!({"sounds": null})).unwrap();
        let back: Config = serde_json::from_value(v).unwrap();
        assert!(back.sounds, "null puts the default back");
    }

    #[test]
    fn patches_that_are_refused() {
        let mut config = Config::default();
        config.api.token = "s3cr3t-t0ken".into();
        for (patch, why) in [
            (json!([1]), "JSON object"),
            (json!({"linear": {"api_key": "lin_x"}}), "set_secret"),
            (json!({"api": {"token": "x"}}), "set_secret"),
            (json!({"api": {"port": 80}}), "1024"),
            (json!({"sounds": "loud"}), "Couldn't read"),
            (json!({"alarm_rules": [{"pattern": " "}]}), "no pattern"),
            (
                json!({"sqs": {"enabled": true, "queue_url": "http://x"}}),
                "https",
            ),
        ] {
            let err = patched(&config, &patch).unwrap_err();
            assert!(err.contains(why), "{patch}: {err}");
        }
        let v = patched(&config, &json!({"sounds": true})).unwrap();
        assert!(
            !v.to_string().contains("s3cr3t-t0ken"),
            "the token never comes back"
        );
    }

    #[test]
    fn secrets_the_api_may_write() {
        let mut config = Config::default();
        config.cloudwatch.accounts[0].auth = crate::config::AwsAuth::AccessKey {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
        };
        assert!(check_secret(&config, "linear-api-key").is_ok());
        assert!(check_secret(&config, "aws-secret:AKIAIOSFODNN7EXAMPLE").is_ok());
        assert!(
            check_secret(&config, "api-token")
                .unwrap_err()
                .contains("regenerated")
        );
        assert!(
            check_secret(&config, "aws-secret:AKIAOTHER000000000")
                .unwrap_err()
                .contains("No account")
        );
        assert!(check_secret(&config, "github-token").is_ok());
        assert!(
            check_secret(&config, "slack-token")
                .unwrap_err()
                .contains("No secret")
        );
    }

    #[test]
    fn every_command_has_a_tool() {
        let names: Vec<String> = crate::mcp::tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        for name in [
            "get_settings",
            "update_settings",
            "set_secret",
            "list_aws_profiles",
            "get_status",
            "set_radio_silence",
            "clear_board",
            "dismiss_island",
            "press_button",
            "run_drill",
            "test_cloudwatch",
            "discover_alarms",
            "test_linear",
            "test_alarm_rule",
            "post_island",
            "remove_island",
            "check_for_updates",
        ] {
            assert!(names.iter().any(|n| n == name), "{name} isn't advertised");
        }
        assert_eq!(names.len(), 17, "a tool without a command, or the reverse");
    }

    #[test]
    fn commands_parse_like_tool_calls() {
        let c: Command =
            serde_json::from_value(json!({"name": "get_settings", "arguments": {}})).unwrap();
        assert!(matches!(c, Command::GetSettings {}));
        let c: Command =
            serde_json::from_value(json!({"name": "set_radio_silence", "arguments": {"on": true}}))
                .unwrap();
        assert!(matches!(c, Command::SetRadioSilence { on: true }));
        assert!(
            serde_json::from_value::<Command>(json!({"name": "nope", "arguments": {}})).is_err()
        );
    }
}
