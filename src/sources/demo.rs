//! Sample islands of every kind, played as a tour at launch or one at a time
//! from the menu. The alarm goes through the real CloudWatch mapping, and the
//! Acknowledge buttons show an action coming back to the source.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::aws::AlarmState;
use crate::island::{Action, Island, Kind, Severity};
use crate::source::{Inbound, Sink, Source};

pub const DEMO_ID: &str = "demo";

/// Menu triggers: (id, label).
pub const TRIGGERS: &[(&str, &str)] = &[
    ("tour", "Full drill"),
    ("alarm", "CloudWatch alarm"),
    ("resolve", "Alarm back to OK"),
    ("deploy", "Deploy progress"),
    ("metric", "Live metric"),
    ("message", "Message"),
    ("incident", "Incident timer"),
];

const ALARM_KEY_ACCOUNT: &str = "123456789012";
const DEPLOY_STEPS: u32 = 40;
const METRIC_STEPS: u32 = 14;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Alarm,
    Resolve,
    Deploy(u32),
    Metric(u32),
    Message,
    Incident,
}

pub struct Demo {
    autoplay: bool,
    queue: Vec<(Instant, Step)>,
    series: Vec<f64>,
    seed: u64,
    acked_alarm: bool,
    incident_started: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn sample_alarm(state: &str, previous: &str) -> AlarmState {
    AlarmState {
        name: "prod-api-5xx-errors".into(),
        description: Some("Target 5XX responses on the prod API load balancer".into()),
        state: state.into(),
        previous: Some(previous.into()),
        reason: Some(if state == "ALARM" {
            "Threshold Crossed: 3 out of the last 3 datapoints [212.0, 187.0, 96.0] were greater than the threshold (50.0).".into()
        } else {
            "Threshold Crossed: 1 datapoint [4.0] was not greater than the threshold (50.0).".into()
        }),
        updated: Some(format!("demo-{state}")),
        region: Some("eu-west-2".into()),
        account: Some(ALARM_KEY_ACCOUNT.into()),
        namespace: Some("AWS/ApplicationELB".into()),
        metric: Some("HTTPCode_Target_5XX_Count".into()),
        comparison: Some("GreaterThanThreshold".into()),
        threshold: Some(50.0),
    }
}

impl Demo {
    pub fn new(autoplay: bool) -> Self {
        Self {
            autoplay,
            queue: Vec::new(),
            series: Vec::new(),
            seed: now_ms() as u64 | 1,
            acked_alarm: false,
            incident_started: 0,
        }
    }

    fn after(&mut self, secs: f64, step: Step) {
        self.queue
            .push((Instant::now() + Duration::from_secs_f64(secs), step));
    }

    /// A number in 0..1 from a small xorshift.
    fn random(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed % 10_000) as f64 / 10_000.0
    }

    fn trigger(&mut self, name: &str) {
        match name {
            "tour" => {
                self.after(1.0, Step::Alarm);
                self.after(9.0, Step::Deploy(0));
                self.after(19.0, Step::Metric(0));
                self.after(29.0, Step::Message);
                self.after(36.0, Step::Incident);
                self.after(46.0, Step::Resolve);
            }
            "alarm" => self.after(0.0, Step::Alarm),
            "resolve" => self.after(0.0, Step::Resolve),
            "deploy" => self.after(0.0, Step::Deploy(0)),
            "metric" => self.after(0.0, Step::Metric(0)),
            "message" => self.after(0.0, Step::Message),
            "incident" => self.after(0.0, Step::Incident),
            other => eprintln!("demo: unknown trigger {other}"),
        }
    }

    fn alarm_island(&self) -> Island {
        let mut island = sample_alarm("ALARM", "OK").island(Some("prod"));
        if self.acked_alarm {
            island = island.field("Acknowledged", "by you").silent(true);
        } else {
            // Snooze stays last; Acknowledge comes back to this source.
            island.actions.insert(1, Action::new("ack", "Acknowledge"));
        }
        island
    }

    fn incident_island(&self, acked: bool) -> Island {
        let island = Island::new(
            "incident-482",
            Kind::Timer {
                started_at: self.incident_started,
            },
            "SEV-2 · checkout latency",
        )
        .icon("🔥")
        .severity(Severity::Warning)
        .field("Service", "checkout-api")
        .field("p99", "2.4 s")
        .sticky();
        if acked {
            island
                .subtitle("Acknowledged · you are incident lead")
                .action(Action::new("resolve", "Resolve").primary())
                .silent(true)
        } else {
            island
                .subtitle("p99 latency above 2 s for 5 minutes")
                .action(Action::new("ack", "Acknowledge").primary())
                .action(Action::new("resolve", "Resolve"))
        }
    }

    fn run_step(&mut self, step: Step, sink: &Sink) {
        match step {
            Step::Alarm => {
                self.acked_alarm = false;
                sink.upsert(self.alarm_island());
            }
            Step::Resolve => sink.upsert(sample_alarm("OK", "ALARM").island(Some("prod"))),
            Step::Deploy(n) if n >= DEPLOY_STEPS => sink.upsert(
                Island::new(
                    "deploy",
                    Kind::Message { sender: None },
                    "Deployed api-service v2.14.0",
                )
                .subtitle("CodePipeline · 3 stages · 12 s")
                .icon("🚀")
                .severity(Severity::Success)
                .compact("Done")
                .ttl(8),
            ),
            Step::Deploy(n) => {
                let value = n as f64 / DEPLOY_STEPS as f64;
                let stage = match n * 3 / DEPLOY_STEPS {
                    0 => "Source",
                    1 => "Build",
                    _ => "Deploy",
                };
                sink.upsert(
                    Island::new("deploy", Kind::Progress { value }, "Deploying api-service")
                        .subtitle(format!("CodePipeline · stage {stage}"))
                        .icon("🚀")
                        .field("Commit", "a1f9c3e")
                        .ttl(30)
                        .silent(n > 0),
                );
                self.after(0.3, Step::Deploy(n + 1));
            }
            Step::Metric(n) => {
                if n == 0 {
                    self.series = (0..24).map(|i| 55.0 + i as f64 * 1.2).collect();
                }
                let last = self.series.last().copied().unwrap_or(80.0);
                let next = (last + (self.random() - 0.4) * 9.0).clamp(60.0, 99.0);
                self.series.push(next);
                if self.series.len() > 40 {
                    self.series.remove(0);
                }
                let severity = if next > 80.0 {
                    Severity::Warning
                } else {
                    Severity::Info
                };
                sink.upsert(
                    Island::new(
                        "rds-cpu",
                        Kind::Metric {
                            value: (next * 10.0).round() / 10.0,
                            unit: "%".into(),
                            threshold: Some(80.0),
                            series: self.series.clone(),
                        },
                        "orders-db CPU",
                    )
                    .subtitle("AWS/RDS · CPUUtilization · 1 min")
                    .icon("📈")
                    .severity(severity)
                    .compact(format!("{next:.0}%"))
                    .ttl(20)
                    .silent(n > 0),
                );
                if n < METRIC_STEPS {
                    self.after(2.0, Step::Metric(n + 1));
                }
            }
            Step::Message => sink.upsert(
                Island::new(
                    "oncall",
                    Kind::Message {
                        sender: Some("Capcom".into()),
                    },
                    "You're on call from 18:00",
                )
                .body("Primary for platform-prod until Monday 09:00. Handover notes are pinned in #platform.")
                .icon("💬")
                .compact("On call")
                .action(Action::new("ack", "Got it").primary())
                .ttl(14),
            ),
            Step::Incident => {
                self.incident_started = now_ms() - 5 * 60_000 - 12_000;
                sink.upsert(self.incident_island(false));
            }
        }
    }

    fn on_action(&mut self, key: &str, action: &str, sink: &Sink) {
        match (key, action) {
            (k, "ack") if k.starts_with("alarm:") => {
                self.acked_alarm = true;
                sink.upsert(self.alarm_island());
            }
            ("incident-482", "ack") => sink.upsert(self.incident_island(true)),
            ("incident-482", "resolve") => {
                let mins = (now_ms() - self.incident_started) / 60_000;
                sink.upsert(
                    Island::new(
                        "incident-482",
                        Kind::Message { sender: None },
                        "SEV-2 resolved",
                    )
                    .subtitle(format!("checkout latency · open for {mins} min"))
                    .icon("✅")
                    .severity(Severity::Success)
                    .compact("Resolved")
                    .ttl(8),
                );
            }
            ("oncall", "ack") => sink.remove("oncall"),
            _ => eprintln!("demo: no handler for {action} on {key}"),
        }
    }
}

impl Source for Demo {
    fn id(&self) -> String {
        DEMO_ID.into()
    }

    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>) {
        let mut demo = *self;
        if demo.autoplay {
            demo.trigger("tour");
        }
        loop {
            let wait = demo
                .queue
                .iter()
                .map(|(t, _)| t.saturating_duration_since(Instant::now()))
                .min()
                .unwrap_or(Duration::from_secs(3600));
            match inbox.recv_timeout(wait) {
                Ok(Inbound::Trigger(t)) => demo.trigger(&t),
                Ok(Inbound::Action { key, action }) => demo.on_action(&key, &action, &sink),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            let (mut due, rest): (Vec<_>, Vec<_>) =
                demo.queue.drain(..).partition(|(t, _)| *t <= now);
            demo.queue = rest;
            due.sort_by_key(|(t, _)| *t);
            for (_, step) in due {
                demo.run_step(step, &sink);
            }
        }
    }
}
