//! Built-in sources. Each is a [`Source`]; add new ones here.

mod cloudwatch;
mod demo;
mod iot;
mod local_api;
mod sqs;

use crate::config::Config;
use crate::source::Source;

pub use demo::DEMO_ID;
pub use demo::TRIGGERS as DEMO_TRIGGERS;

pub fn from_config(config: &Config) -> Vec<Box<dyn Source>> {
    let mut sources: Vec<Box<dyn Source>> = vec![Box::new(demo::Demo::new(config.demo.autoplay))];
    if config.api.enabled {
        sources.push(Box::new(local_api::LocalApi::new(
            config.api.port,
            config.api.token.clone(),
            config.alarm_rules.clone(),
        )));
    }
    if config.cloudwatch.enabled && !config.cloudwatch.accounts.is_empty() {
        sources.push(Box::new(cloudwatch::CloudWatch::new(
            config.cloudwatch.clone(),
            config.alarm_rules.clone(),
            config.aws_cli.clone(),
        )));
    }
    if config.sqs.enabled && !config.sqs.queue_url.is_empty() {
        sources.push(Box::new(sqs::Sqs::new(
            config.sqs.clone(),
            config.alarm_rules.clone(),
            config.aws_cli.clone(),
        )));
    }
    if config.iot.enabled {
        sources.push(Box::new(iot::Iot::new(config.iot.clone())));
    }
    sources
}
