//! Everything that knows about AWS: signed calls to CloudWatch and SQS over
//! their JSON protocol (no AWS SDK, and the `aws` CLI only for SSO or role
//! profiles) and turning CloudWatch alarm payloads into islands. Three
//! payload shapes are understood:
//!
//! - rows from DescribeAlarms
//! - EventBridge `CloudWatch Alarm State Change` events
//! - SNS alarm notifications, raw or inside an SNS envelope
//!
//! It also resolves credentials (a CLI profile, or an IAM access key from the
//! Keychain), lists local profiles and discovers alarms for Preferences.
//!
//! It also maps Security Hub findings (EventBridge `Security Hub Findings -
//! Imported`, raw or inside an SNS envelope) and applies the `alarm_rules`
//! from the config.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{AlarmRule, AwsAuth};
use crate::island::{Action, Island, Kind, Severity};
use crate::secrets;
use crate::sigv4::{self, Keys};

/// One alarm's state, whichever way it reached us.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlarmState {
    pub name: String,
    pub description: Option<String>,
    /// `ALARM`, `OK` or `INSUFFICIENT_DATA`.
    pub state: String,
    pub previous: Option<String>,
    pub reason: Option<String>,
    /// When the state last changed; the island's revision.
    pub updated: Option<String>,
    pub region: Option<String>,
    pub account: Option<String>,
    pub namespace: Option<String>,
    pub metric: Option<String>,
    pub comparison: Option<String>,
    pub threshold: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DescribeAlarms {
    #[serde(default)]
    metric_alarms: Vec<DescribedAlarm>,
    #[serde(default)]
    composite_alarms: Vec<DescribedAlarm>,
    next_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DescribedAlarm {
    alarm_name: String,
    alarm_arn: Option<String>,
    alarm_description: Option<String>,
    state_value: String,
    state_reason: Option<String>,
    #[serde(default, deserialize_with = "timestamp")]
    state_updated_timestamp: Option<String>,
    metric_name: Option<String>,
    namespace: Option<String>,
    comparison_operator: Option<String>,
    threshold: Option<f64>,
}

/// The JSON protocol sends epoch seconds; the CLI printed ISO 8601. Both
/// become the CLI's form, so an island's revision (and so whether it's news)
/// doesn't change with the transport.
fn timestamp<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stamp {
        Text(String),
        Seconds(f64),
    }
    Ok(match Option::<Stamp>::deserialize(d)? {
        Some(Stamp::Text(t)) => Some(t),
        Some(Stamp::Seconds(s)) => {
            let micros = (s * 1e6).round() as i64;
            DateTime::from_timestamp_micros(micros)
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Micros, false))
        }
        None => None,
    })
}

#[derive(Deserialize)]
struct SnsEnvelope {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "Message")]
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SnsAlarm {
    alarm_name: String,
    alarm_description: Option<String>,
    #[serde(rename = "AWSAccountId")]
    account_id: Option<String>,
    new_state_value: String,
    new_state_reason: Option<String>,
    state_change_time: Option<String>,
    old_state_value: Option<String>,
    alarm_arn: Option<String>,
    trigger: Option<SnsTrigger>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SnsTrigger {
    metric_name: Option<String>,
    namespace: Option<String>,
    comparison_operator: Option<String>,
    threshold: Option<f64>,
}

#[derive(Deserialize)]
struct EventBridgeEvent {
    #[serde(rename = "detail-type")]
    detail_type: String,
    account: Option<String>,
    region: Option<String>,
    detail: EventDetail,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventDetail {
    alarm_name: String,
    state: EventState,
    previous_state: Option<EventState>,
    configuration: Option<EventConfig>,
}

#[derive(Deserialize)]
struct EventState {
    value: String,
    reason: Option<String>,
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct EventConfig {
    description: Option<String>,
    #[serde(default)]
    metrics: Vec<EventMetric>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventMetric {
    metric_stat: Option<EventMetricStat>,
}

#[derive(Deserialize)]
struct EventMetricStat {
    metric: EventMetricId,
}

#[derive(Deserialize)]
struct EventMetricId {
    namespace: Option<String>,
    name: Option<String>,
}

/// `arn:aws:cloudwatch:REGION:ACCOUNT:alarm:NAME` → (region, account).
fn arn_parts(arn: Option<&str>) -> (Option<String>, Option<String>) {
    let mut parts = arn.unwrap_or_default().split(':').skip(3);
    let region = parts.next().filter(|s| !s.is_empty()).map(String::from);
    let account = parts.next().filter(|s| !s.is_empty()).map(String::from);
    (region, account)
}

/// Parse a DescribeAlarms answer (or `aws cloudwatch describe-alarms`).
#[cfg(test)]
pub fn parse_describe_alarms(json: &str) -> Result<Vec<AlarmState>, String> {
    let d: DescribeAlarms =
        serde_json::from_str(json).map_err(|e| format!("describe-alarms: {e}"))?;
    Ok(alarm_states(d))
}

fn alarm_states(d: DescribeAlarms) -> Vec<AlarmState> {
    d.metric_alarms
        .into_iter()
        .chain(d.composite_alarms)
        .map(|a| {
            let (region, account) = arn_parts(a.alarm_arn.as_deref());
            AlarmState {
                name: a.alarm_name,
                description: a.alarm_description,
                state: a.state_value,
                previous: None,
                reason: a.state_reason,
                updated: a.state_updated_timestamp,
                region,
                account,
                namespace: a.namespace,
                metric: a.metric_name,
                comparison: a.comparison_operator,
                threshold: a.threshold,
            }
        })
        .collect()
}

/// An alarm from an SQS body or a webhook: an SNS envelope, an EventBridge
/// event or a bare SNS alarm message. `None` if it is none of those.
pub fn alarm_from_message(body: &str) -> Option<AlarmState> {
    if let Ok(env) = serde_json::from_str::<SnsEnvelope>(body)
        && env.kind == "Notification"
    {
        return alarm_from_message(&env.message);
    }
    if let Ok(ev) = serde_json::from_str::<EventBridgeEvent>(body)
        && ev.detail_type == "CloudWatch Alarm State Change"
    {
        let d = ev.detail;
        let metric = d
            .configuration
            .as_ref()
            .and_then(|c| c.metrics.first())
            .and_then(|m| m.metric_stat.as_ref())
            .map(|s| &s.metric);
        return Some(AlarmState {
            name: d.alarm_name,
            description: d.configuration.as_ref().and_then(|c| c.description.clone()),
            state: d.state.value,
            previous: d.previous_state.map(|p| p.value),
            reason: d.state.reason,
            updated: d.state.timestamp,
            region: ev.region,
            account: ev.account,
            namespace: metric.and_then(|m| m.namespace.clone()),
            metric: metric.and_then(|m| m.name.clone()),
            comparison: None,
            threshold: None,
        });
    }
    let a = serde_json::from_str::<SnsAlarm>(body).ok()?;
    let (region, arn_account) = arn_parts(a.alarm_arn.as_deref());
    let trigger = a.trigger.as_ref();
    Some(AlarmState {
        name: a.alarm_name,
        description: a.alarm_description,
        state: a.new_state_value,
        previous: a.old_state_value,
        reason: a.new_state_reason,
        updated: a.state_change_time,
        region,
        account: a.account_id.or(arn_account),
        namespace: trigger.and_then(|t| t.namespace.clone()),
        metric: trigger.and_then(|t| t.metric_name.clone()),
        comparison: trigger.and_then(|t| t.comparison_operator.clone()),
        threshold: trigger.and_then(|t| t.threshold),
    })
}

fn comparison_symbol(op: &str) -> &str {
    match op {
        "GreaterThanOrEqualToThreshold" => "≥",
        "GreaterThanThreshold" => ">",
        "LessThanThreshold" => "<",
        "LessThanOrEqualToThreshold" => "≤",
        "LessThanLowerOrGreaterThanUpperThreshold" => "outside band",
        "LessThanLowerThreshold" => "below band",
        "GreaterThanUpperThreshold" => "above band",
        other => other,
    }
}

fn number(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Percent-encode for a URL path or query component.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl AlarmState {
    pub fn key(&self) -> String {
        format!(
            "alarm:{}:{}:{}",
            self.account.as_deref().unwrap_or("-"),
            self.region.as_deref().unwrap_or("-"),
            self.name
        )
    }

    pub fn console_url(&self) -> Option<String> {
        let region = self.region.as_deref()?;
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudwatch/home?region={region}#alarmsV2:alarm/{}",
            encode_component(&self.name)
        ))
    }

    fn condition(&self) -> Option<String> {
        let metric = self.metric.as_deref()?;
        Some(match (&self.comparison, self.threshold) {
            (Some(op), Some(t)) => format!("{metric} {} {}", comparison_symbol(op), number(t)),
            _ => metric.to_string(),
        })
    }

    /// `account_label` names the account (e.g. the CLI profile) in the card.
    pub fn island(&self, account_label: Option<&str>) -> Island {
        let (severity, icon, compact) = match self.state.as_str() {
            "ALARM" => (Severity::Critical, "🚨", "ALARM"),
            "INSUFFICIENT_DATA" => (Severity::Warning, "❔", "NO DATA"),
            "OK" => (Severity::Success, "✅", "OK"),
            _ => (Severity::Info, "🔔", "ALARM?"),
        };
        let mut island = Island::new(
            &self.key(),
            Kind::Alert {
                from: self.previous.clone(),
                to: self.state.clone(),
            },
            &self.name,
        )
        .severity(severity)
        .icon(icon)
        .compact(compact)
        .revision(self.updated.clone().unwrap_or_else(|| self.state.clone()));
        if let Some(sub) = self.condition().or_else(|| self.description.clone()) {
            island = island.subtitle(sub);
        }
        if let Some(reason) = &self.reason {
            island = island.body(reason.clone());
        }
        match (account_label, &self.account) {
            (Some(label), Some(id)) => island = island.field("Account", format!("{label} · {id}")),
            (Some(label), None) => island = island.field("Account", label),
            (None, Some(id)) => island = island.field("Account", id.clone()),
            (None, None) => {}
        }
        if let Some(r) = &self.region {
            island = island.field("Region", r.clone());
        }
        if let Some(ns) = &self.namespace {
            island = island.field("Namespace", ns.clone());
        }
        if let Some(url) = self.console_url() {
            island = island.action(Action::open("Console ↗", &url).primary());
        }
        match self.state.as_str() {
            "ALARM" => island.sticky().action(Action::snooze(60)),
            "OK" => island.ttl(20),
            _ => island.ttl(120),
        }
    }
}

/// `*` matches any run of characters; everything else is literal.
fn glob(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

impl AlarmState {
    /// The first rule whose pattern matches this alarm's name.
    fn rule<'a>(&self, rules: &'a [AlarmRule]) -> Option<&'a AlarmRule> {
        self.rule_index(rules).map(|i| &rules[i])
    }

    /// Its position, for Preferences' "Which rule?".
    pub fn rule_index(&self, rules: &[AlarmRule]) -> Option<usize> {
        rules.iter().position(|r| glob(&r.pattern, &self.name))
    }

    /// The island after `alarm_rules`; `None` when a rule mutes the alarm.
    pub fn island_with(&self, account_label: Option<&str>, rules: &[AlarmRule]) -> Option<Island> {
        let mut island = self.island(account_label);
        let Some(rule) = self.rule(rules) else {
            return Some(island);
        };
        if rule.mute {
            return None;
        }
        if self.state == "ALARM" {
            if let Some(severity) = rule.severity {
                island.severity = severity;
                island.icon = Some(severity_icon(severity).into());
            }
            island.sticky = rule.sticky.unwrap_or(island.severity == Severity::Critical);
            if !island.sticky && island.ttl_secs.is_none() {
                island.ttl_secs = Some(300);
            }
        }
        island.silent = rule.silent;
        if let Some(url) = rule.runbook_url.as_deref() {
            // Keep Console first and Snooze last.
            let at = island.actions.len().min(1);
            island.actions.insert(at, Action::open("Runbook ↗", url));
        }
        Some(island)
    }
}

fn severity_icon(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "🚨",
        Severity::Warning => "⚠️",
        Severity::Info => "🔔",
        Severity::Success => "✅",
    }
}

#[derive(Deserialize)]
struct SecurityHubEvent {
    source: String,
    region: Option<String>,
    detail: SecurityHubDetail,
}

#[derive(Deserialize)]
struct SecurityHubDetail {
    #[serde(default)]
    findings: Vec<Finding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Finding {
    id: String,
    title: Option<String>,
    description: Option<String>,
    severity: Option<FindingSeverity>,
    product_name: Option<String>,
    aws_account_id: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    resources: Vec<FindingResource>,
    remediation: Option<FindingRemediation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindingSeverity {
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindingResource {
    id: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindingRemediation {
    recommendation: Option<FindingRecommendation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindingRecommendation {
    url: Option<String>,
}

/// Security Hub findings (GuardDuty, Inspector, Macie, Access Analyzer) from
/// an EventBridge event, raw or in an SNS envelope. Empty if it is not one.
pub fn findings_from_message(body: &str) -> Vec<Island> {
    if let Ok(env) = serde_json::from_str::<SnsEnvelope>(body)
        && env.kind == "Notification"
    {
        return findings_from_message(&env.message);
    }
    let Ok(ev) = serde_json::from_str::<SecurityHubEvent>(body) else {
        return Vec::new();
    };
    if ev.source != "aws.securityhub" {
        return Vec::new();
    }
    let region = ev.region;
    ev.detail
        .findings
        .into_iter()
        .map(|f| finding_island(f, region.as_deref()))
        .collect()
}

fn finding_island(f: Finding, region: Option<&str>) -> Island {
    let label = f
        .severity
        .and_then(|s| s.label)
        .unwrap_or_else(|| "UNKNOWN".into());
    let severity = match label.as_str() {
        "CRITICAL" | "HIGH" => Severity::Critical,
        "MEDIUM" => Severity::Warning,
        _ => Severity::Info,
    };
    let product = f.product_name.unwrap_or_else(|| "Security Hub".into());
    let mut island = Island::new(
        &format!("finding:{}", f.id),
        Kind::Alert {
            from: None,
            to: label.clone(),
        },
        f.title.as_deref().unwrap_or("Security finding"),
    )
    .severity(severity)
    .icon("🛡️")
    .compact(label)
    .subtitle(product)
    .revision(f.updated_at.unwrap_or_default());
    if let Some(d) = f.description {
        island = island.body(d);
    }
    if let Some(a) = f.aws_account_id {
        island = island.field("Account", a);
    }
    if let Some(r) = f.resources.first() {
        let value = match (&r.kind, &r.id) {
            (Some(k), Some(id)) => format!("{k} · {id}"),
            (Some(v), None) | (None, Some(v)) => v.clone(),
            (None, None) => String::new(),
        };
        if !value.is_empty() {
            island = island.field("Resource", value);
        }
    }
    if let Some(region) = region {
        let url = format!(
            "https://{region}.console.aws.amazon.com/securityhub/home?region={region}#/findings"
        );
        island = island.action(Action::open("Security Hub ↗", &url).primary());
    }
    if let Some(url) = f
        .remediation
        .and_then(|r| r.recommendation)
        .and_then(|r| r.url)
        .filter(|u| u.starts_with("https://"))
    {
        island = island.action(Action::open("Remediate ↗", &url));
    }
    if severity == Severity::Critical {
        island.sticky().action(Action::snooze(60))
    } else {
        island.ttl(300)
    }
}

fn find_cli() -> PathBuf {
    ["/opt/homebrew/bin/aws", "/usr/local/bin/aws"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("aws"))
}

/// Who an AWS call runs as. Built fresh for each call, so a rotated key is
/// picked up without a restart.
#[derive(Clone, Default)]
pub struct Creds {
    pub profile: Option<String>,
    pub region: Option<String>,
    /// (access key id, secret access key).
    access_key: Option<(String, String)>,
}

/// Never print the secret.
impl std::fmt::Debug for Creds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Creds")
            .field("profile", &self.profile)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key.as_ref().map(|(id, _)| id))
            .finish()
    }
}

impl Creds {
    /// For `auth`; an access key's secret comes from `secret` if given (typed
    /// in Preferences, not saved yet), else from the Keychain.
    pub fn resolve(
        auth: &AwsAuth,
        profile: Option<&str>,
        region: Option<&str>,
        secret: Option<&str>,
    ) -> Result<Self, String> {
        let access_key = match auth {
            AwsAuth::Profile => None,
            AwsAuth::AccessKey { access_key_id } => {
                let key = secret
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .or_else(|| secrets::get_aws_secret(access_key_id))
                    .ok_or_else(|| format!("No secret key for {access_key_id} in the Keychain"))?;
                Some((access_key_id.clone(), key))
            }
        };
        Ok(Self {
            // A profile would override the keys, so it is dropped with them.
            profile: profile
                .filter(|p| !p.is_empty() && access_key.is_none())
                .map(String::from),
            region: region.filter(|r| !r.is_empty()).map(String::from),
            access_key,
        })
    }
}

/// Where the profile and region settings live: `AWS_CONFIG_FILE` and
/// `AWS_SHARED_CREDENTIALS_FILE`, else ~/.aws/config and ~/.aws/credentials.
fn aws_file(var: &str, name: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".aws").join(name)))
}

/// The `key = value` lines of the INI section headed `[section]`.
fn ini_section(text: &str, section: &str) -> BTreeMap<String, String> {
    let mut inside = false;
    let mut out = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = header.split_whitespace().collect::<Vec<_>>().join(" ") == section;
        } else if inside
            && !line.starts_with(['#', ';'])
            && let Some((k, v)) = line.split_once('=')
        {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// A profile's settings: ~/.aws/config, then ~/.aws/credentials on top.
fn profile_settings(name: &str) -> BTreeMap<String, String> {
    let read = |var, file| {
        aws_file(var, file)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default()
    };
    merge_settings(
        &read("AWS_CONFIG_FILE", "config"),
        &read("AWS_SHARED_CREDENTIALS_FILE", "credentials"),
        name,
    )
}

/// `[profile x]` (`[default]`) in the config, `[x]` in the credentials file.
fn merge_settings(config: &str, credentials: &str, name: &str) -> BTreeMap<String, String> {
    let section = if name == "default" {
        "default".to_string()
    } else {
        format!("profile {name}")
    };
    let mut settings = ini_section(config, &section);
    settings.extend(ini_section(credentials, name));
    settings
}

/// The JSON a `credential_process` prints, and so does
/// `aws configure export-credentials --format process`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ProcessCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    expiration: Option<String>,
}

/// Keys and when they expire (`None`: they don't).
type Expiring = (Keys, Option<DateTime<Utc>>);

/// Temporary keys by profile, until a few minutes before they expire.
static TEMPORARY: Mutex<BTreeMap<String, Expiring>> = Mutex::new(BTreeMap::new());

/// Login items start with a bare PATH, so the usual install locations are
/// added for the CLI and credential helpers.
fn run(mut cmd: Command, what: &str) -> Result<String, String> {
    let path = format!(
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{}",
        std::env::var("PATH").unwrap_or_default()
    );
    let out = cmd
        .env("PATH", path)
        .env("AWS_PAGER", "")
        .output()
        .map_err(|e| format!("{what}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed");
        return Err(line.trim().to_string());
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// Keys for a profile. Static keys and `credential_process` are handled
/// here; anything else (SSO, an assumed role) is handed to the AWS CLI, which
/// knows how to refresh them.
fn profile_keys(binary: Option<&str>, name: &str) -> Result<Keys, String> {
    let settings = profile_settings(name);
    if let (Some(id), Some(secret)) = (
        settings.get("aws_access_key_id"),
        settings.get("aws_secret_access_key"),
    ) {
        return Ok(Keys {
            id: id.clone(),
            secret: secret.clone(),
            token: settings.get("aws_session_token").cloned(),
        });
    }
    let mut cache = TEMPORARY.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = |expires: &Option<DateTime<Utc>>| {
        expires.is_none_or(|t| t - chrono::Duration::minutes(5) > Utc::now())
    };
    if let Some((keys, expires)) = cache.get(name)
        && fresh(expires)
    {
        return Ok(keys.clone());
    }
    let json = if let Some(process) = settings.get("credential_process") {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", process]);
        run(cmd, "credential_process")?
    } else {
        let bin = binary.map(PathBuf::from).unwrap_or_else(find_cli);
        let mut cmd = Command::new(&bin);
        cmd.args([
            "configure",
            "export-credentials",
            "--profile",
            name,
            "--format",
            "process",
        ]);
        run(cmd, &bin.display().to_string()).map_err(|e| {
            if e.contains("No such file") {
                format!(
                    "profile {name} has no keys of its own (SSO or a role?), which needs the \
                     AWS CLI: install it, or use an access key instead"
                )
            } else {
                e
            }
        })?
    };
    let c: ProcessCredentials =
        serde_json::from_str(&json).map_err(|e| format!("credentials for {name}: {e}"))?;
    let keys = Keys {
        id: c.access_key_id,
        secret: c.secret_access_key,
        token: c.session_token.filter(|t| !t.is_empty()),
    };
    let expires = c
        .expiration
        .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.with_timezone(&Utc));
    cache.insert(name.to_string(), (keys.clone(), expires));
    Ok(keys)
}

impl Creds {
    fn profile_name(&self) -> String {
        self.profile
            .clone()
            .or_else(|| std::env::var("AWS_PROFILE").ok())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "default".into())
    }

    fn keys(&self, binary: Option<&str>) -> Result<Keys, String> {
        match &self.access_key {
            Some((id, secret)) => Ok(Keys {
                id: id.clone(),
                secret: secret.clone(),
                token: None,
            }),
            None => profile_keys(binary, &self.profile_name()),
        }
    }

    /// The account's region, else the profile's, else `AWS_REGION`.
    fn region(&self) -> Result<String, String> {
        self.region
            .clone()
            .or_else(|| {
                self.access_key
                    .is_none()
                    .then(|| profile_settings(&self.profile_name()).remove("region"))
                    .flatten()
            })
            .or_else(|| std::env::var("AWS_REGION").ok())
            .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
            .filter(|r| !r.is_empty())
            .ok_or_else(|| "no region: set one for this account".to_string())
    }

    /// The same, with the region read from an SQS queue URL if none is set
    /// (`https://sqs.eu-west-2.amazonaws.com/…`).
    pub fn with_region_from(mut self, url: &str) -> Self {
        if self.region.is_none() {
            self.region = queue_region(url);
        }
        self
    }
}

fn queue_region(url: &str) -> Option<String> {
    let host = url.strip_prefix("https://")?.split('/').next()?;
    let mut labels = host.split('.');
    let first = labels.next()?;
    // sqs.<region>.amazonaws.com, or the legacy <region>.queue.amazonaws.com.
    let region = if first == "sqs" {
        labels.next()?
    } else {
        first
    };
    region.contains('-').then(|| region.to_string())
}

/// An AWS service spoken to over its JSON protocol.
#[derive(Debug, Clone, Copy)]
pub enum Service {
    CloudWatch,
    Sqs,
}

impl Service {
    /// (endpoint prefix and signing name, X-Amz-Target prefix)
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Service::CloudWatch => ("monitoring", "GraniteServiceVersion20100801"),
            Service::Sqs => ("sqs", "AmazonSQS"),
        }
    }
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            // Past SQS's longest long poll (20s).
            .timeout_global(Some(Duration::from_secs(40)))
            .http_status_as_error(false)
            .build()
            .into()
    })
}

/// "AccessDeniedException: User … is not authorized" from an error body.
fn error_message(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    let code = v["__type"]
        .as_str()
        .or(v["code"].as_str())
        .map(|t| t.rsplit('#').next().unwrap_or(t));
    let message = v["message"].as_str().or(v["Message"].as_str());
    match (code, message) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (Some(c), None) => c.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => format!("AWS answered {status}"),
    }
}

/// One signed call, e.g. `call(None, &creds, Service::Sqs, "ReceiveMessage",
/// &json!({...}))`, returning the JSON answer. `binary` is the AWS CLI, used
/// only to fetch keys for an SSO or role profile.
pub fn call(
    binary: Option<&str>,
    creds: &Creds,
    service: Service,
    action: &str,
    body: &Value,
) -> Result<Value, String> {
    let (prefix, target) = service.names();
    let region = creds.region()?;
    let keys = creds.keys(binary)?;
    let domain = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    let host = format!("{prefix}.{region}.{domain}");
    let body = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let target = format!("{target}.{action}");
    let headers = [
        ("content-type", "application/x-amz-json-1.0"),
        ("x-amz-target", target.as_str()),
    ];
    let req = sigv4::Request {
        method: "POST",
        host: &host,
        path: "/",
        query: "",
        headers: &headers,
        body: &body,
    };
    let mut request = agent().post(format!("https://{host}/"));
    for (k, v) in headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .chain(sigv4::sign(&req, &keys, &region, prefix, Utc::now()))
    {
        request = request.header(&k, &v);
    }
    let mut response = request
        .send(&body[..])
        .map_err(|e| format!("{host}: {e}"))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{host}: {e}"))?;
    if status != 200 {
        return Err(error_message(status, &text));
    }
    // SQS answers an empty receive with `{}`; never with nothing, but be kind.
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(&text).map_err(|e| format!("{action}: {e}"))
}

/// Every alarm (metric and composite, any state), optionally under a name
/// prefix, following `NextToken` across pages.
pub fn describe_alarms(
    binary: Option<&str>,
    creds: &Creds,
    prefix: Option<&str>,
) -> Result<Vec<AlarmState>, String> {
    let mut alarms = Vec::new();
    let mut next: Option<String> = None;
    // 100 a page, so this is 10,000 alarms: plenty, and never a runaway.
    for _ in 0..100 {
        let mut body = json!({"AlarmTypes": ["MetricAlarm", "CompositeAlarm"], "MaxRecords": 100});
        if let Some(p) = prefix.filter(|p| !p.is_empty()) {
            body["AlarmNamePrefix"] = p.into();
        }
        if let Some(token) = next.take() {
            body["NextToken"] = token.into();
        }
        let page: DescribeAlarms = serde_json::from_value(call(
            binary,
            creds,
            Service::CloudWatch,
            "DescribeAlarms",
            &body,
        )?)
        .map_err(|e| format!("DescribeAlarms: {e}"))?;
        next = page.next_token.clone().filter(|t| !t.is_empty());
        alarms.extend(alarm_states(page));
        if next.is_none() {
            break;
        }
    }
    Ok(alarms)
}

/// A profile in ~/.aws/config, for the picker in Preferences.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    pub name: String,
    /// Uses SSO, whose sessions expire within hours: worth a warning.
    pub sso: bool,
    pub region: Option<String>,
}

/// Profiles from ~/.aws/config (`AWS_CONFIG_FILE` if set), without running
/// the CLI.
pub fn profiles() -> Vec<Profile> {
    let path = std::env::var_os("AWS_CONFIG_FILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".aws/config")));
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| parse_profiles(&text))
        .unwrap_or_default()
}

fn parse_profiles(text: &str) -> Vec<Profile> {
    let mut out: Vec<Profile> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = match header.trim() {
                "default" => Some("default"),
                h => h.strip_prefix("profile ").map(str::trim),
            };
            if let Some(name) = name {
                out.push(Profile {
                    name: name.to_string(),
                    sso: false,
                    region: None,
                });
            } else {
                // An [sso-session …] or other section: not a profile.
                out.push(Profile {
                    name: String::new(),
                    sso: false,
                    region: None,
                });
            }
            continue;
        }
        let (Some(current), Some((k, v))) = (out.last_mut(), line.split_once('=')) else {
            continue;
        };
        match k.trim() {
            "sso_session" | "sso_start_url" => current.sso = true,
            "region" => current.region = Some(v.trim().to_string()),
            _ => {}
        }
    }
    out.retain(|p| !p.name.is_empty());
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One alarm found by Discover alarms.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Discovered {
    pub name: String,
    pub state: String,
    pub metric: Option<String>,
    pub description: Option<String>,
}

/// Every alarm (any state) in the account, optionally under a prefix.
pub fn discover_alarms(
    binary: Option<&str>,
    creds: &Creds,
    prefix: Option<&str>,
) -> Result<Vec<Discovered>, String> {
    let mut found: Vec<Discovered> = describe_alarms(binary, creds, prefix)?
        .into_iter()
        .map(|a| Discovered {
            metric: a.condition(),
            name: a.name,
            state: a.state,
            description: a.description,
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIBE: &str = r#"{"MetricAlarms":[{"AlarmName":"prod api 5xx","AlarmArn":"arn:aws:cloudwatch:eu-west-2:123456789012:alarm:prod api 5xx","StateValue":"ALARM","StateReason":"Threshold Crossed: 3 datapoints","StateUpdatedTimestamp":"2026-10-30T10:00:00Z","MetricName":"HTTPCode_Target_5XX_Count","Namespace":"AWS/ApplicationELB","ComparisonOperator":"GreaterThanThreshold","Threshold":50.0}],"CompositeAlarms":[]}"#;

    /// Against real AWS: `HOUSTON_LIVE_PROFILE=<profile> cargo test live -- --ignored`.
    #[test]
    #[ignore]
    fn live_describe_alarms() {
        let profile = std::env::var("HOUSTON_LIVE_PROFILE").expect("HOUSTON_LIVE_PROFILE");
        let region = std::env::var("HOUSTON_LIVE_REGION").ok();
        let c = Creds::resolve(&AwsAuth::Profile, Some(&profile), region.as_deref(), None).unwrap();
        let alarms = describe_alarms(None, &c, None).unwrap();
        let firing = alarms.iter().filter(|a| a.state == "ALARM").count();
        println!("{profile}: {} alarms, {firing} in ALARM", alarms.len());
        assert!(alarms.iter().all(|a| a.updated.is_some()));
    }

    #[test]
    fn json_protocol_pages() {
        let page = r#"{"MetricAlarms":[{"AlarmName":"a","StateValue":"OK","StateUpdatedTimestamp":1.7617944001234E9}],"NextToken":"n2"}"#;
        let d: DescribeAlarms = serde_json::from_str(page).unwrap();
        assert_eq!(d.next_token.as_deref(), Some("n2"));
        let a = &alarm_states(d)[0];
        // The CLI's form, so revisions match what older builds saw.
        assert_eq!(
            a.updated.as_deref(),
            Some("2025-10-30T03:20:00.123400+00:00")
        );
    }

    #[test]
    fn profiles_and_credentials_files() {
        let config = "[default]\nregion = us-east-1\n\n[profile  ops ]\nregion=eu-west-2\n# aws_access_key_id = commented\n[sso-session ops]\nsso_region = eu-west-1\n";
        let credentials = "[ops]\naws_access_key_id = AKIAOPS\naws_secret_access_key = s3cret\n[default]\naws_access_key_id = AKIADEF\n";
        let ops = merge_settings(config, credentials, "ops");
        assert_eq!(ops.get("region").map(String::as_str), Some("eu-west-2"));
        assert_eq!(
            ops.get("aws_access_key_id").map(String::as_str),
            Some("AKIAOPS")
        );
        assert!(!ops.contains_key("sso_region"), "{ops:?}");
        let default = merge_settings(config, credentials, "default");
        assert_eq!(default.get("region").map(String::as_str), Some("us-east-1"));
        assert!(merge_settings(config, credentials, "nope").is_empty());
    }

    #[test]
    fn queue_regions() {
        let r = |u| queue_region(u);
        assert_eq!(
            r("https://sqs.eu-west-2.amazonaws.com/123456789012/q").as_deref(),
            Some("eu-west-2")
        );
        assert_eq!(
            r("https://us-east-1.queue.amazonaws.com/1/q").as_deref(),
            Some("us-east-1")
        );
        assert_eq!(r("http://localhost:9324/q"), None);
    }

    #[test]
    fn aws_error_bodies() {
        assert_eq!(
            error_message(
                400,
                r#"{"__type":"com.amazonaws.cloudwatch#AccessDenied","message":"no"}"#
            ),
            "AccessDenied: no"
        );
        assert_eq!(
            error_message(
                400,
                r#"{"__type":"InvalidClientTokenId","Message":"bad key"}"#
            ),
            "InvalidClientTokenId: bad key"
        );
        assert_eq!(error_message(503, "<html>"), "AWS answered 503");
    }

    #[test]
    fn regions_come_from_the_account_first() {
        let c = Creds::resolve(
            &AwsAuth::AccessKey {
                access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            },
            None,
            Some("eu-west-2"),
            Some("secret"),
        )
        .unwrap();
        assert_eq!(c.region().unwrap(), "eu-west-2");
        let keys = c.keys(None).unwrap();
        assert_eq!(
            (keys.id.as_str(), keys.token),
            ("AKIAIOSFODNN7EXAMPLE", None)
        );
        let sqs = Creds::default().with_region_from("https://sqs.ap-south-1.amazonaws.com/1/q");
        assert_eq!(sqs.region.as_deref(), Some("ap-south-1"));
    }

    #[test]
    fn describe_alarms_rows() {
        let a = &parse_describe_alarms(DESCRIBE).unwrap()[0];
        assert_eq!(a.region.as_deref(), Some("eu-west-2"));
        assert_eq!(a.account.as_deref(), Some("123456789012"));
        let i = a.island(Some("prod"));
        assert_eq!(i.severity, Severity::Critical);
        assert!(i.sticky);
        assert_eq!(
            i.subtitle.as_deref(),
            Some("HTTPCode_Target_5XX_Count > 50")
        );
        assert_eq!(i.revision.as_deref(), Some("2026-10-30T10:00:00Z"));
        assert_eq!(i.fields[0].value, "prod · 123456789012");
        let url = i.actions[0].url.as_deref().unwrap();
        assert!(url.ends_with("#alarmsV2:alarm/prod%20api%205xx"), "{url}");
    }

    #[test]
    fn eventbridge_event() {
        let ev = r#"{"version":"0","detail-type":"CloudWatch Alarm State Change","source":"aws.cloudwatch","account":"111122223333","region":"us-east-1","detail":{"alarmName":"db cpu","state":{"value":"OK","reason":"back to normal","timestamp":"t2"},"previousState":{"value":"ALARM"},"configuration":{"description":"RDS CPU","metrics":[{"id":"m1","metricStat":{"metric":{"namespace":"AWS/RDS","name":"CPUUtilization"}}}]}}}"#;
        let a = alarm_from_message(ev).unwrap();
        assert_eq!(a.state, "OK");
        assert_eq!(a.previous.as_deref(), Some("ALARM"));
        assert_eq!(a.metric.as_deref(), Some("CPUUtilization"));
        let i = a.island(None);
        assert_eq!(i.severity, Severity::Success);
        assert!(!i.sticky);
    }

    #[test]
    fn sns_envelope_and_raw_message() {
        let raw = r#"{"AlarmName":"q depth","AWSAccountId":"999","NewStateValue":"ALARM","OldStateValue":"OK","NewStateReason":"r","StateChangeTime":"t","AlarmArn":"arn:aws:cloudwatch:eu-west-1:999:alarm:q depth","Trigger":{"MetricName":"ApproximateNumberOfMessagesVisible","Namespace":"AWS/SQS","ComparisonOperator":"GreaterThanOrEqualToThreshold","Threshold":1000}}"#;
        let env = serde_json::json!({"Type":"Notification","Message": raw}).to_string();
        let a = alarm_from_message(&env).unwrap();
        assert_eq!(a.region.as_deref(), Some("eu-west-1"));
        assert_eq!(a.account.as_deref(), Some("999"));
        assert_eq!(
            a.island(None).subtitle.as_deref(),
            Some("ApproximateNumberOfMessagesVisible ≥ 1000")
        );
        assert!(alarm_from_message(r#"{"hello":"world"}"#).is_none());
    }

    fn rule(pattern: &str) -> AlarmRule {
        AlarmRule {
            pattern: pattern.into(),
            severity: None,
            sticky: None,
            silent: false,
            mute: false,
            runbook_url: None,
        }
    }

    #[test]
    fn globs() {
        assert!(glob("apollo-prod-*", "apollo-prod-routing-errors"));
        assert!(glob(
            "*-dlq-not-empty",
            "apollo-prod-call-ended-dlq-not-empty"
        ));
        assert!(glob(
            "apollo-prod-*-heartbeat-*",
            "apollo-prod-ivr-heartbeat-missing"
        ));
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exactly"));
        assert!(!glob("apollo-prod-*", "apollo-staging-x"));
        assert!(!glob("a*b*c", "a-c-b"));
    }

    #[test]
    fn rules_reshape_alarms() {
        let a = &parse_describe_alarms(DESCRIBE).unwrap()[0];
        let quiet = AlarmRule {
            severity: Some(Severity::Info),
            runbook_url: Some("https://example.com/rb".into()),
            ..rule("prod api *")
        };
        let i = a.island_with(None, &[quiet]).unwrap();
        assert_eq!(i.severity, Severity::Info);
        assert!(!i.sticky);
        assert_eq!(i.ttl_secs, Some(300));
        let labels: Vec<&str> = i.actions.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Console ↗", "Runbook ↗", "Snooze 1h"]);

        let muted = AlarmRule {
            mute: true,
            ..rule("*5xx")
        };
        assert!(a.island_with(None, &[muted]).is_none());
        // No match keeps the defaults.
        let i = a.island_with(None, &[rule("other-*")]).unwrap();
        assert_eq!(i.severity, Severity::Critical);
        assert!(i.sticky);
    }

    #[test]
    fn security_hub_findings() {
        let ev = serde_json::json!({
            "source": "aws.securityhub",
            "detail-type": "Security Hub Findings - Imported",
            "region": "eu-west-2",
            "detail": {"findings": [{
                "Id": "arn:aws:guardduty:eu-west-2:1:detector/d/finding/f",
                "Title": "Unusual API call",
                "Severity": {"Label": "HIGH"},
                "ProductName": "GuardDuty",
                "AwsAccountId": "1",
                "UpdatedAt": "2026-10-30T10:00:00Z",
                "Resources": [{"Type": "AwsIamAccessKey", "Id": "AKIA..."}]
            }]}
        })
        .to_string();
        let env = serde_json::json!({"Type": "Notification", "Message": ev}).to_string();
        let islands = findings_from_message(&env);
        assert_eq!(islands.len(), 1);
        let i = &islands[0];
        assert_eq!(i.severity, Severity::Critical);
        assert!(i.sticky);
        assert_eq!(i.subtitle.as_deref(), Some("GuardDuty"));
        assert_eq!(i.fields[1].value, "AwsIamAccessKey · AKIA...");
        // An alarm envelope is not a finding, and a finding is not an alarm.
        assert!(findings_from_message(DESCRIBE).is_empty());
        assert!(alarm_from_message(&env).is_none());
    }

    #[test]
    fn profiles_from_config() {
        let text = "[default]\nregion = eu-west-1\n\n[profile apollo-voip]\nsso_session = arrow\nregion=eu-west-2\n\n[sso-session arrow]\nsso_start_url = https://x.awsapps.com/start\n\n[profile ci]\ncredential_process = /bin/true\n";
        let p = parse_profiles(text);
        let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["apollo-voip", "ci", "default"]);
        assert!(p[0].sso && !p[1].sso);
        assert_eq!(p[0].region.as_deref(), Some("eu-west-2"));
    }

    #[test]
    fn access_keys_drop_the_profile() {
        let auth = AwsAuth::AccessKey {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
        };
        let c = Creds::resolve(
            &auth,
            Some("apollo-prod"),
            Some("eu-west-2"),
            Some("s3cr3t"),
        )
        .unwrap();
        assert!(c.access_key.is_some());
        assert_eq!(c.profile, None);
        assert!(
            !format!("{c:?}").contains("s3cr3t"),
            "Debug hides the secret"
        );
        let p = Creds::resolve(&AwsAuth::Profile, Some("apollo-prod"), None, None).unwrap();
        assert_eq!(p.profile.as_deref(), Some("apollo-prod"));
    }
}
