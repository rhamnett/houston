//! Follows macOS Focus (Do Not Disturb, Work, Sleep…). Apple's Focus Status
//! API needs the Communication Notifications entitlement, which an ad-hoc
//! signed app can't have, so this reads the Do Not Disturb store directly:
//!
//! - `Assertions.json` lists the Focus modes switched on by hand, from
//!   Control Centre, a Shortcut, or a synced device
//! - `ModeConfigurations.json` maps each mode's identifier to its name
//!
//! Newer macOS versions protect the folder; reading it then needs Full Disk
//! Access for Houston. Focus modes switched on by a schedule or another
//! automation don't appear in the store, so those aren't followed.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    Off,
    /// The name of the active Focus, e.g. "Do Not Disturb" or "Work".
    On(String),
    /// The store can't be read (usually: no Full Disk Access).
    Unreadable,
}

impl Focus {
    pub fn is_on(&self) -> bool {
        matches!(self, Focus::On(_))
    }

    /// For the menu and Preferences.
    pub fn describe(&self) -> String {
        match self {
            Focus::Off => "No Focus on".into(),
            Focus::On(name) => format!("{name} is on"),
            Focus::Unreadable => "Can't read Focus: needs Full Disk Access".into(),
        }
    }
}

fn store() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/DoNotDisturb/DB"))
}

/// The current Focus. Cheap enough to call every few seconds.
pub fn read() -> Focus {
    let Some(dir) = store() else {
        return Focus::Off;
    };
    let assertions = match std::fs::read(dir.join("Assertions.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Focus::Off,
        Err(_) => return Focus::Unreadable,
    };
    let Ok(assertions) = serde_json::from_slice::<Value>(&assertions) else {
        return Focus::Off;
    };
    let Some(mode) = active_mode(&assertions) else {
        return Focus::Off;
    };
    let names = std::fs::read(dir.join("ModeConfigurations.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    Focus::On(
        names
            .as_ref()
            .and_then(|n| mode_name(n, &mode))
            .unwrap_or_else(|| "Focus".into()),
    )
}

/// The mode identifier of the first active assertion, or "" when one is
/// active without saying which mode.
fn active_mode(assertions: &Value) -> Option<String> {
    let records: Vec<&Value> = assertions["data"]
        .as_array()?
        .iter()
        .filter_map(|d| d["storeAssertionRecords"].as_array())
        .flatten()
        .collect();
    let first = records.first()?;
    Some(
        first["assertionDetails"]["assertionDetailsModeIdentifier"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

fn mode_name(configs: &Value, mode: &str) -> Option<String> {
    configs["data"]
        .as_array()?
        .iter()
        .find_map(|d| d["modeConfigurations"][mode]["mode"]["name"].as_str())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_active_mode() {
        let idle = json!({"data": [{}], "header": {"version": 1}});
        assert_eq!(active_mode(&idle), None);

        let on = json!({"data": [{"storeAssertionRecords": [{
            "assertionDetails": {
                "assertionDetailsIdentifier": "com.apple.donotdisturb.control-center.module",
                "assertionDetailsModeIdentifier": "com.apple.focus.work",
                "assertionDetailsReason": "user-action"
            }
        }]}]});
        assert_eq!(active_mode(&on).as_deref(), Some("com.apple.focus.work"));

        let configs = json!({"data": [{"modeConfigurations": {
            "com.apple.focus.work": {"mode": {"name": "Work", "modeIdentifier": "com.apple.focus.work"}}
        }}]});
        assert_eq!(
            mode_name(&configs, "com.apple.focus.work").as_deref(),
            Some("Work")
        );
        assert_eq!(mode_name(&configs, "com.apple.sleep.sleep-mode"), None);
    }

    #[test]
    fn descriptions() {
        assert!(Focus::On("Work".into()).is_on());
        assert!(!Focus::Unreadable.is_on());
        assert_eq!(Focus::On("Work".into()).describe(), "Work is on");
    }
}
