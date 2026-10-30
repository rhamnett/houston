//! Long-polls an SQS queue. Point an EventBridge rule for `CloudWatch Alarm
//! State Change` (from any account, via an event bus) or an SNS alarm topic at
//! the queue. A message body can also be an island JSON, so anything that can
//! write to SQS can post. Security Hub findings (the `security-incidents`
//! style of topic) are understood too. Messages are deleted once shown.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use serde::Deserialize;

use crate::aws;
use crate::config::{AlarmRule, SqsConfig};
use crate::island::Island;
use crate::source::{Inbound, Sink, Source};

const RETRY_AFTER: Duration = Duration::from_secs(30);

pub struct Sqs {
    config: SqsConfig,
    rules: Vec<AlarmRule>,
    cli: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Received {
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Message {
    body: String,
    receipt_handle: String,
}

/// What a queue message turns into. `None` if it is not understood; an empty
/// list if it was understood but muted by a rule.
fn islands_from_body(body: &str, rules: &[AlarmRule]) -> Option<Vec<Island>> {
    if let Some(alarm) = aws::alarm_from_message(body) {
        return Some(alarm.island_with(None, rules).into_iter().collect());
    }
    let findings = aws::findings_from_message(body);
    if !findings.is_empty() {
        return Some(findings);
    }
    let island = serde_json::from_str::<Island>(body)
        .ok()?
        .validated()
        .ok()?;
    Some(vec![island])
}

impl Sqs {
    pub fn new(config: SqsConfig, rules: Vec<AlarmRule>, cli: Option<String>) -> Self {
        Self { config, rules, cli }
    }

    fn creds(&self) -> Result<aws::Creds, String> {
        aws::Creds::resolve(
            &self.config.auth,
            self.config.profile.as_deref(),
            self.config.region.as_deref(),
            None,
        )
    }

    fn receive(&self) -> Result<Vec<Message>, String> {
        let wait = self.config.wait_secs.clamp(1, 20).to_string();
        let args = vec![
            "sqs",
            "receive-message",
            "--queue-url",
            &self.config.queue_url,
            "--max-number-of-messages",
            "10",
            "--wait-time-seconds",
            &wait,
            "--output",
            "json",
        ];
        let out = aws::cli(self.cli.as_deref(), &args, &self.creds()?)?;
        // The CLI prints nothing when the queue was empty.
        if out.trim().is_empty() {
            return Ok(Vec::new());
        }
        let r: Received =
            serde_json::from_str(&out).map_err(|e| format!("receive-message: {e}"))?;
        Ok(r.messages)
    }

    fn delete(&self, receipt: &str) -> Result<(), String> {
        let args = vec![
            "sqs",
            "delete-message",
            "--queue-url",
            &self.config.queue_url,
            "--receipt-handle",
            receipt,
        ];
        aws::cli(self.cli.as_deref(), &args, &self.creds()?).map(|_| ())
    }
}

impl Source for Sqs {
    fn id(&self) -> String {
        "sqs".into()
    }

    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>) {
        sink.status(format!("sqs: listening on {}", self.config.queue_url));
        loop {
            match inbox.try_recv() {
                Ok(_) | Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return,
            }
            match self.receive() {
                Ok(messages) => {
                    for m in messages {
                        match islands_from_body(&m.body, &self.rules) {
                            Some(islands) => islands.into_iter().for_each(|i| sink.upsert(i)),
                            None => crate::log::line(&format!(
                                "sqs: skipped a message that is not an alarm, finding or island: {:.120}",
                                m.body
                            )),
                        }
                        if let Err(e) = self.delete(&m.receipt_handle) {
                            crate::log::line(&format!("sqs delete: {e}"));
                        }
                    }
                }
                Err(e) => {
                    sink.status(format!("sqs: {e}"));
                    std::thread::sleep(RETRY_AFTER);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_become_islands() {
        let alarm = r#"{"AlarmName":"x","NewStateValue":"ALARM","AlarmArn":"arn:aws:cloudwatch:eu-west-2:1:alarm:x"}"#;
        assert_eq!(
            islands_from_body(alarm, &[]).unwrap()[0].key,
            "alarm:1:eu-west-2:x"
        );
        let custom = r#"{"key":"build-42","title":"Build failed"}"#;
        assert_eq!(
            islands_from_body(custom, &[]).unwrap()[0].title,
            "Build failed"
        );
        assert!(islands_from_body("not json", &[]).is_none());
        let mute = AlarmRule {
            pattern: "x".into(),
            severity: None,
            sticky: None,
            silent: false,
            mute: true,
            runbook_url: None,
        };
        assert!(islands_from_body(alarm, &[mute]).unwrap().is_empty());
    }
}
