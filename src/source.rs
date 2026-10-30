//! The plug-in seam. A source runs on its own thread, posts islands through a
//! [`Sink`], and hears back about button presses (and menu triggers) on its
//! inbox. To add one, implement [`Source`] and list it in `sources::from_config`.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};

use tao::event_loop::EventLoopProxy;

use crate::UserEvent;
use crate::island::Island;

#[derive(Debug)]
pub enum SourceEvent {
    Upsert(Box<Island>),
    Remove(String),
    /// A line for the log and the tray tooltip, e.g. "polling 2 accounts".
    Status(String),
}

/// Sent to a source.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// The user pressed a button the app does not handle itself.
    Action { key: String, action: String },
    /// A named trigger from the menu or the panel (used by the demo).
    Trigger(String),
}

/// Where a source sends its islands. Cheap to clone across threads.
#[derive(Clone)]
pub struct Sink {
    source: String,
    proxy: EventLoopProxy<UserEvent>,
}

impl Sink {
    fn send(&self, event: SourceEvent) {
        let _ = self.proxy.send_event(UserEvent::Source {
            source: self.source.clone(),
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
    inboxes: HashMap<String, Sender<Inbound>>,
}

impl Registry {
    pub fn start(sources: Vec<Box<dyn Source>>, proxy: &EventLoopProxy<UserEvent>) -> Self {
        let mut inboxes = HashMap::new();
        for source in sources {
            let id = source.id();
            let (tx, rx) = mpsc::channel();
            let sink = Sink {
                source: id.clone(),
                proxy: proxy.clone(),
            };
            let spawned = std::thread::Builder::new()
                .name(format!("source:{id}"))
                .spawn(move || source.run(sink, rx));
            match spawned {
                Ok(_) => {
                    crate::log::line(&format!("source {id} started"));
                    inboxes.insert(id, tx);
                }
                Err(e) => crate::log::line(&format!("source {id}: {e}")),
            }
        }
        Self { inboxes }
    }

    pub fn send(&self, source: &str, msg: Inbound) {
        if let Some(tx) = self.inboxes.get(source) {
            let _ = tx.send(msg);
        }
    }

    pub fn ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.inboxes.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }
}
