//! Polls `aws cloudwatch describe-alarms --state-value ALARM` for each
//! configured profile and region. New alarms appear as critical islands; an
//! alarm that drops out of ALARM gets a short "OK" island. Uses the AWS CLI, so
//! SSO profiles work; when a session expires, the warning island has a
//! **Sign in** button that runs `aws sso login`. Accounts can use an IAM access
//! key from the Keychain instead.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::aws::{self, AlarmState, Creds, Discovered};
use crate::config::{AlarmRule, AwsAccount, AwsAuth, CloudWatchConfig};
use crate::island::{Action, Island, Kind, Severity};
use crate::source::{Inbound, Sink, Source};

const MIN_INTERVAL_SECS: u64 = 15;
const SSO_LOGIN: &str = "sso-login";

fn creds(account: &AwsAccount, secret: Option<&str>) -> Result<Creds, String> {
    Creds::resolve(
        &account.auth,
        account.profile.as_deref(),
        account.region.as_deref(),
        secret,
    )
}

pub struct CloudWatch {
    config: CloudWatchConfig,
    rules: Vec<AlarmRule>,
    cli: Option<String>,
}

impl CloudWatch {
    pub fn new(config: CloudWatchConfig, rules: Vec<AlarmRule>, cli: Option<String>) -> Self {
        Self { config, rules, cli }
    }

    /// Alarms in ALARM; `secret` overrides the Keychain (Preferences' Test).
    fn poll(&self, account: &AwsAccount, secret: Option<&str>) -> Result<Vec<AlarmState>, String> {
        let mut args = vec![
            "cloudwatch",
            "describe-alarms",
            "--state-value",
            "ALARM",
            "--alarm-types",
            "MetricAlarm",
            "CompositeAlarm",
            "--output",
            "json",
        ];
        if let Some(prefix) = account.alarm_prefix.as_deref() {
            args.extend(["--alarm-name-prefix", prefix]);
        }
        let out = aws::cli(self.cli.as_deref(), &args, &creds(account, secret)?)?;
        let mut alarms = aws::parse_describe_alarms(&out)?;
        if !account.include.is_empty() {
            alarms.retain(|a| account.include.contains(&a.name));
        }
        Ok(alarms)
    }
}

/// Every alarm in the account, for Discover alarms in Preferences.
pub fn discover(
    account: &AwsAccount,
    cli: Option<&str>,
    secret: Option<&str>,
) -> Result<Vec<Discovered>, String> {
    aws::discover_alarms(
        cli,
        &creds(account, secret)?,
        account.alarm_prefix.as_deref(),
    )
}

/// One poll for the Test button in Preferences: a one-line summary.
pub fn probe(
    account: &AwsAccount,
    cli: Option<&str>,
    secret: Option<&str>,
) -> Result<String, String> {
    let cw = CloudWatch::new(
        CloudWatchConfig::default(),
        Vec::new(),
        cli.map(String::from),
    );
    let alarms = cw.poll(account, secret)?;
    Ok(match alarms.len() {
        0 => format!("Connected to {}. No alarms in ALARM.", label(account)),
        n => {
            let names: Vec<&str> = alarms.iter().take(3).map(|a| a.name.as_str()).collect();
            let more = if n > 3 {
                format!(" and {} more", n - 3)
            } else {
                String::new()
            };
            format!(
                "Connected to {}. {n} in ALARM: {}{more}.",
                label(account),
                names.join(", ")
            )
        }
    })
}

fn label(account: &AwsAccount) -> String {
    let fallback = match &account.auth {
        AwsAuth::Profile => account.profile.clone(),
        AwsAuth::AccessKey { access_key_id } => Some(format!(
            "key …{}",
            &access_key_id[access_key_id.len().saturating_sub(4)..]
        )),
    };
    account
        .label
        .clone()
        .or(fallback)
        .unwrap_or_else(|| "default".into())
}

fn error_key(account: &AwsAccount) -> String {
    format!(
        "error:{}:{}",
        label(account),
        account.region.as_deref().unwrap_or("-")
    )
}

/// An expired or missing SSO session, as the CLI words it.
fn needs_sso_login(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("sso") || e.contains("expired") || e.contains("token")
}

fn error_island(account: &AwsAccount, err: &str) -> Island {
    let profile = account.profile.as_deref().unwrap_or("default");
    let sso = account.auth == AwsAuth::Profile && needs_sso_login(err);
    let hint = match &account.auth {
        AwsAuth::Profile if sso => format!("SSO session for {profile} has expired"),
        AwsAuth::Profile => "Check the profile and region in Mission settings".into(),
        AwsAuth::AccessKey { .. } => "Check the access key in Mission settings".into(),
    };
    let island = Island::new(
        &error_key(account),
        Kind::Message { sender: None },
        &format!("Lost signal from {}", label(account)),
    )
    .subtitle(hint)
    .body(err.to_string())
    .icon("🔑")
    .severity(Severity::Warning)
    .compact("AWS")
    .revision(err.to_string())
    .ttl(600);
    if sso {
        island.action(Action::new(SSO_LOGIN, "Sign in").primary())
    } else {
        island
    }
}

impl Source for CloudWatch {
    fn id(&self) -> String {
        "cloudwatch".into()
    }

    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>) {
        let interval = Duration::from_secs(self.config.interval_secs.max(MIN_INTERVAL_SECS));
        sink.status(format!(
            "cloudwatch: polling {} account(s) every {}s",
            self.config.accounts.len(),
            interval.as_secs()
        ));
        // Per account, the alarms in ALARM at the last good poll (keyed by island key).
        let mut known: HashMap<usize, HashMap<String, AlarmState>> = HashMap::new();
        // Set when a Sign in finishes, to poll straight away.
        let poll_now = Arc::new(AtomicBool::new(false));
        loop {
            let started = Instant::now();
            for (i, account) in self.config.accounts.iter().enumerate() {
                let name = label(account);
                match self.poll(account, None) {
                    Ok(alarms) => {
                        sink.remove(&error_key(account));
                        let now: HashMap<String, AlarmState> =
                            alarms.into_iter().map(|a| (a.key(), a)).collect();
                        for alarm in now.values() {
                            if let Some(island) = alarm.island_with(Some(&name), &self.rules) {
                                sink.upsert(island);
                            }
                        }
                        if let Some(before) = known.get(&i) {
                            let still: HashSet<&String> = now.keys().collect();
                            for (key, alarm) in before {
                                if !still.contains(key) {
                                    let ok = AlarmState {
                                        state: "OK".into(),
                                        previous: Some("ALARM".into()),
                                        reason: Some("No longer in ALARM.".into()),
                                        updated: None,
                                        ..alarm.clone()
                                    };
                                    if let Some(island) = ok.island_with(Some(&name), &self.rules) {
                                        sink.upsert(island);
                                    }
                                }
                            }
                        }
                        known.insert(i, now);
                    }
                    Err(e) => {
                        crate::log::line(&format!("cloudwatch {name}: {e}"));
                        sink.upsert(error_island(account, &e));
                    }
                }
            }
            // Wait out the interval, in short steps so a finished Sign in
            // can cut it short.
            let deadline = started + interval;
            while !poll_now.swap(false, Ordering::Relaxed) {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match inbox.recv_timeout(left.min(Duration::from_secs(1))) {
                    Ok(Inbound::Action { key, action }) if action == SSO_LOGIN => {
                        let Some(account) =
                            self.config.accounts.iter().find(|a| error_key(a) == key)
                        else {
                            continue;
                        };
                        let profile = account.profile.clone().unwrap_or_else(|| "default".into());
                        let (cli, sink, done) = (self.cli.clone(), sink.clone(), poll_now.clone());
                        sink.status(format!(
                            "cloudwatch: signing in to {profile} in the browser"
                        ));
                        // Waits for the browser, so off this thread.
                        std::thread::spawn(move || {
                            match aws::sso_login(cli.as_deref(), &profile) {
                                Ok(msg) => {
                                    sink.status(format!("cloudwatch: {msg}"));
                                    done.store(true, Ordering::Relaxed);
                                }
                                Err(e) => {
                                    sink.status(format!("cloudwatch: sign in to {profile}: {e}"))
                                }
                            }
                        });
                    }
                    Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }
}
