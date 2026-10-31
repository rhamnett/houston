//! Update check: asks GitHub for the latest Houston release half a minute
//! after launch and then daily, or now when the menu (or an agent) says
//! "check". A newer version becomes a sticky island with Download, What's new
//! and Skip this version. Nothing is installed automatically.
//!
//! The repository is private for now, so the check needs a GitHub token
//! (fine-grained, Contents: read) in the Keychain; without one it reports why
//! in its status line. `HOUSTON_UPDATE_URL` points it at another feed (the
//! end-to-end tests serve their own).

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::config::{Config, UpdatesConfig};
use crate::island::{Action, Island, Kind, Severity};
use crate::source::{Inbound, Sink, Source};

pub const ID: &str = "updates";
pub const REPO: &str = "rhamnett/houston";
const FIRST_CHECK: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(24 * 3600);
const KEY: &str = "update";
const SKIP: &str = "skip";

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

/// `v1.2.3`, `1.2`, `1.2.3-beta` → (1, 2, 3); pre-release and build tags are
/// ignored (GitHub's "latest" never points at a pre-release).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = || -> Option<u64> {
        match parts.next() {
            Some(p) => p.parse().ok(),
            None => Some(0),
        }
    };
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(t), Some(c)) => t > c,
        _ => false,
    }
}

impl Release {
    /// The packaged app, if the release has one.
    fn download(&self) -> Option<&str> {
        self.assets
            .iter()
            .find(|a| a.name.ends_with("-macos.zip"))
            .map(|a| a.browser_download_url.as_str())
    }

    pub fn island(&self, current: &str) -> Island {
        // The notes open behind What's new: they start with install steps.
        let version = self.tag_name.trim_start_matches('v');
        let mut island = Island::new(
            KEY,
            Kind::Message {
                sender: Some("Houston".into()),
            },
            &format!("Houston {version} is ready for launch"),
        )
        .subtitle(format!("You have {current}"))
        .icon("🚀")
        .severity(Severity::Info)
        .compact(format!("v{version}"))
        .revision(self.tag_name.clone())
        .sticky();
        if let Some(url) = self.download() {
            island = island.action(Action {
                url: Some(url.into()),
                ..Action::new("download", "Download ↗").primary()
            });
        }
        island
            .action(Action {
                url: Some(self.html_url.clone()),
                ..Action::new("notes", "What's new ↗")
            })
            .action(Action::new(SKIP, "Skip this version"))
    }
}

fn skipped_path() -> Option<std::path::PathBuf> {
    Some(Config::dir()?.join("skipped-update"))
}

fn skipped() -> Option<String> {
    let text = std::fs::read_to_string(skipped_path()?).ok()?;
    Some(text.trim().to_string()).filter(|s| !s.is_empty())
}

pub struct Updates {
    config: UpdatesConfig,
    feed: String,
    agent: ureq::Agent,
}

impl Updates {
    pub fn new(config: UpdatesConfig) -> Self {
        let feed = std::env::var("HOUSTON_UPDATE_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("https://api.github.com/repos/{REPO}/releases/latest"));
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            config,
            feed,
            agent,
        }
    }

    fn latest(&self) -> Result<Release, String> {
        let mut request = self
            .agent
            .get(&self.feed)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", &format!("houston/{}", crate::VERSION));
        let token = self.config.token.trim();
        if !token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let mut response = request.call().map_err(|e| e.to_string())?;
        match response.status().as_u16() {
            200 => response
                .body_mut()
                .read_json::<Release>()
                .map_err(|e| format!("release: {e}")),
            404 if token.is_empty() => Err(format!(
                "no release visible: {REPO} is private, so add a GitHub token in Mission settings"
            )),
            404 => Err("no release yet, or the token can't read the repository".into()),
            401 | 403 => Err("GitHub refused the token (expired, or no Contents: read)".into()),
            status => Err(format!("GitHub answered {status}")),
        }
    }

    /// Check, returning the newer version if there is one. `manual`: someone
    /// asked, so say so even when there's nothing new.
    fn check(&self, sink: &Sink, manual: bool) -> Option<String> {
        let current = crate::VERSION;
        match self.latest() {
            Ok(release) if is_newer(&release.tag_name, current) => {
                sink.status(format!("updates: {} available", release.tag_name));
                if manual || skipped().as_deref() != Some(&release.tag_name) {
                    sink.upsert(release.island(current));
                }
                return Some(release.tag_name);
            }
            Ok(release) => {
                sink.status(format!("updates: up to date (latest {})", release.tag_name));
                sink.remove(KEY);
                if manual {
                    sink.upsert(
                        Island::new(KEY, Kind::default(), "Houston is up to date")
                            .subtitle(format!("{current} is the latest"))
                            .icon("🛰️")
                            .severity(Severity::Success)
                            .ttl(8),
                    );
                }
            }
            Err(e) => {
                sink.status(format!("updates: {e}"));
                if manual {
                    sink.upsert(
                        Island::new(KEY, Kind::default(), "Couldn't check for updates")
                            .subtitle(e)
                            .icon("📡")
                            .severity(Severity::Warning)
                            .ttl(30),
                    );
                }
            }
        }
        None
    }

    fn skip(&self, sink: &Sink, version: &str) {
        if let Some(path) = skipped_path()
            && let Err(e) = std::fs::write(&path, version)
        {
            crate::log::line(&format!("updates: {}: {e}", path.display()));
        }
        sink.remove(KEY);
    }
}

impl Source for Updates {
    fn id(&self) -> String {
        ID.into()
    }

    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>) {
        let mut next = Instant::now() + FIRST_CHECK;
        // The version on the island, for Skip.
        let mut offered: Option<String> = None;
        loop {
            match inbox.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(Inbound::Trigger(t)) if t == "check" => {
                    offered = self.check(&sink, true);
                    next = Instant::now() + EVERY;
                }
                Ok(Inbound::Action { action, .. }) if action == SKIP => {
                    if let Some(v) = offered.take() {
                        self.skip(&sink, &v);
                    }
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    offered = self.check(&sink, false).or(offered);
                    next = Instant::now() + EVERY;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10"), Some((0, 10, 0)));
        assert_eq!(parse_version("v2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer("v0.10.0", "0.9.9"), "numeric, not alphabetical");
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn release_island() {
        let release: Release = serde_json::from_value(serde_json::json!({
            "tag_name": "v0.3.0",
            "html_url": "https://github.com/rhamnett/houston/releases/tag/v0.3.0",
            "body": "## What's new\n- Watch list\n- MCP server\n- Faster polls\n- More",
            "assets": [
                {"name": "Houston-0.3.0-macos.zip.sha256", "browser_download_url": "https://x/sha"},
                {"name": "Houston-0.3.0-macos.zip", "browser_download_url": "https://x/zip"},
            ],
        }))
        .unwrap();
        let island = release.island("0.2.0").validated().unwrap();
        assert_eq!(island.title, "Houston 0.3.0 is ready for launch");
        assert_eq!(island.body, None, "the notes are behind What's new");
        assert!(island.sticky);
        let ids: Vec<&str> = island.actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["download", "notes", "skip"]);
        assert_eq!(
            island.actions[0].url.as_deref(),
            Some("https://x/zip"),
            "the zip, not its checksum"
        );
    }
}
