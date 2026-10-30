//! Everything that knows about AWS: running the `aws` CLI (so CLI profiles and
//! credential processes just work) and turning CloudWatch alarm payloads into
//! islands. Three payload shapes are understood:
//!
//! - rows from `aws cloudwatch describe-alarms`
//! - EventBridge `CloudWatch Alarm State Change` events
//! - SNS alarm notifications, raw or inside an SNS envelope
//!
//! It also resolves credentials (a CLI profile, or an IAM access key from the
//! Keychain), lists local profiles and discovers alarms for Preferences.
//!
//! It also maps Security Hub findings (EventBridge `Security Hub Findings -
//! Imported`, raw or inside an SNS envelope) and applies the `alarm_rules`
//! from the config.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{AlarmRule, AwsAuth};
use crate::island::{Action, Island, Kind, Severity};
use crate::secrets;

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
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DescribedAlarm {
    alarm_name: String,
    alarm_arn: Option<String>,
    alarm_description: Option<String>,
    state_value: String,
    state_reason: Option<String>,
    state_updated_timestamp: Option<String>,
    metric_name: Option<String>,
    namespace: Option<String>,
    comparison_operator: Option<String>,
    threshold: Option<f64>,
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

/// Parse `describe-alarms --output json`.
pub fn parse_describe_alarms(json: &str) -> Result<Vec<AlarmState>, String> {
    let d: DescribeAlarms =
        serde_json::from_str(json).map_err(|e| format!("describe-alarms: {e}"))?;
    Ok(d.metric_alarms
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
        .collect())
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

/// Run the AWS CLI as `creds` and return stdout. Access keys go in the
/// environment, never on the command line where `ps` would show them. Login
/// items start with a bare PATH, so the usual install locations are added for
/// credential helpers.
pub fn cli(binary: Option<&str>, args: &[&str], creds: &Creds) -> Result<String, String> {
    let bin = binary.map(PathBuf::from).unwrap_or_else(find_cli);
    let path = format!(
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{}",
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = Command::new(&bin);
    cmd.args(args).env("PATH", path).env("AWS_PAGER", "");
    if let Some(p) = &creds.profile {
        cmd.args(["--profile", p]);
    }
    if let Some(r) = &creds.region {
        cmd.args(["--region", r]);
    }
    if let Some((id, secret)) = &creds.access_key {
        cmd.env("AWS_ACCESS_KEY_ID", id)
            .env("AWS_SECRET_ACCESS_KEY", secret)
            .env_remove("AWS_SESSION_TOKEN")
            .env_remove("AWS_PROFILE");
    }
    let out = cmd
        .output()
        .map_err(|e| format!("{}: {e}", display(&bin)))?;
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

fn display(p: &Path) -> String {
    p.display().to_string()
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
    let mut args = vec![
        "cloudwatch",
        "describe-alarms",
        "--alarm-types",
        "MetricAlarm",
        "CompositeAlarm",
        "--output",
        "json",
    ];
    if let Some(p) = prefix.filter(|p| !p.is_empty()) {
        args.extend(["--alarm-name-prefix", p]);
    }
    let out = cli(binary, &args, creds)?;
    let mut found: Vec<Discovered> = parse_describe_alarms(&out)?
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
