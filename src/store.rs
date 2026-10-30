//! The islands currently showing, keyed by `source/key`, with expiry, snooze
//! and dismissal. Decides whether a post should expand the island ("peek").

use std::collections::HashMap;

use crate::island::{DEFAULT_TTL_SECS, Island};

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub island: Island,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub expires_ms: Option<i64>,
}

/// What a post did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    /// Snoozed or already dismissed; nothing changed.
    Ignored,
    /// Updated in place.
    Quiet,
    /// New, escalated or a new revision: expand to show it.
    Peek,
}

#[derive(Debug, Default)]
pub struct Store {
    entries: Vec<Entry>,
    /// id → unix ms until which posts are ignored.
    snoozed: HashMap<String, i64>,
    /// id → revision that was dismissed.
    dismissed: HashMap<String, String>,
}

pub fn id_of(source: &str, key: &str) -> String {
    format!("{source}/{key}")
}

impl Store {
    pub fn upsert(&mut self, island: Island, now_ms: i64) -> Upsert {
        let id = id_of(&island.source, &island.key);
        if self.snoozed.get(&id).is_some_and(|&until| until > now_ms) {
            return Upsert::Ignored;
        }
        self.snoozed.remove(&id);
        match (&island.revision, self.dismissed.get(&id)) {
            (Some(rev), Some(gone)) if rev == gone => return Upsert::Ignored,
            _ => {
                self.dismissed.remove(&id);
            }
        }
        let expires_ms = (!island.sticky)
            .then(|| now_ms + 1000 * island.ttl_secs.unwrap_or(DEFAULT_TTL_SECS) as i64);
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            let louder = island.severity > e.island.severity;
            let new_revision = island.revision.is_some() && island.revision != e.island.revision;
            let outcome = if !island.silent && (louder || new_revision) {
                Upsert::Peek
            } else {
                Upsert::Quiet
            };
            e.island = island;
            e.updated_ms = now_ms;
            e.expires_ms = expires_ms;
            return outcome;
        }
        self.entries.push(Entry {
            id,
            island,
            created_ms: now_ms,
            updated_ms: now_ms,
            expires_ms,
        });
        Upsert::Peek
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The source cleared it; it may come back with any revision.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    /// The user closed it; the same revision stays hidden.
    pub fn dismiss(&mut self, id: &str) -> bool {
        let Some(pos) = self.entries.iter().position(|e| e.id == id) else {
            return false;
        };
        let e = self.entries.remove(pos);
        if let Some(rev) = e.island.revision {
            self.dismissed.insert(e.id, rev);
        }
        true
    }

    pub fn snooze(&mut self, id: &str, mins: u64, now_ms: i64) -> bool {
        if !self.remove(id) {
            return false;
        }
        self.snoozed
            .insert(id.to_string(), now_ms + mins as i64 * 60_000);
        true
    }

    pub fn clear(&mut self) {
        let ids: Vec<String> = self.entries.iter().map(|e| e.id.clone()).collect();
        for id in ids {
            self.dismiss(&id);
        }
    }

    /// Drop expired islands; true if any went.
    pub fn expire(&mut self, now_ms: i64) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| e.expires_ms.is_none_or(|t| t > now_ms));
        self.snoozed.retain(|_, until| *until > now_ms);
        self.entries.len() != before
    }

    pub fn next_expiry(&self) -> Option<i64> {
        self.entries.iter().filter_map(|e| e.expires_ms).min()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Most urgent first, then most recently updated.
    pub fn sorted(&self) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.entries.iter().collect();
        v.sort_by(|a, b| {
            b.island
                .severity
                .cmp(&a.island.severity)
                .then(b.updated_ms.cmp(&a.updated_ms))
        });
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::{Kind, Severity};

    fn island(key: &str) -> Island {
        let mut i = Island::new(key, Kind::default(), "t");
        i.source = "s".into();
        i
    }

    #[test]
    fn new_peeks_update_is_quiet_escalation_peeks() {
        let mut s = Store::default();
        assert_eq!(s.upsert(island("a"), 0), Upsert::Peek);
        assert_eq!(s.upsert(island("a"), 1), Upsert::Quiet);
        assert_eq!(
            s.upsert(island("a").severity(Severity::Critical), 2),
            Upsert::Peek
        );
        assert_eq!(
            s.upsert(island("a").severity(Severity::Critical).silent(true), 3),
            Upsert::Quiet
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s.get("s/a").unwrap().created_ms, 0);
    }

    #[test]
    fn dismissed_revision_stays_hidden_until_it_changes() {
        let mut s = Store::default();
        s.upsert(island("a").sticky().revision("r1"), 0);
        assert!(s.dismiss("s/a"));
        assert_eq!(s.upsert(island("a").revision("r1"), 1), Upsert::Ignored);
        assert_eq!(s.upsert(island("a").revision("r2"), 2), Upsert::Peek);
    }

    #[test]
    fn snooze_and_expiry() {
        let mut s = Store::default();
        s.upsert(island("a"), 0);
        assert!(s.snooze("s/a", 1, 0));
        assert_eq!(s.upsert(island("a"), 30_000), Upsert::Ignored);
        assert_eq!(s.upsert(island("a"), 61_000), Upsert::Peek);
        assert_eq!(s.next_expiry(), Some(61_000 + 15_000));
        assert!(!s.expire(70_000));
        assert!(s.expire(76_000));
        assert!(s.is_empty());
    }

    #[test]
    fn sorted_by_severity_then_recency() {
        let mut s = Store::default();
        s.upsert(island("info"), 0);
        s.upsert(island("crit").severity(Severity::Critical), 1);
        s.upsert(island("info2"), 2);
        let order: Vec<&str> = s.sorted().iter().map(|e| e.island.key.as_str()).collect();
        assert_eq!(order, ["crit", "info2", "info"]);
    }
}
