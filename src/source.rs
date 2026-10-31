//! The plug-in seam. A source runs on its own thread, posts islands through a
//! [`Sink`], and hears back about button presses (and menu triggers) on its
//! inbox. To add one, implement [`Source`] and list it in `sources::build`.
//!
//! A source can be restarted (after its settings change) by dropping its inbox:
//! the old thread returns when it next reads it. Every event carries the
//! generation of the thread that sent it, so stragglers from a stopped thread
//! are ignored.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use tao::event_loop::EventLoopProxy;

use crate::UserEvent;
use crate::config::AlarmRule;
use crate::control::Command;
use crate::island::Island;

#[derive(Debug)]
pub enum SourceEvent {
    Upsert(Box<Island>),
    Remove(String),
    /// A line for the log and the tray tooltip, e.g. "polling 2 accounts".
    Status(String),
    /// Everything the source checks, in any state (CloudWatch's alarms), for
    /// the watch list in the expanded island. Replaces the last list.
    Watching(Vec<Watched>),
}

/// One thing a source keeps an eye on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Watched {
    pub name: String,
    /// e.g. `ALARM`, `OK`, `INSUFFICIENT_DATA`.
    pub state: String,
    /// Which account or group it belongs to.
    pub group: String,
    pub url: Option<String>,
    /// A rule drops it, so it never raises an island.
    pub muted: bool,
}

/// Sent to a source.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// The user pressed a button the app does not handle itself.
    Action { key: String, action: String },
    /// A named trigger from the menu or the panel (used by the demo).
    Trigger(String),
    /// New alarm rules, for a source that applies them without a restart
    /// (the local API, so an agent changing rules keeps its connection).
    Rules(Vec<AlarmRule>),
}

/// The app's event loop, or a channel standing in for it in tests.
#[derive(Clone)]
enum Outlet {
    App(EventLoopProxy<UserEvent>),
    #[cfg(test)]
    Channel(Sender<UserEvent>),
}

impl Outlet {
    /// False once the app (or test) has gone.
    fn send(&self, event: UserEvent) -> bool {
        match self {
            Outlet::App(proxy) => proxy.send_event(event).is_ok(),
            #[cfg(test)]
            Outlet::Channel(tx) => tx.send(event).is_ok(),
        }
    }
}

/// Where a source sends its islands. Cheap to clone across threads.
#[derive(Clone)]
pub struct Sink {
    source: String,
    generation: u64,
    outlet: Outlet,
}

impl Sink {
    /// A sink whose events arrive on the returned receiver.
    #[cfg(test)]
    pub fn channel(source: &str) -> (Self, Receiver<UserEvent>) {
        let (tx, rx) = mpsc::channel();
        let sink = Self {
            source: source.into(),
            generation: 1,
            outlet: Outlet::Channel(tx),
        };
        (sink, rx)
    }

    fn send(&self, event: SourceEvent) {
        self.outlet.send(UserEvent::Source {
            source: self.source.clone(),
            generation: self.generation,
            event,
        });
    }

    pub fn upsert(&self, mut island: Island) {
        island.source = self.source.clone();
        self.send(SourceEvent::Upsert(Box::new(island)));
    }

    pub fn remove(&self, key: &str) {
        self.send(SourceEvent::Remove(key.to_string()));
    }

    pub fn status(&self, msg: impl Into<String>) {
        self.send(SourceEvent::Status(msg.into()));
    }

    pub fn watching(&self, list: Vec<Watched>) {
        self.send(SourceEvent::Watching(list));
    }

    /// Have the app carry out `command` and wait for its answer (the local
    /// API's remote control). Checks against AWS can take a while.
    pub fn request(&self, command: Command) -> Result<Value, String> {
        let (reply, answer) = mpsc::channel();
        if !self.outlet.send(UserEvent::Control { command, reply }) {
            return Err("Houston is shutting down.".into());
        }
        answer
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| "Houston didn't answer in time.".to_string())?
    }
}

pub trait Source: Send + 'static {
    /// Unique and stable; part of every island id this source posts.
    fn id(&self) -> String;
    /// Runs on a dedicated thread for the life of the app. Return when the
    /// inbox disconnects.
    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>);
}

/// The running sources, for routing messages back to them.
pub struct Registry {
    inboxes: HashMap<String, (u64, Sender<Inbound>)>,
    next_generation: u64,
    proxy: EventLoopProxy<UserEvent>,
}

impl Registry {
    pub fn start(sources: Vec<Box<dyn Source>>, proxy: &EventLoopProxy<UserEvent>) -> Self {
        let mut registry = Self {
            inboxes: HashMap::new(),
            next_generation: 1,
            proxy: proxy.clone(),
        };
        for source in sources {
            registry.spawn(source);
        }
        registry
    }

    fn spawn(&mut self, source: Box<dyn Source>) {
        let id = source.id();
        let generation = self.next_generation;
        self.next_generation += 1;
        let (tx, rx) = mpsc::channel();
        let sink = Sink {
            source: id.clone(),
            generation,
            outlet: Outlet::App(self.proxy.clone()),
        };
        let spawned = std::thread::Builder::new()
            .name(format!("source:{id}"))
            .spawn(move || source.run(sink, rx));
        match spawned {
            Ok(_) => {
                crate::log::line(&format!("source {id} started"));
                self.inboxes.insert(id, (generation, tx));
            }
            Err(e) => crate::log::line(&format!("source {id}: {e}")),
        }
    }

    /// Stop the source called `id`, if running, and start `replacement` (its
    /// new configuration) if there is one.
    pub fn restart(&mut self, id: &str, replacement: Option<Box<dyn Source>>) {
        if self.inboxes.remove(id).is_some() {
            crate::log::line(&format!("source {id} stopped"));
        }
        if let Some(source) = replacement {
            self.spawn(source);
        }
    }

    /// Whether an event from this thread of `source` should still count.
    pub fn is_current(&self, source: &str, generation: u64) -> bool {
        self.inboxes
            .get(source)
            .is_some_and(|(g, _)| *g == generation)
    }

    pub fn send(&self, source: &str, msg: Inbound) {
        if let Some((_, tx)) = self.inboxes.get(source) {
            let _ = tx.send(msg);
        }
    }

    pub fn ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.inboxes.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }
}
