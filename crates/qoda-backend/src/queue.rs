//! Session membership and its selected/upcoming projection. Traversed entries
//! are retained as membership, not claimed as listening history.
use qoda_core::{Counter, MembershipError, OccurrenceId, QueueMembership, SessionId};
use qoda_provider::Track;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::collections::{HashMap, HashSet};
const LIMIT: usize = 500;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Conflict,
    Missing,
    Pinned,
    Limit,
    Counter,
    Version,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self{
  Self::Conflict=>"The queue changed. Review its current order and retry.",Self::Missing=>"That occurrence is no longer in the current or upcoming queue.",Self::Pinned=>"The selected entry stays pinned; move upcoming entries instead.",Self::Limit=>"This development session supports at most 500 occurrences. Start a new session or remove entries.",Self::Counter=>"The queue revision cannot advance. Start a new session.",Self::Version=>"The selected media version changed. Play it as new content to choose the new version.",})
    }
}
impl From<MembershipError> for Error {
    fn from(e: MembershipError) -> Self {
        match e {
            MembershipError::RevisionConflict => Self::Conflict,
            MembershipError::CounterExhausted => Self::Counter,
            _ => Self::Missing,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct Row {
    pub id: OccurrenceId,
    pub track: Track,
    pub selected: bool,
}
#[derive(Serialize)]
pub struct Snapshot {
    pub session: SessionId,
    pub revision: Counter,
    pub selected: Option<OccurrenceId>,
    pub ended: bool,
    pub entries: Vec<Row>,
    pub membership_count: usize,
    pub position_seconds: f64,
}
#[derive(Clone, Serialize)]
pub struct Queue {
    membership: QueueMembership,
    revision: Counter,
    metadata: HashMap<OccurrenceId, Track>,
    selected: Option<OccurrenceId>,
    traversed: HashSet<OccurrenceId>,
    priority: Vec<OccurrenceId>,
    ended: bool,
    position_seconds: f64,
    versions: HashMap<OccurrenceId, String>,
}
impl Default for Queue {
    fn default() -> Self {
        Self {
            membership: QueueMembership::new(),
            revision: Counter::ZERO,
            metadata: HashMap::new(),
            selected: None,
            traversed: HashSet::new(),
            priority: Vec::new(),
            ended: false,
            position_seconds: 0.0,
            versions: HashMap::new(),
        }
    }
}
impl Queue {
    pub fn durable_stamp(&self) -> (SessionId, Counter, u64) {
        (
            self.membership.session_id(),
            self.revision,
            self.position_seconds.to_bits(),
        )
    }
    pub fn position(&self) -> f64 {
        self.position_seconds
    }
    pub fn seek_to(&mut self, seconds: f64) -> Result<(), Error> {
        let revision = if self.ended {
            Some(self.revision.checked_next().map_err(|_| Error::Counter)?)
        } else {
            None
        };
        self.set_position(seconds)?;
        if let Some(revision) = revision {
            self.ended = false;
            self.revision = revision;
        }
        Ok(())
    }
    pub fn set_position(&mut self, seconds: f64) -> Result<(), Error> {
        if self.selected.is_none() || !seconds.is_finite() || !(0.0..=1.0e9).contains(&seconds) {
            return Err(Error::Missing);
        }
        self.position_seconds = seconds;
        Ok(())
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            session: self.membership.session_id(),
            revision: self.revision,
            selected: self.selected,
            ended: self.ended,
            entries: self
                .membership
                .entries()
                .iter()
                .filter(|e| !self.traversed.contains(&e.id))
                .map(|e| Row {
                    id: e.id,
                    track: self.metadata[&e.id].clone(),
                    selected: Some(e.id) == self.selected,
                })
                .collect(),
            membership_count: self.membership.entries().len(),
            position_seconds: self.position_seconds,
        }
    }
    pub fn check(&self, session: SessionId, revision: Counter) -> Result<(), Error> {
        if session != self.membership.session_id() || revision != self.revision {
            Err(Error::Conflict)
        } else {
            Ok(())
        }
    }
    pub fn selected(&self) -> Option<OccurrenceId> {
        self.selected
    }
    pub fn ended(&self) -> bool {
        self.ended
    }
    pub fn pending(&self) -> Vec<OccurrenceId> {
        self.membership
            .entries()
            .iter()
            .filter(|e| Some(e.id) != self.selected && !self.traversed.contains(&e.id))
            .map(|e| e.id)
            .collect()
    }
    pub fn visible_track(&self, id: OccurrenceId) -> Result<Track, Error> {
        if self.traversed.contains(&id) {
            return Err(Error::Missing);
        }
        self.metadata.get(&id).cloned().ok_or(Error::Missing)
    }
    pub fn enqueue(&mut self, track: Track, next: bool) -> Result<OccurrenceId, Error> {
        if self.membership.entries().len() >= LIMIT {
            return Err(Error::Limit);
        }
        let revision = self.revision.checked_next().map_err(|_| Error::Counter)?;
        let pending = self.pending();
        let before = if next {
            let after_priority = pending
                .iter()
                .rposition(|id| self.priority.contains(id))
                .map(|i| i + 1)
                .unwrap_or(0);
            pending.get(after_priority).copied()
        } else {
            None
        };
        let id = self.membership.commit_insert(
            self.membership.revision(),
            track.reference.clone(),
            before,
        )?;
        self.metadata.insert(id, track);
        self.revision = revision;
        if next {
            self.priority.push(id)
        }
        Ok(id)
    }
    pub fn replace(&mut self, track: Track) -> Result<OccurrenceId, Error> {
        let mut next = Self::default();
        let id = next.enqueue(track, false)?;
        next.select(id)?;
        *self = next;
        Ok(id)
    }
    pub fn select(&mut self, id: OccurrenceId) -> Result<Track, Error> {
        let track = self.visible_track(id)?;
        let revision = self.revision.checked_next().map_err(|_| Error::Counter)?;
        for entry in self.membership.entries() {
            if entry.id == id {
                break;
            }
            self.traversed.insert(entry.id);
        }
        self.revision = revision;
        self.selected = Some(id);
        self.position_seconds = 0.0;
        self.ended = false;
        let pending = self.pending();
        self.priority.retain(|id| pending.contains(id));
        Ok(track)
    }
    pub fn advance(&mut self) -> Result<Option<Track>, Error> {
        if let Some(id) = self.pending().first() {
            return self.select(*id).map(Some);
        }
        if !self.ended {
            self.revision = self.revision.checked_next().map_err(|_| Error::Counter)?;
        }
        self.ended = true;
        Ok(None)
    }
    pub fn replay(&mut self) -> Result<Option<Track>, Error> {
        self.revision.checked_next().map_err(|_| Error::Counter)?;
        let first = self.membership.entries().first().map(|e| e.id);
        if first.is_none() {
            return Ok(None);
        }
        self.traversed.clear();
        self.priority.clear();
        self.selected = None;
        self.ended = false;
        first.map(|id| self.select(id)).transpose()
    }
    pub fn move_before(
        &mut self,
        id: OccurrenceId,
        before: Option<OccurrenceId>,
    ) -> Result<(), Error> {
        self.visible_track(id)?;
        if Some(id) == self.selected {
            return Err(Error::Pinned);
        }
        if let Some(before) = before {
            self.visible_track(before)?;
            if Some(before) == self.selected {
                return Err(Error::Pinned);
            }
        }
        let revision = self.revision.checked_next().map_err(|_| Error::Counter)?;
        self.membership
            .commit_move(self.membership.revision(), id, before)?;
        self.priority.retain(|p| *p != id);
        self.revision = revision;
        Ok(())
    }
    pub fn remove(&mut self, id: OccurrenceId) -> Result<bool, Error> {
        self.visible_track(id)?;
        let selected = Some(id) == self.selected;
        let successor = if selected {
            self.pending().first().copied()
        } else {
            None
        };
        let revision = self.revision.checked_next().map_err(|_| Error::Counter)?;
        self.membership
            .commit_remove(self.membership.revision(), id)?;
        self.revision = revision;
        self.metadata.remove(&id);
        self.versions.remove(&id);
        self.priority.retain(|p| *p != id);
        if selected {
            self.selected = successor;
            self.position_seconds = 0.0;
            self.ended = successor.is_none();
            self.priority
                .retain(|p| Some(*p) != self.selected && !self.traversed.contains(p));
        }
        Ok(selected)
    }
    pub fn clear_upcoming(&mut self) -> Result<(), Error> {
        let ids = self.pending();
        if ids.is_empty() {
            return Ok(());
        }
        let revision = self.revision.checked_next().map_err(|_| Error::Counter)?;
        self.membership
            .commit_remove_many(self.membership.revision(), &ids)?;
        for id in ids {
            self.metadata.remove(&id);
            self.versions.remove(&id);
        }
        self.priority.clear();
        self.revision = revision;
        Ok(())
    }
    pub fn update_selected(&mut self, track: Track, version: &str) -> Result<(), Error> {
        let id = self.selected.ok_or(Error::Missing)?;
        if self.metadata[&id].reference != track.reference {
            return Err(Error::Missing);
        }
        if self.versions.get(&id).is_some_and(|saved| saved != version) {
            return Err(Error::Version);
        }
        if version.is_empty() || version.len() > 512 {
            return Err(Error::Version);
        }
        let revision = self.revision.checked_next().map_err(|_| Error::Counter)?;
        self.versions.insert(id, version.to_owned());
        self.metadata.insert(id, track);
        self.revision = revision;
        Ok(())
    }
}
impl<'de> Deserialize<'de> for Queue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            membership: QueueMembership,
            revision: Counter,
            metadata: HashMap<OccurrenceId, Track>,
            selected: Option<OccurrenceId>,
            traversed: HashSet<OccurrenceId>,
            priority: Vec<OccurrenceId>,
            ended: bool,
            position_seconds: f64,
            versions: HashMap<OccurrenceId, String>,
        }
        let w = Wire::deserialize(d)?;
        let entries = w.membership.entries();
        let ids: HashSet<_> = entries.iter().map(|e| e.id).collect();
        let invalid = || D::Error::custom("invalid saved session invariants");
        if entries.len() > LIMIT
            || w.metadata.len() != ids.len()
            || w.metadata.keys().any(|id| !ids.contains(id))
            || w.traversed.iter().any(|id| !ids.contains(id))
            || w.selected
                .is_some_and(|id| !ids.contains(&id) || w.traversed.contains(&id))
        {
            return Err(invalid());
        }
        for entry in entries {
            let track = w.metadata.get(&entry.id).ok_or_else(invalid)?;
            if track.reference != entry.track
                || [&track.title, &track.artist, &track.album]
                    .iter()
                    .any(|s| s.len() > 1024)
                || String::from(track.reference.key.clone()).len() > 512
            {
                return Err(invalid());
            }
        }
        let prefix: HashSet<_> = entries
            .iter()
            .take_while(|entry| Some(entry.id) != w.selected)
            .map(|e| e.id)
            .collect();
        if w.selected.is_some() && prefix != w.traversed {
            return Err(invalid());
        }
        if w.selected.is_none() && !w.traversed.is_empty() && w.traversed != ids {
            return Err(invalid());
        }
        let priority: HashSet<_> = w.priority.iter().copied().collect();
        if priority.len() != w.priority.len()
            || priority
                .iter()
                .any(|id| !ids.contains(id) || Some(*id) == w.selected || w.traversed.contains(id))
        {
            return Err(invalid());
        }
        if !w.position_seconds.is_finite()
            || !(0.0..=1.0e9).contains(&w.position_seconds)
            || w.selected.is_none() && w.position_seconds != 0.0
            || w.versions
                .iter()
                .any(|(id, v)| !ids.contains(id) || v.is_empty() || v.len() > 512)
        {
            return Err(invalid());
        }
        Ok(Self {
            membership: w.membership,
            revision: w.revision,
            metadata: w.metadata,
            selected: w.selected,
            traversed: w.traversed,
            priority: w.priority,
            ended: w.ended,
            position_seconds: w.position_seconds,
            versions: w.versions,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn track(name: &str) -> Track {
        Track {
            reference: qoda_core::TrackRef {
                source: qoda_core::SourceId::new(),
                key: name.to_owned().try_into().unwrap(),
            },
            title: name.into(),
            artist: "Fixture".into(),
            album: "Fixture".into(),
            duration_ms: None,
        }
    }
    fn titles(q: &Queue) -> Vec<String> {
        q.snapshot()
            .entries
            .iter()
            .map(|r| r.track.title.clone())
            .collect()
    }
    #[test]
    fn checkpoint_restores_identity_order_priority_versions_and_position() {
        let mut q = Queue::default();
        let x = q.replace(track("X")).unwrap();
        q.enqueue(track("Y"), false).unwrap();
        let a = q.enqueue(track("A"), true).unwrap();
        q.update_selected(q.visible_track(x).unwrap(), "media:part")
            .unwrap();
        q.set_position(42.5).unwrap();
        let mut restored: Queue =
            serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(restored.selected(), Some(x));
        assert_eq!(restored.position(), 42.5);
        assert_eq!(
            restored.update_selected(restored.visible_track(x).unwrap(), "different:part"),
            Err(Error::Version)
        );
        restored.enqueue(track("B"), true).unwrap();
        assert_eq!(titles(&restored), ["X", "A", "B", "Y"]);
        restored.remove(x).unwrap();
        assert_eq!(restored.selected(), Some(a));
        assert_eq!(restored.position(), 0.0);
        let again: Queue =
            serde_json::from_str(&serde_json::to_string(&restored).unwrap()).unwrap();
        assert_eq!(titles(&again), ["A", "B", "Y"]);
    }
    #[test]
    fn invalid_saved_references_and_positions_are_rejected() {
        let mut q = Queue::default();
        q.replace(track("X")).unwrap();
        let valid = serde_json::to_value(q).unwrap();
        for (field, value) in [
            ("selected", serde_json::json!(OccurrenceId::new())),
            ("position_seconds", serde_json::json!(-1)),
            ("position_seconds", serde_json::json!(1.0e20)),
            ("metadata", serde_json::json!({})),
        ] {
            let mut bad = valid.clone();
            bad[field] = value;
            assert!(serde_json::from_value::<Queue>(bad).is_err());
        }
    }
    #[test]
    fn duplicates_fifo_and_no_implicit_selection() {
        let mut q = Queue::default();
        let a = q.enqueue(track("same"), false).unwrap();
        let b = q.enqueue(q.visible_track(a).unwrap(), false).unwrap();
        assert_ne!(a, b);
        assert_eq!(q.selected(), None);
        q.replace(track("X")).unwrap();
        q.enqueue(track("Y"), false).unwrap();
        q.enqueue(track("A"), true).unwrap();
        q.enqueue(track("B"), true).unwrap();
        assert_eq!(titles(&q), ["X", "A", "B", "Y"]);
    }
    #[test]
    fn paused_intent_is_not_owned_or_started_by_edits() {
        let mut q = Queue::default();
        let x = q.replace(track("X")).unwrap();
        let y = q.enqueue(track("Y"), false).unwrap();
        assert!(q.remove(x).unwrap());
        assert_eq!(q.selected(), Some(y));
        q.clear_upcoming().unwrap();
        assert_eq!(q.selected(), Some(y));
        assert!(q.remove(y).unwrap());
        assert!(q.ended());
        assert!(q.snapshot().entries.is_empty());
    }
    #[test]
    fn traversed_membership_is_retained_without_resurrecting_removed_entries() {
        let mut q = Queue::default();
        let x = q.replace(track("X")).unwrap();
        let y = q.enqueue(track("Y"), false).unwrap();
        let z = q.enqueue(track("Z"), false).unwrap();
        q.advance().unwrap();
        assert_eq!(titles(&q), ["Y", "Z"]);
        assert!(q.visible_track(x).is_err());
        q.remove(z).unwrap();
        assert!(q.advance().unwrap().is_none());
        assert!(q.ended());
        q.enqueue(track("A"), false).unwrap();
        assert!(q.ended());
        assert_eq!(q.selected(), Some(y));
        assert_eq!(q.replay().unwrap().unwrap().title, "X");
        assert_eq!(titles(&q), ["X", "Y", "A"]);
    }
    #[test]
    fn edits_are_pinned_and_stale_session_revisions_fail() {
        let mut q = Queue::default();
        let x = q.replace(track("X")).unwrap();
        let snap = q.snapshot();
        let y = q.enqueue(track("Y"), false).unwrap();
        assert_eq!(q.check(snap.session, snap.revision), Err(Error::Conflict));
        assert_eq!(q.move_before(x, None), Err(Error::Pinned));
        assert_eq!(q.move_before(y, Some(x)), Err(Error::Pinned));
        q.replace(track("Z")).unwrap();
        assert_eq!(
            q.check(snap.session, q.snapshot().revision),
            Err(Error::Conflict)
        );
    }
    #[test]
    fn reorder_overrides_moved_play_next_priority() {
        let mut q = Queue::default();
        q.replace(track("X")).unwrap();
        q.enqueue(track("Y"), false).unwrap();
        q.enqueue(track("A"), true).unwrap();
        let b = q.enqueue(track("B"), true).unwrap();
        q.move_before(b, None).unwrap();
        q.enqueue(track("C"), true).unwrap();
        assert_eq!(titles(&q), ["X", "A", "C", "Y", "B"]);
    }
}
