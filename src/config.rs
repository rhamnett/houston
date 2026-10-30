//! Settings in ~/Library/Application Support/houston/config.json. Written
//! with every option on first launch so it can be edited by hand; sources are
//! read at startup, so restart after changing them.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::island::Severity;

fn yes() -> bool {
    true
}

fn default_port() -> u16 {
    7878
}

fn default_interval() -> u64 {
    60
}

fn default_wait() -> u64 {
    20
}

fn default_max_priority() -> u8 {
    2
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Bearer token for every request but `/v1/health`. Generated when empty.
    #[serde(default)]
    pub token: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_port(),
            token: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemoConfig {
    /// Play the tour of notification types a second after launch.
    #[serde(default = "yes")]
    pub autoplay: bool,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self { autoplay: true }
    }
}

/// One AWS CLI profile and region to poll.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AwsAccount {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    /// Shown on the card; defaults to the profile name.
    #[serde(default)]
    pub label: Option<String>,
    /// Only alarms whose names start with this.
    #[serde(default)]
    pub alarm_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudWatchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default)]
    pub accounts: Vec<AwsAccount>,
}

impl Default for CloudWatchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_interval(),
            accounts: vec![AwsAccount {
                profile: Some("default".into()),
                region: Some("eu-west-2".into()),
                label: None,
                alarm_prefix: None,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SqsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub queue_url: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default = "default_wait")]
    pub wait_secs: u64,
}

impl Default for SqsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            queue_url: String::new(),
            profile: None,
            region: None,
            wait_secs: default_wait(),
        }
    }
}

/// How one group of alarms should look. `pattern` is matched against the
/// alarm name with `*` as a wildcard; the first matching rule wins. Alarms that
/// match no rule keep the defaults (ALARM is critical and sticky).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlarmRule {
    pub pattern: String,
    /// Severity while in ALARM.
    #[serde(default)]
    pub severity: Option<Severity>,
    /// Stay until dismissed. Defaults to true for critical, false otherwise.
    #[serde(default)]
    pub sticky: Option<bool>,
    /// Update in place without a peek or a sound.
    #[serde(default)]
    pub silent: bool,
    /// Drop the alarm entirely.
    #[serde(default)]
    pub mute: bool,
    /// Extra "Runbook ↗" button.
    #[serde(default)]
    pub runbook_url: Option<String>,
}

/// AWS IoT Core (MQTT over WSS) subscription. Not implemented yet: the source
/// only reports that in the status line. See `sources/iot.rs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IotConfig {
    #[serde(default)]
    pub enabled: bool,
    /// The ATS data endpoint, e.g. `abc123-ats.iot.eu-west-2.amazonaws.com`.
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    /// MQTT topic filters, e.g. `apollo/prod/t/+/call/+/events`.
    #[serde(default)]
    pub topics: Vec<String>,
}

/// Linear issues: yours, and urgent ones in the teams you watch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinearConfig {
    #[serde(default)]
    pub enabled: bool,
    /// A personal API key (Linear > Settings > Security & access). Falls back
    /// to the `LINEAR_API_KEY` environment variable when empty.
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Tell me when an issue is assigned to me or changes state.
    #[serde(default = "yes")]
    pub assigned_to_me: bool,
    /// Team keys to watch for high-priority issues, e.g. `["APO"]`.
    #[serde(default)]
    pub teams: Vec<String>,
    /// Watch team issues up to this priority: 1 urgent, 2 high, 3 medium.
    #[serde(default = "default_max_priority")]
    pub max_priority: u8,
}

impl Default for LinearConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            interval_secs: default_interval(),
            assigned_to_me: true,
            teams: Vec::new(),
            max_priority: default_max_priority(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Show islands without expanding or sounds.
    #[serde(default)]
    pub do_not_disturb: bool,
    #[serde(default = "yes")]
    pub sounds: bool,
    /// Path to the aws binary if it is not in the usual places.
    #[serde(default)]
    pub aws_cli: Option<String>,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub demo: DemoConfig,
    #[serde(default)]
    pub cloudwatch: CloudWatchConfig,
    #[serde(default)]
    pub sqs: SqsConfig,
    #[serde(default)]
    pub iot: IotConfig,
    #[serde(default)]
    pub linear: LinearConfig,
    /// Applied to alarms from every source (poll, SQS, local API).
    #[serde(default)]
    pub alarm_rules: Vec<AlarmRule>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            do_not_disturb: false,
            sounds: true,
            aws_cli: None,
            api: ApiConfig::default(),
            demo: DemoConfig::default(),
            cloudwatch: CloudWatchConfig::default(),
            sqs: SqsConfig::default(),
            iot: IotConfig::default(),
            linear: LinearConfig::default(),
            alarm_rules: Vec::new(),
        }
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Config {
    pub fn dir() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/houston"))
    }

    /// The app was called notch-tray; move its folder over once, keeping the token.
    fn migrate_old_dir() {
        let (Some(home), Some(new)) = (std::env::var_os("HOME"), Self::dir()) else {
            return;
        };
        let old = PathBuf::from(home).join("Library/Application Support/notch-tray");
        if old.is_dir()
            && !new.exists()
            && let Err(e) = fs::rename(&old, &new)
        {
            eprintln!("config: moving {} to {}: {e}", old.display(), new.display());
        }
    }

    pub fn path() -> Option<PathBuf> {
        Some(Self::dir()?.join("config.json"))
    }

    /// Load, filling in a token and writing the file if anything was missing.
    pub fn load() -> Self {
        Self::migrate_old_dir();
        let path = Self::path();
        let existing = path.as_ref().and_then(|p| fs::read(p).ok());
        let mut config = match &existing {
            Some(bytes) => serde_json::from_slice(bytes).unwrap_or_else(|e| {
                eprintln!("config: {e}; using defaults");
                Self::default()
            }),
            None => Self::default(),
        };
        let mut changed = existing.is_none();
        if config.api.token.is_empty() {
            config.api.token = random_token();
            changed = true;
        }
        if changed && let Err(e) = config.save() {
            eprintln!("config: {e}");
        }
        config
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Self::path() else {
            return Err("HOME is not set".into());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
        // The file holds the API token.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gets_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.sounds);
        assert!(c.api.enabled);
        assert_eq!(c.api.port, 7878);
        assert!(c.demo.autoplay);
        assert!(!c.cloudwatch.enabled);
        assert_eq!(c.cloudwatch.interval_secs, 60);
        assert_eq!(c.sqs.wait_secs, 20);
        assert!(!c.iot.enabled);
        assert!(!c.linear.enabled);
        assert!(c.linear.assigned_to_me);
        assert_eq!(c.linear.max_priority, 2);
        assert!(c.alarm_rules.is_empty());
    }

    #[test]
    fn token_is_random_hex() {
        let a = random_token();
        assert_eq!(a.len(), 48);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_token());
    }
}
