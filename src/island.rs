//! The one notification model every source produces. An island is keyed by
//! `key` within its source; posting the same key again updates it in place.
//! It is also the JSON body of `POST /v1/islands` on the local API.

use serde::{Deserialize, Serialize};

const MAX_KEY: usize = 200;
const MAX_TITLE: usize = 120;
const MAX_TEXT: usize = 500;
const MAX_SERIES: usize = 120;
const MAX_ACTIONS: usize = 3;
const MAX_FIELDS: usize = 6;

/// Ordered from least to most urgent, so `max` picks the loudest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Success,
    #[default]
    Info,
    Warning,
    Critical,
}

/// What the island shows besides its title.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// A state change such as a CloudWatch alarm going `OK → ALARM`.
    Alert {
        #[serde(default)]
        from: Option<String>,
        to: String,
    },
    /// Work in flight; `value` runs from 0 to 1.
    Progress { value: f64 },
    /// A live number with an optional threshold and recent history.
    Metric {
        value: f64,
        #[serde(default)]
        unit: String,
        #[serde(default)]
        threshold: Option<f64>,
        #[serde(default)]
        series: Vec<f64>,
    },
    /// Plain text, optionally from someone.
    Message {
        #[serde(default)]
        sender: Option<String>,
    },
    /// A running clock since `started_at` (unix milliseconds).
    Timer { started_at: i64 },
}

impl Default for Kind {
    fn default() -> Self {
        Kind::Message { sender: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRole {
    #[default]
    Default,
    Primary,
    Destructive,
}

/// A button. `dismiss` and `snooze` are handled by the app, an action with a
/// `url` opens it, and anything else is sent back to the source that posted it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub role: ActionRole,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub snooze_mins: Option<u64>,
}

impl Action {
    pub fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            role: ActionRole::Default,
            url: None,
            snooze_mins: None,
        }
    }

    pub fn primary(mut self) -> Self {
        self.role = ActionRole::Primary;
        self
    }

    pub fn open(label: &str, url: &str) -> Self {
        Self {
            url: Some(url.into()),
            ..Self::new("open", label)
        }
    }

    pub fn snooze(mins: u64) -> Self {
        let label = if mins.is_multiple_of(60) {
            format!("Snooze {}h", mins / 60)
        } else {
            format!("Snooze {mins}m")
        };
        Self {
            snooze_mins: Some(mins),
            ..Self::new("snooze", &label)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Island {
    pub key: String,
    /// Set by the app from the source that posted it.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub kind: Kind,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// An emoji or a short glyph.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub severity: Severity,
    /// Short text for the right of the collapsed island; derived from the kind if absent.
    #[serde(default)]
    pub compact: Option<String>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Stays until dismissed or removed by its source.
    #[serde(default)]
    pub sticky: bool,
    /// Seconds before a non-sticky island goes away (default 15).
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    /// Identifies this occurrence, e.g. an alarm's state timestamp. A dismissed
    /// island stays dismissed while its source keeps posting the same revision.
    #[serde(default)]
    pub revision: Option<String>,
    /// Update in place without expanding or playing a sound (progress ticks).
    #[serde(default)]
    pub silent: bool,
}

pub const DEFAULT_TTL_SECS: u64 = 15;

fn clip(s: &mut String, max: usize) {
    if s.chars().count() > max {
        *s = s.chars().take(max - 1).collect::<String>() + "…";
    }
}

fn clip_opt(s: &mut Option<String>, max: usize) {
    if let Some(v) = s {
        clip(v, max);
        if v.trim().is_empty() {
            *s = None;
        }
    }
}

impl Island {
    pub fn new(key: &str, kind: Kind, title: &str) -> Self {
        Self {
            key: key.into(),
            source: String::new(),
            kind,
            title: title.into(),
            subtitle: None,
            body: None,
            icon: None,
            severity: Severity::Info,
            compact: None,
            fields: Vec::new(),
            actions: Vec::new(),
            sticky: false,
            ttl_secs: None,
            revision: None,
            silent: false,
        }
    }

    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }

    pub fn body(mut self, s: impl Into<String>) -> Self {
        self.body = Some(s.into());
        self
    }

    pub fn icon(mut self, s: &str) -> Self {
        self.icon = Some(s.into());
        self
    }

    pub fn severity(mut self, s: Severity) -> Self {
        self.severity = s;
        self
    }

    pub fn compact(mut self, s: impl Into<String>) -> Self {
        self.compact = Some(s.into());
        self
    }

    pub fn field(mut self, label: &str, value: impl Into<String>) -> Self {
        self.fields.push(Field {
            label: label.into(),
            value: value.into(),
        });
        self
    }

    pub fn action(mut self, a: Action) -> Self {
        self.actions.push(a);
        self
    }

    pub fn sticky(mut self) -> Self {
        self.sticky = true;
        self
    }

    pub fn ttl(mut self, secs: u64) -> Self {
        self.ttl_secs = Some(secs);
        self
    }

    pub fn revision(mut self, r: impl Into<String>) -> Self {
        self.revision = Some(r.into());
        self
    }

    pub fn silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }

    /// Check and tidy an island from outside (the local API, a queue).
    pub fn validated(mut self) -> Result<Self, String> {
        if self.key.trim().is_empty() {
            return Err("key is required".into());
        }
        if self.key.len() > MAX_KEY {
            return Err(format!("key is longer than {MAX_KEY} bytes"));
        }
        if self.title.trim().is_empty() {
            return Err("title is required".into());
        }
        clip(&mut self.title, MAX_TITLE);
        clip_opt(&mut self.subtitle, MAX_TEXT);
        clip_opt(&mut self.body, MAX_TEXT);
        clip_opt(&mut self.compact, 16);
        clip_opt(&mut self.icon, 4);
        clip_opt(&mut self.revision, MAX_KEY);
        self.fields.truncate(MAX_FIELDS);
        for f in &mut self.fields {
            clip(&mut f.label, 40);
            clip(&mut f.value, 120);
        }
        self.actions.truncate(MAX_ACTIONS);
        for a in &mut self.actions {
            clip(&mut a.label, 24);
            if a.url.as_deref().is_some_and(|u| !is_web_url(u)) {
                return Err(format!("action {}: url must be http or https", a.id));
            }
        }
        match &mut self.kind {
            Kind::Progress { value } => *value = value.clamp(0.0, 1.0),
            Kind::Metric { value, series, .. } => {
                if !value.is_finite() {
                    return Err("metric value must be a number".into());
                }
                series.retain(|v| v.is_finite());
                if series.len() > MAX_SERIES {
                    series.drain(..series.len() - MAX_SERIES);
                }
            }
            Kind::Alert { to, .. } => clip(to, 24),
            Kind::Message { .. } | Kind::Timer { .. } => {}
        }
        Ok(self)
    }
}

pub fn is_web_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_json_gets_defaults() {
        let i: Island = serde_json::from_str(r#"{"key":"k","title":"Hello"}"#).unwrap();
        assert_eq!(i.kind, Kind::Message { sender: None });
        assert_eq!(i.severity, Severity::Info);
        assert!(!i.sticky);
        assert!(i.validated().is_ok());
    }

    #[test]
    fn kinds_parse_by_type_tag() {
        let i: Island = serde_json::from_str(
            r#"{"key":"m","title":"CPU","kind":{"type":"metric","value":91.5,"unit":"%","threshold":80,"series":[70,80,91.5]}}"#,
        )
        .unwrap();
        assert!(matches!(i.kind, Kind::Metric { threshold: Some(t), .. } if t == 80.0));
        let p: Island = serde_json::from_str(
            r#"{"key":"p","title":"Deploy","kind":{"type":"progress","value":4}}"#,
        )
        .unwrap();
        assert_eq!(p.validated().unwrap().kind, Kind::Progress { value: 1.0 });
    }

    #[test]
    fn validation_rejects_and_clips() {
        assert!(Island::new("", Kind::default(), "t").validated().is_err());
        assert!(Island::new("k", Kind::default(), " ").validated().is_err());
        let bad = Island::new("k", Kind::default(), "t").action(Action::open("x", "file:///etc"));
        assert!(bad.validated().is_err());
        let long = Island::new("k", Kind::default(), &"x".repeat(300));
        let v = long.validated().unwrap();
        assert_eq!(v.title.chars().count(), MAX_TITLE);
        assert!(v.title.ends_with('…'));
    }

    #[test]
    fn severity_orders_by_urgency() {
        assert!(Severity::Critical > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
        assert!(Severity::Info > Severity::Success);
    }
}
