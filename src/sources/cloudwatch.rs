//! Polls `aws cloudwatch describe-alarms --state-value ALARM` for each
//! configured profile and region. New alarms appear as critical islands; an
//! alarm that drops out of ALARM gets a short "OK" island. Uses the AWS CLI, so
//! SSO profiles work (run `aws sso login` when the session expires).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::aws::{self, AlarmState};
use crate::config::{AlarmRule, AwsAccount, CloudWatchConfig};
use crate::island::{Island, Kind, Severity};
use crate::source::{Inbound, Sink, Source};

const MIN_INTERVAL_SECS: u64 = 15;

pub struct CloudWatch {
    config: CloudWatchConfig,
    rules: Vec<AlarmRule>,
    cli: Option<String>,
}

impl CloudWatch {
    pub fn new(config: CloudWatchConfig, rules: Vec<AlarmRule>, cli: Option<String>) -> Self {
        Self { config, rules, cli }
    }

    fn poll(&self, account: &AwsAccount) -> Result<Vec<AlarmState>, String> {
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
        args.extend(aws::profile_args(
            account.profile.as_deref(),
            account.region.as_deref(),
        ));
        let out = aws::cli(self.cli.as_deref(), &args)?;
        aws::parse_describe_alarms(&out)
    }
}

/// One poll for the Test button in Preferences: a one-line summary.
pub fn probe(account: &AwsAccount, cli: Option<&str>) -> Result<String, String> {
    let cw = CloudWatch::new(
        CloudWatchConfig::default(),
        Vec::new(),
        cli.map(String::from),
    );
    let alarms = cw.poll(account)?;
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
    account
        .label
        .clone()
        .or_else(|| account.profile.clone())
        .unwrap_or_else(|| "default".into())
}

fn error_key(account: &AwsAccount) -> String {
    format!(
        "error:{}:{}",
        label(account),
        account.region.as_deref().unwrap_or("-")
    )
}

fn error_island(account: &AwsAccount, err: &str) -> Island {
    let hint = if err.contains("SSO") || err.contains("expired") || err.contains("token") {
        format!(
            "Run: aws sso login --profile {}",
            account.profile.as_deref().unwrap_or("default")
        )
    } else {
        "Check the profile and region in config.json".into()
    };
    Island::new(
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
    .ttl(600)
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
        loop {
            let started = Instant::now();
            for (i, account) in self.config.accounts.iter().enumerate() {
                let name = label(account);
                match self.poll(account) {
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
            // Wait out the interval; actions on alarm islands are all built in.
            let deadline = started + interval;
            loop {
                match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(_) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }
}
