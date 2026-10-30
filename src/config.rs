//! Settings in ~/Library/Application Support/houston/config.json. Written
//! with every option on first launch so it can be edited by hand, or changed
//! in Preferences, which restarts only the sources whose settings changed.
//!
//! Secrets (the API token, the Linear key) are kept in the Keychain, never in
//! the file: see `secrets.rs`. A secret found in the file (an older version,
//! or a hand edit) is moved to the Keychain and blanked on the next load.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::island::Severity;
use crate::secrets::{self, Secret};

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
    /// Bearer token for every request but `/v1/health`. Kept in the Keychain;
    /// generated when missing.
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

/// How Houston signs AWS calls.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AwsAuth {
    /// A CLI profile: SSO (`aws sso login --profile …`), a credential process,
    /// or keys in ~/.aws/credentials.
    #[default]
    Profile,
    /// A long-lived IAM access key. The secret half is in the Keychain under
    /// `aws-secret:<access_key_id>`; only the id is in the file.
    AccessKey { access_key_id: String },
}

/// One AWS account and region to poll.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AwsAccount {
    #[serde(default)]
    pub auth: AwsAuth,
    /// The CLI profile, when `auth` is a profile.
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
    /// Only these alarms (exact names), as picked with Discover alarms in
    /// Mission settings. Empty watches every alarm matching the prefix.
    #[serde(default)]
    pub include: Vec<String>,
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
                ..AwsAccount::default()
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
    pub auth: AwsAuth,
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
            auth: AwsAuth::default(),
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
    /// A personal API key (Linear > Settings > Security & access), kept in the
    /// Keychain. Falls back to the `LINEAR_API_KEY` environment variable.
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
    /// Also go quiet while a macOS Focus (Do Not Disturb, Work…) is on.
    #[serde(default = "yes")]
    pub follow_focus: bool,
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
            follow_focus: true,
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

pub fn random_token() -> String {
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

    pub fn secret(&self, s: Secret) -> &str {
        match s {
            Secret::ApiToken => &self.api.token,
            Secret::LinearApiKey => &self.linear.api_key,
        }
    }

    fn secret_mut(&mut self, s: Secret) -> &mut String {
        match s {
            Secret::ApiToken => &mut self.api.token,
            Secret::LinearApiKey => &mut self.linear.api_key,
        }
    }

    /// Put a secret in the Keychain (empty removes it) and in memory.
    pub fn set_secret(&mut self, s: Secret, value: &str) -> Result<(), String> {
        secrets::set(s, value)?;
        *self.secret_mut(s) = value.to_string();
        Ok(())
    }

    /// Every IAM access key id the config uses, for their Keychain secrets.
    pub fn access_key_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .cloudwatch
            .accounts
            .iter()
            .map(|a| &a.auth)
            .chain([&self.sqs.auth])
            .filter_map(|auth| match auth {
                AwsAuth::AccessKey { access_key_id } if !access_key_id.is_empty() => {
                    Some(access_key_id.clone())
                }
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Keep `from`'s secrets (Preferences never sees them, so a config it
    /// sends back has them blank).
    pub fn keep_secrets(&mut self, from: &Config) {
        for s in Secret::ALL {
            *self.secret_mut(s) = from.secret(s).to_string();
        }
    }

    /// Problems a hand edit or Preferences could introduce, one per line.
    pub fn validate(&self) -> Result<(), String> {
        let mut problems = Vec::new();
        if self.api.enabled && self.api.port < 1024 {
            problems.push("Local API port must be 1024 or higher.".to_string());
        }
        if self.sqs.enabled && !self.sqs.queue_url.starts_with("https://") {
            problems.push("SQS queue URL must start with https://.".into());
        }
        let auths = self
            .cloudwatch
            .accounts
            .iter()
            .map(|a| &a.auth)
            .chain([&self.sqs.auth]);
        for auth in auths {
            if let AwsAuth::AccessKey { access_key_id } = auth
                && !secrets::is_access_key_id(access_key_id)
            {
                problems.push(format!(
                    "\"{access_key_id}\" doesn't look like an access key id (AKIA…)."
                ));
            }
        }
        if self.linear.enabled && !(1..=4).contains(&self.linear.max_priority) {
            problems.push("Linear priority must be 1 to 4.".into());
        }
        for (i, r) in self.alarm_rules.iter().enumerate() {
            if r.pattern.trim().is_empty() {
                problems.push(format!("Alarm rule {} has no pattern.", i + 1));
            }
            if r.runbook_url
                .as_deref()
                .is_some_and(|u| !crate::island::is_web_url(u))
            {
                problems.push(format!(
                    "Alarm rule {}: runbook URL must be http or https.",
                    i + 1
                ));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join(" "))
        }
    }

    /// A copy safe to write to disk or show in a page.
    pub fn without_secrets(&self) -> Self {
        let mut c = self.clone();
        for s in Secret::ALL {
            c.secret_mut(s).clear();
        }
        c
    }

    /// Load the file and the Keychain, generating a token on first run.
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
        for s in Secret::ALL {
            let in_file = config.secret(s).to_string();
            if !in_file.is_empty() {
                // Move it out of the file; keep it in memory either way.
                match secrets::set(s, &in_file) {
                    Ok(()) => changed = true,
                    Err(e) => eprintln!("config: {e}; left in the file"),
                }
            } else if let Some(v) = secrets::get(s) {
                *config.secret_mut(s) = v;
            }
        }
        if config.api.token.is_empty()
            && let Err(e) = config.set_secret(Secret::ApiToken, &random_token())
        {
            eprintln!("config: {e}");
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
        let json = serde_json::to_vec_pretty(&self.without_secrets()).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
        // No secrets in it, but alarm names and account ids are nobody else's business.
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
    fn secrets_never_reach_the_file() {
        let mut c = Config::default();
        c.api.token = "s3cr3t-t0ken".into();
        c.linear.api_key = "lin_api_x".into();
        let written = serde_json::to_string(&c.without_secrets()).unwrap();
        assert!(!written.contains("s3cr3t-t0ken") && !written.contains("lin_api_x"));
        assert_eq!(
            c.secret(Secret::LinearApiKey),
            "lin_api_x",
            "memory keeps them"
        );
    }

    #[test]
    fn older_accounts_default_to_profiles() {
        let a: AwsAccount = serde_json::from_str(r#"{"profile":"apollo-prod"}"#).unwrap();
        assert_eq!(a.auth, AwsAuth::Profile);
        assert!(a.include.is_empty());
        let k: AwsAccount = serde_json::from_str(
            r#"{"auth":{"type":"access_key","access_key_id":"AKIAIOSFODNN7EXAMPLE"},"region":"eu-west-2"}"#,
        )
        .unwrap();
        let mut c = Config::default();
        c.cloudwatch.accounts = vec![k.clone(), k];
        assert_eq!(c.access_key_ids(), ["AKIAIOSFODNN7EXAMPLE"]);
        assert!(c.validate().is_ok());
        c.sqs.auth = AwsAuth::AccessKey {
            access_key_id: "nope".into(),
        };
        assert!(c.validate().unwrap_err().contains("nope"));
    }

    #[test]
    fn validation() {
        let mut c = Config::default();
        assert!(c.validate().is_ok());
        c.api.port = 80;
        c.alarm_rules
            .push(serde_json::from_str(r#"{"pattern":" ","runbook_url":"file:///x"}"#).unwrap());
        let err = c.validate().unwrap_err();
        assert!(
            err.contains("port") && err.contains("no pattern") && err.contains("runbook"),
            "{err}"
        );
    }

    #[test]
    fn token_is_random_hex() {
        let a = random_token();
        assert_eq!(a.len(), 48);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_token());
    }
}
