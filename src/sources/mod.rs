//! Built-in sources. Each is a [`Source`]; add new ones to [`build`],
//! [`fingerprint`] and [`RESTARTABLE`].

mod cloudwatch;
mod demo;
mod iot;
mod linear;
mod local_api;
mod sqs;

use serde_json::json;

use crate::config::Config;
use crate::source::Source;

pub use cloudwatch::{discover as discover_alarms, probe as probe_cloudwatch};
pub use demo::DEMO_ID;
pub use demo::TRIGGERS as DEMO_TRIGGERS;
pub use linear::probe as probe_linear;

/// Sources that Preferences can restart. The demo keeps running: its only
/// setting takes effect at the next launch.
pub const RESTARTABLE: &[&str] = &["api", "cloudwatch", "sqs", "linear", "iot"];

/// Sources that apply alarm rules sent as `Inbound::Rules`.
pub const LIVE_RULES: &[&str] = &["api"];

/// The source called `id` for this configuration, or `None` if it is off.
pub fn build(id: &str, config: &Config) -> Option<Box<dyn Source>> {
    let rules = || config.alarm_rules.clone();
    let cli = || config.aws_cli.clone();
    match id {
        "api" if config.api.enabled => Some(Box::new(local_api::LocalApi::new(
            config.api.port,
            config.api.token.clone(),
            rules(),
        ))),
        "cloudwatch" if config.cloudwatch.enabled && !config.cloudwatch.accounts.is_empty() => {
            Some(Box::new(cloudwatch::CloudWatch::new(
                config.cloudwatch.clone(),
                rules(),
                cli(),
            )))
        }
        "sqs" if config.sqs.enabled && !config.sqs.queue_url.is_empty() => {
            Some(Box::new(sqs::Sqs::new(config.sqs.clone(), rules(), cli())))
        }
        "linear" if config.linear.enabled => {
            Some(Box::new(linear::Linear::new(config.linear.clone())))
        }
        "iot" if config.iot.enabled => Some(Box::new(iot::Iot::new(config.iot.clone()))),
        _ => None,
    }
}

/// Everything source `id` reads from the config. When it changes, the source
/// is restarted. The local API takes new alarm rules live instead (see
/// [`LIVE_RULES`]), so changing them doesn't drop its connections.
pub fn fingerprint(id: &str, config: &Config) -> String {
    let value = match id {
        "api" => json!(config.api),
        "cloudwatch" => json!([config.cloudwatch, config.alarm_rules, config.aws_cli]),
        "sqs" => json!([config.sqs, config.alarm_rules, config.aws_cli]),
        "linear" => json!(config.linear),
        "iot" => json!(config.iot),
        _ => json!(null),
    };
    value.to_string()
}

pub fn from_config(config: &Config) -> Vec<Box<dyn Source>> {
    let mut sources: Vec<Box<dyn Source>> = vec![Box::new(demo::Demo::new(config.demo.autoplay))];
    sources.extend(RESTARTABLE.iter().filter_map(|id| build(id, config)));
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_sources_restart() {
        let a = Config::default();
        let mut b = a.clone();
        b.linear.teams.push("APO".into());
        let changed: Vec<&str> = RESTARTABLE
            .iter()
            .copied()
            .filter(|id| fingerprint(id, &a) != fingerprint(id, &b))
            .collect();
        assert_eq!(changed, ["linear"]);

        let mut c = a.clone();
        c.alarm_rules
            .push(serde_json::from_str(r#"{"pattern":"x"}"#).unwrap());
        let changed: Vec<&str> = RESTARTABLE
            .iter()
            .copied()
            .filter(|id| fingerprint(id, &a) != fingerprint(id, &c))
            .collect();
        assert_eq!(changed, ["cloudwatch", "sqs"], "the API takes rules live");
    }

    #[test]
    fn built_ids_match() {
        let mut c = Config::default();
        c.linear.enabled = true;
        c.iot.enabled = true;
        for id in ["api", "linear", "iot"] {
            assert_eq!(build(id, &c).map(|s| s.id()).as_deref(), Some(id));
        }
        assert!(build("cloudwatch", &c).is_none(), "off by default");
    }
}
