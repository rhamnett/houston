//! Polls Linear's GraphQL API for two kinds of issue:
//!
//! - issues assigned to you: an island when one is newly assigned or changes
//!   state or priority (not on the first poll, so launch is quiet)
//! - high-priority issues in the teams you watch: sticky until they are
//!   completed, cancelled or drop below `max_priority`
//!
//! Islands carry **Open in Linear ↗**, and **Take it** on issues that aren't
//! yours, which assigns the issue to you and polls again.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::LinearConfig;
use crate::island::{Action, Island, Kind, Severity};
use crate::source::{Inbound, Sink, Source};

const ENDPOINT: &str = "https://api.linear.app/graphql";
const MIN_INTERVAL_SECS: u64 = 30;
const ERROR_KEY: &str = "linear:error";
const TAKE: &str = "take";

const QUERY: &str = r#"
query Houston($teams: [String!], $maxPriority: Float, $mine: Boolean!, $watch: Boolean!) {
  viewer {
    id
    assignedIssues(first: 50, orderBy: updatedAt,
      filter: { state: { type: { nin: ["completed", "canceled"] } } }) @include(if: $mine) {
      nodes { ...Fields }
    }
  }
  teamIssues: issues(first: 50, orderBy: updatedAt,
    filter: {
      team: { key: { in: $teams } }
      priority: { gte: 1, lte: $maxPriority }
      state: { type: { nin: ["completed", "canceled"] } }
    }) @include(if: $watch) {
    nodes { ...Fields }
  }
}
fragment Fields on Issue {
  id identifier title url priority priorityLabel
  state { name }
  team { key }
  assignee { id name }
}
"#;

const ASSIGN: &str = r#"
mutation Take($id: String!, $me: String!) {
  issueUpdate(id: $id, input: { assigneeId: $me }) { success }
}
"#;

#[derive(Debug, Deserialize)]
struct Response<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphError>,
}

#[derive(Debug, Deserialize)]
struct GraphError {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data {
    viewer: Viewer,
    team_issues: Option<Nodes>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Viewer {
    id: String,
    assigned_issues: Option<Nodes>,
}

#[derive(Debug, Deserialize)]
struct Nodes {
    nodes: Vec<Issue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Issue {
    id: String,
    identifier: String,
    title: String,
    url: String,
    /// 0 none, 1 urgent, 2 high, 3 medium, 4 low. A Float in the schema.
    priority: f64,
    priority_label: String,
    state: Named,
    team: Keyed,
    assignee: Option<Person>,
}

#[derive(Debug, Clone, Deserialize)]
struct Named {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Keyed {
    key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Person {
    id: String,
    name: String,
}

/// One poll: who you are, and every issue worth showing keyed by identifier.
struct Snapshot {
    me: String,
    issues: HashMap<String, Watched>,
}

#[derive(Clone)]
struct Watched {
    issue: Issue,
    /// Came from the team watch, so it stays until resolved.
    urgent: bool,
}

impl Watched {
    /// What counts as a change worth a peek.
    fn revision(&self) -> String {
        let who = self.issue.assignee.as_ref().map_or("-", |a| a.id.as_str());
        format!("{}|{}|{who}", self.issue.state.name, self.issue.priority)
    }

    fn island(&self, me: &str) -> Island {
        let i = &self.issue;
        let mine = i.assignee.as_ref().is_some_and(|a| a.id == me);
        let priority = i.priority.round() as u8;
        let severity = match priority {
            1 => Severity::Critical,
            2 if self.urgent => Severity::Warning,
            _ => Severity::Info,
        };
        let who = match &i.assignee {
            _ if mine => "you".to_string(),
            Some(a) => a.name.clone(),
            None => "unassigned".to_string(),
        };
        let mut island = Island::new(
            &key(&i.identifier),
            Kind::Alert {
                from: None,
                to: i.state.name.clone(),
            },
            &i.title,
        )
        .subtitle(format!("{} · {}", i.identifier, i.priority_label))
        .icon(if priority == 1 { "🔥" } else { "📋" })
        .severity(severity)
        .compact(i.identifier.clone())
        .field("Team", i.team.key.clone())
        .field("Assignee", who)
        .revision(self.revision())
        .action(Action::open("Open in Linear ↗", &i.url).primary());
        if !mine {
            island = island.action(Action::new(TAKE, "Take it"));
        }
        if self.urgent {
            island.sticky().action(Action::snooze(60))
        } else {
            island.ttl(60)
        }
    }
}

fn key(identifier: &str) -> String {
    format!("linear:{identifier}")
}

fn snapshot(data: Data) -> Snapshot {
    let mut issues = HashMap::new();
    for issue in data
        .viewer
        .assigned_issues
        .map(|n| n.nodes)
        .unwrap_or_default()
    {
        issues.insert(
            issue.identifier.clone(),
            Watched {
                issue,
                urgent: false,
            },
        );
    }
    for issue in data.team_issues.map(|n| n.nodes).unwrap_or_default() {
        issues.insert(
            issue.identifier.clone(),
            Watched {
                issue,
                urgent: true,
            },
        );
    }
    Snapshot {
        me: data.viewer.id,
        issues,
    }
}

fn error_island(err: &str) -> Island {
    let hint = if err.to_lowercase().contains("authenticat") {
        "Check linear.api_key in config.json"
    } else {
        "Will retry on the next poll"
    };
    Island::new(
        ERROR_KEY,
        Kind::Message { sender: None },
        "Lost signal from Linear",
    )
    .subtitle(hint)
    .body(err.to_string())
    .icon("🔑")
    .severity(Severity::Warning)
    .compact("Linear")
    .revision(err.to_string())
    .ttl(600)
}

pub struct Linear {
    config: LinearConfig,
    agent: ureq::Agent,
}

impl Linear {
    pub fn new(config: LinearConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            // Linear puts the reason in a GraphQL errors body, even on a 4xx.
            .http_status_as_error(false)
            .build()
            .into();
        Self { config, agent }
    }

    fn api_key(&self) -> Option<String> {
        Some(self.config.api_key.trim().to_string())
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("LINEAR_API_KEY").ok())
    }

    fn graphql<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        vars: Value,
    ) -> Result<T, String> {
        let key = self.api_key().ok_or("no API key")?;
        let r: Response<T> = self
            .agent
            .post(ENDPOINT)
            // Personal keys go in as-is; OAuth tokens would need "Bearer ".
            .header("Authorization", &key)
            .send_json(json!({ "query": query, "variables": vars }))
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_json()
            .map_err(|e| e.to_string())?;
        if let Some(e) = r.errors.first() {
            return Err(e.message.clone());
        }
        r.data.ok_or_else(|| "empty response".into())
    }

    fn poll(&self) -> Result<Snapshot, String> {
        let data: Data = self.graphql(
            QUERY,
            json!({
                "teams": self.config.teams,
                "maxPriority": self.config.max_priority.clamp(1, 4),
                "mine": self.config.assigned_to_me,
                "watch": !self.config.teams.is_empty(),
            }),
        )?;
        Ok(snapshot(data))
    }

    fn take(&self, issue_id: &str, me: &str) -> Result<(), String> {
        let _: Value = self.graphql(ASSIGN, json!({ "id": issue_id, "me": me }))?;
        Ok(())
    }
}

impl Source for Linear {
    fn id(&self) -> String {
        "linear".into()
    }

    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>) {
        let interval = Duration::from_secs(self.config.interval_secs.max(MIN_INTERVAL_SECS));
        sink.status(format!(
            "linear: {}{} team(s), every {}s",
            if self.config.assigned_to_me {
                "your issues + "
            } else {
                ""
            },
            self.config.teams.len(),
            interval.as_secs()
        ));
        // Revision per identifier at the last good poll; None until the first.
        let mut seen: Option<HashMap<String, String>> = None;
        let mut current: Option<Snapshot> = None;
        loop {
            let started = Instant::now();
            match self.poll() {
                Ok(snap) => {
                    sink.remove(ERROR_KEY);
                    let first = seen.is_none();
                    let before = seen.take().unwrap_or_default();
                    for (id, w) in &snap.issues {
                        let changed = before.get(id) != Some(&w.revision());
                        // Launch is quiet: post only the sticky ones, silently.
                        if first && w.urgent {
                            sink.upsert(w.island(&snap.me).silent(true));
                        } else if !first && changed {
                            sink.upsert(w.island(&snap.me));
                        }
                    }
                    for id in before.keys().filter(|id| !snap.issues.contains_key(*id)) {
                        sink.remove(&key(id));
                    }
                    seen = Some(
                        snap.issues
                            .iter()
                            .map(|(id, w)| (id.clone(), w.revision()))
                            .collect(),
                    );
                    current = Some(snap);
                }
                Err(e) => {
                    crate::log::line(&format!("linear: {e}"));
                    sink.upsert(error_island(&e));
                }
            }
            let deadline = started + interval;
            loop {
                match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(Inbound::Action { key: k, action }) if action == TAKE => {
                        let target = current.as_ref().and_then(|s| {
                            let w = s.issues.values().find(|w| key(&w.issue.identifier) == k)?;
                            Some((w.issue.id.clone(), s.me.clone()))
                        });
                        if let Some((issue_id, me)) = target {
                            match self.take(&issue_id, &me) {
                                Ok(()) => crate::log::line(&format!("linear: took {k}")),
                                Err(e) => sink.upsert(error_island(&e)),
                            }
                        }
                        break; // poll now so the card shows the new assignee
                    }
                    Ok(_) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &str = r#"{"data":{
      "viewer":{"id":"me","assignedIssues":{"nodes":[
        {"id":"u1","identifier":"APO-1","title":"Mine","url":"https://linear.app/x/issue/APO-1",
         "priority":3.0,"priorityLabel":"Medium","state":{"name":"Todo"},"team":{"key":"APO"},
         "assignee":{"id":"me","name":"Me"}}]}},
      "teamIssues":{"nodes":[
        {"id":"u2","identifier":"APO-2","title":"Calls dropping","url":"https://linear.app/x/issue/APO-2",
         "priority":1,"priorityLabel":"Urgent","state":{"name":"Triage"},"team":{"key":"APO"},
         "assignee":null}]}}}"#;

    fn snap() -> Snapshot {
        let r: Response<Data> = serde_json::from_str(RESPONSE).unwrap();
        snapshot(r.data.unwrap())
    }

    #[test]
    fn maps_issues() {
        let s = snap();
        assert_eq!(s.me, "me");
        let mine = s.issues["APO-1"].island(&s.me);
        assert_eq!(mine.severity, Severity::Info);
        assert!(!mine.sticky);
        assert_eq!(mine.actions.len(), 1, "no Take it on your own issue");

        let urgent = s.issues["APO-2"].island(&s.me);
        assert_eq!(urgent.key, "linear:APO-2");
        assert_eq!(urgent.severity, Severity::Critical);
        assert!(urgent.sticky);
        assert_eq!(urgent.fields[1].value, "unassigned");
        let ids: Vec<&str> = urgent.actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["open", TAKE, "snooze"]);
        assert!(urgent.clone().validated().is_ok());
    }

    #[test]
    fn revision_tracks_state_priority_and_assignee() {
        let s = snap();
        let mut w = s.issues["APO-2"].clone();
        let r = w.revision();
        w.issue.title = "renamed".into();
        assert_eq!(w.revision(), r, "title edits don't peek");
        w.issue.state.name = "In Progress".into();
        assert_ne!(w.revision(), r);
    }

    #[test]
    fn graphql_errors_surface() {
        let r: Response<Data> =
            serde_json::from_str(r#"{"errors":[{"message":"Authentication required"}]}"#).unwrap();
        assert!(r.data.is_none());
        assert_eq!(r.errors[0].message, "Authentication required");
    }

    /// Hits the real API: `cargo test -- --ignored linear`.
    #[test]
    #[ignore]
    fn live_bad_key_is_reported() {
        let l = Linear::new(LinearConfig {
            api_key: "lin_api_not_a_real_key".into(),
            teams: vec!["APO".into()],
            ..LinearConfig::default()
        });
        let err = l.poll().err().expect("a bad key must fail");
        assert!(err.to_lowercase().contains("authenticat"), "{err}");
        assert_eq!(
            error_island(&err).subtitle.as_deref(),
            Some("Check linear.api_key in config.json")
        );
    }
}
