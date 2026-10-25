//! Session membership and its selected/upcoming projection. Traversed entries
//! are retained as membership, not claimed as listening history.
use qoda_core::{Counter, MembershipError, OccurrenceId, QueueMembership, SessionId};
use qoda_provider::Track;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::collections::{HashMap, HashSet};
const LIMIT: usize = 500;
const NAVIGATION_LIMIT: usize = 4096;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Off,
    One,
    All,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Conflict,
    Missing,
    Pinned,
    Limit,
    Counter,
    Version,
    Unconfirmed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self{
  Self::Conflict=>"The queue changed. Review its current order and retry.",Self::Missing=>"That occurrence is no longer in the current or upcoming queue.",Self::Pinned=>"The selected entry and tracks in Previous navigation stay pinned; move ordinary upcoming entries instead.",Self::Limit=>"This development session supports at most 500 occurrences. Start a new session or remove entries.",Self::Counter=>"The queue revision cannot advance. Start a new session.",Self::Version=>"The selected media version changed. Play it as new content to choose the new version.",Self::Unconfirmed=>"Automatic repeat paused because this visit had no confirmed nominal audio clock. Choose Play or Replay to retry.",})
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
    pub can_move: bool,
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
    pub shuffled: bool,
    pub repeat: Repeat,
    pub previous_available: bool,
    pub navigating: bool,
    pub navigation_dropped: Counter,
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
    order: Vec<OccurrenceId>,
    shuffled: bool,
    repeat: Repeat,
    route: Vec<OccurrenceId>,
    cursor: Option<usize>,
    navigating: bool,
    recorded: bool,
    navigation_dropped: Counter,
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
            order: Vec::new(),
            shuffled: false,
            repeat: Repeat::Off,
            route: Vec::new(),
            cursor: None,
            navigating: false,
            recorded: false,
            navigation_dropped: Counter::ZERO,
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
    fn next_revision(&self) -> Result<Counter, Error> {
        self.revision.checked_next().map_err(|_| Error::Counter)
    }
    pub fn empty_session(&self) -> Self {
        Self {
            shuffled: self.shuffled,
            repeat: self.repeat,
            ..Self::default()
        }
    }
    pub fn seek_to(&mut self, seconds: f64) -> Result<(), Error> {
        let revision = if self.ended {
            Some(self.next_revision()?)
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
        let forward = self.forward_route();
        let entries = self
            .selected
            .into_iter()
            .chain(self.pending())
            .map(|id| Row {
                id,
                track: self.metadata[&id].clone(),
                selected: Some(id) == self.selected,
                can_move: Some(id) != self.selected && !forward.contains(&id),
            })
            .collect();
        Snapshot {
            session: self.membership.session_id(),
            revision: self.revision,
            selected: self.selected,
            ended: self.ended,
            entries,
            membership_count: self.membership.entries().len(),
            position_seconds: self.position_seconds,
            shuffled: self.shuffled,
            repeat: self.repeat,
            previous_available: self.selected.is_some(),
            navigating: self.navigating,
            navigation_dropped: self.navigation_dropped,
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
    fn forward_route(&self) -> Vec<OccurrenceId> {
        if !self.navigating {
            return Vec::new();
        }
        self.cursor
            .map(|cursor| {
                self.route[cursor + 1..]
                    .iter()
                    .copied()
                    .filter(|id| Some(*id) != self.selected && self.metadata.contains_key(id))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn ordinary_pending(&self) -> Vec<OccurrenceId> {
        self.order
            .iter()
            .copied()
            .filter(|id| Some(*id) != self.selected && !self.traversed.contains(id))
            .collect()
    }
    pub fn pending(&self) -> Vec<OccurrenceId> {
        let mut result = Vec::new();
        for id in self
            .forward_route()
            .into_iter()
            .chain(self.ordinary_pending())
        {
            if !result.contains(&id) {
                result.push(id)
            }
        }
        result
    }
    pub fn visible_track(&self, id: OccurrenceId) -> Result<Track, Error> {
        if Some(id) != self.selected && !self.pending().contains(&id) {
            return Err(Error::Missing);
        }
        self.metadata.get(&id).cloned().ok_or(Error::Missing)
    }
    pub fn enqueue(&mut self, track: Track, next: bool) -> Result<OccurrenceId, Error> {
        if self.membership.entries().len() >= LIMIT {
            return Err(Error::Limit);
        }
        let revision = self.next_revision()?;
        let pending = self.ordinary_pending();
        let before = if next {
            pending
                .get(
                    pending
                        .iter()
                        .rposition(|id| self.priority.contains(id))
                        .map(|i| i + 1)
                        .unwrap_or(0),
                )
                .copied()
        } else {
            None
        };
        let id = self.membership.commit_insert(
            self.membership.revision(),
            track.reference.clone(),
            before,
        )?;
        let index = before
            .and_then(|anchor| self.order.iter().position(|p| *p == anchor))
            .unwrap_or(self.order.len());
        self.order.insert(index, id);
        self.metadata.insert(id, track);
        if next {
            self.priority.push(id)
        }
        self.revision = revision;
        Ok(id)
    }
    pub fn replace(&mut self, track: Track) -> Result<OccurrenceId, Error> {
        let mut next = self.empty_session();
        let id = next.enqueue(track, false)?;
        next.select(id)?;
        *self = next;
        Ok(id)
    }
    pub fn select(&mut self, id: OccurrenceId) -> Result<Track, Error> {
        let track = self.visible_track(id)?;
        let revision = self.next_revision()?;
        self.traversed = self
            .order
            .iter()
            .take_while(|entry| **entry != id)
            .copied()
            .collect();
        self.selected = Some(id);
        self.position_seconds = 0.0;
        self.ended = false;
        self.navigating = false;
        self.recorded = false;
        let pending = self.ordinary_pending();
        self.priority.retain(|id| pending.contains(id));
        self.revision = revision;
        Ok(track)
    }
    // Called only for a fresh, positive nominal audio clock for this selection.
    // Navigation through this route does not append entries and bounce Previous.
    pub fn confirm_presented(&mut self) -> Result<bool, Error> {
        let Some(id) = self.selected else {
            return Ok(false);
        };
        if self.recorded || self.ended {
            return Ok(false);
        }
        let revision = self.next_revision()?;
        if !self.navigating {
            if let Some(cursor) = self.cursor {
                self.route.truncate(cursor + 1);
            }
            if self.route.len() == NAVIGATION_LIMIT {
                self.navigation_dropped = self
                    .navigation_dropped
                    .checked_next()
                    .map_err(|_| Error::Counter)?;
                self.route.remove(0);
            }
            self.route.push(id);
            self.cursor = Some(self.route.len() - 1);
        }
        self.recorded = true;
        self.revision = revision;
        Ok(true)
    }
    fn navigate(&mut self, cursor: usize) -> Result<Track, Error> {
        let id = *self.route.get(cursor).ok_or(Error::Missing)?;
        let track = self.metadata.get(&id).cloned().ok_or(Error::Missing)?;
        let revision = self.next_revision()?;
        self.selected = Some(id);
        self.cursor = Some(cursor);
        self.navigating = true;
        self.recorded = true;
        self.position_seconds = 0.0;
        self.ended = false;
        self.revision = revision;
        Ok(track)
    }
    pub fn previous(&mut self) -> Result<Option<Track>, Error> {
        let Some(id) = self.selected else {
            return Ok(None);
        };
        if self.position_seconds > 3.0 {
            let track = self.metadata.get(&id).cloned().ok_or(Error::Missing)?;
            self.revision = self.next_revision()?;
            self.position_seconds = 0.0;
            self.ended = false;
            return Ok(Some(track));
        }
        let target = self.cursor.and_then(|cursor| {
            if self.recorded {
                cursor.checked_sub(1)
            } else {
                Some(cursor)
            }
        });
        if let Some(cursor) = target {
            return self.navigate(cursor).map(Some);
        }
        let track = self.metadata.get(&id).cloned().ok_or(Error::Missing)?;
        self.revision = self.next_revision()?;
        self.position_seconds = 0.0;
        self.ended = false;
        Ok(Some(track))
    }
    pub fn advance(&mut self) -> Result<Option<Track>, Error> {
        if self.navigating
            && let Some(cursor) = self.cursor
            && cursor + 1 < self.route.len()
        {
            return self.navigate(cursor + 1).map(Some);
        }
        // At the navigation frontier, resume ordinary pending order.
        if self.navigating {
            self.navigating = false;
            if let Some(id) = self.selected {
                self.traversed.remove(&id);
            }
        }
        if let Some(id) = self.ordinary_pending().first() {
            return self.select(*id).map(Some);
        }
        if self.repeat == Repeat::All && !self.order.is_empty() {
            return self.restart_cycle();
        }
        if !self.ended {
            self.revision = self.next_revision()?;
        }
        self.ended = true;
        Ok(None)
    }
    pub fn complete(&mut self, presented: bool) -> Result<Option<Track>, Error> {
        if !presented
            && (self.repeat == Repeat::One
                || self.repeat == Repeat::All && self.pending().is_empty())
        {
            return Err(Error::Unconfirmed);
        }
        if self.repeat == Repeat::One
            && let Some(id) = self.selected
        {
            let track = self.metadata[&id].clone();
            let revision = self.next_revision()?;
            self.traversed = self
                .order
                .iter()
                .take_while(|entry| **entry != id)
                .copied()
                .collect();
            self.position_seconds = 0.0;
            self.ended = false;
            self.navigating = false;
            self.recorded = false;
            self.revision = revision;
            return Ok(Some(track));
        }
        self.advance()
    }
    fn restart_cycle(&mut self) -> Result<Option<Track>, Error> {
        let Some(id) = self.order.first().copied() else {
            return Ok(None);
        };
        self.next_revision()?;
        self.traversed.clear();
        self.selected = None;
        self.navigating = false;
        self.select(id).map(Some)
    }
    pub fn replay(&mut self) -> Result<Option<Track>, Error> {
        self.restart_cycle()
    }
    pub fn set_repeat(&mut self, mode: Repeat) -> Result<(), Error> {
        if self.repeat != mode {
            self.revision = self.next_revision()?;
            self.repeat = mode;
        }
        Ok(())
    }
    pub fn set_shuffle(&mut self, enabled: bool) -> Result<(), Error> {
        self.shuffle_with(enabled, uuid::Uuid::new_v4)
    }
    fn shuffle_with(
        &mut self,
        enabled: bool,
        mut key: impl FnMut() -> uuid::Uuid,
    ) -> Result<(), Error> {
        if self.shuffled == enabled {
            return Ok(());
        }
        let revision = self.next_revision()?;
        let pending = self.ordinary_pending();
        let forward = self.forward_route();
        let fixed: HashSet<_> = self
            .order
            .iter()
            .copied()
            .filter(|id| !pending.contains(id) || forward.contains(id))
            .collect();
        let mut priority: Vec<_> = pending
            .iter()
            .copied()
            .filter(|id| self.priority.contains(id) && !fixed.contains(id))
            .collect();
        let mut rest: Vec<_> = self
            .membership
            .entries()
            .iter()
            .map(|e| e.id)
            .filter(|id| !fixed.contains(id) && !priority.contains(id))
            .collect();
        if enabled {
            rest.sort_by_cached_key(|_| key());
        }
        let mut order: Vec<_> = self
            .order
            .iter()
            .copied()
            .filter(|id| fixed.contains(id))
            .collect();
        order.append(&mut priority);
        order.append(&mut rest);
        self.order = order;
        self.shuffled = enabled;
        self.revision = revision;
        Ok(())
    }
    pub fn move_before(
        &mut self,
        id: OccurrenceId,
        before: Option<OccurrenceId>,
    ) -> Result<(), Error> {
        self.visible_track(id)?;
        let forward = self.forward_route();
        if Some(id) == self.selected || forward.contains(&id) {
            return Err(Error::Pinned);
        }
        if let Some(anchor) = before {
            self.visible_track(anchor)?;
            if Some(anchor) == self.selected || forward.contains(&anchor) {
                return Err(Error::Pinned);
            }
        }
        let revision = self.next_revision()?;
        self.membership
            .commit_move(self.membership.revision(), id, before)?;
        self.order.retain(|p| *p != id);
        let index = before
            .and_then(|anchor| self.order.iter().position(|p| *p == anchor))
            .unwrap_or(self.order.len());
        self.order.insert(index, id);
        self.priority.retain(|p| *p != id);
        if before.is_some_and(|anchor| self.priority.contains(&anchor)) {
            self.priority.push(id)
        }
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
        let revision = self.next_revision()?;
        self.membership
            .commit_remove(self.membership.revision(), id)?;
        let old_cursor = self.cursor;
        self.cursor = old_cursor.and_then(|cursor| {
            let count = self.route[..=cursor]
                .iter()
                .filter(|entry| **entry != id)
                .count();
            count.checked_sub(1)
        });
        self.route.retain(|entry| *entry != id);
        if self.cursor.is_none() {
            self.route.clear();
        }
        self.order.retain(|entry| *entry != id);
        self.traversed.remove(&id);
        self.metadata.remove(&id);
        self.versions.remove(&id);
        self.priority.retain(|entry| *entry != id);
        if selected {
            self.selected = successor;
            self.position_seconds = 0.0;
            self.ended = successor.is_none();
            self.navigating = false;
            self.recorded = false;
            if let Some(next) = successor {
                self.traversed = self
                    .order
                    .iter()
                    .take_while(|entry| **entry != next)
                    .copied()
                    .collect();
            }
        }
        let pending = self.ordinary_pending();
        self.priority.retain(|id| pending.contains(id));
        self.revision = revision;
        Ok(selected)
    }
    pub fn clear_upcoming(&mut self) -> Result<(), Error> {
        let ids = self.pending();
        if ids.is_empty() {
            return Ok(());
        }
        self.next_revision()?;
        // Stage the batch so any counter failure leaves the original coherent.
        let mut next = self.clone();
        for id in ids {
            next.remove(id)?;
        }
        *self = next;
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
impl Queue {
    pub fn from_legacy(mut value: serde_json::Value) -> Result<Self, serde_json::Error> {
        let order = value["membership"]["entries"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry["id"].clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(fields) = value.as_object_mut() {
            for key in [
                "order",
                "shuffled",
                "repeat",
                "route",
                "cursor",
                "navigating",
                "recorded",
                "navigation_dropped",
            ] {
                if fields.contains_key(key) {
                    return Err(<serde_json::Error as serde::de::Error>::custom(
                        "unexpected legacy session fields",
                    ));
                }
            }
            fields.insert("order".into(), serde_json::json!(order));
            fields.insert("shuffled".into(), serde_json::json!(false));
            fields.insert("repeat".into(), serde_json::json!("off"));
            fields.insert("route".into(), serde_json::json!([]));
            fields.insert("cursor".into(), serde_json::Value::Null);
            fields.insert("navigating".into(), serde_json::json!(false));
            fields.insert("recorded".into(), serde_json::json!(false));
            fields.insert("navigation_dropped".into(), serde_json::json!("0"));
        }
        serde_json::from_value(value)
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
            order: Vec<OccurrenceId>,
            shuffled: bool,
            repeat: Repeat,
            route: Vec<OccurrenceId>,
            cursor: Option<usize>,
            navigating: bool,
            recorded: bool,
            navigation_dropped: Counter,
        }
        let w = Wire::deserialize(d)?;
        let entries = w.membership.entries();
        let ids: HashSet<_> = entries.iter().map(|e| e.id).collect();
        let invalid = || D::Error::custom("invalid saved session invariants");
        if entries.len() > LIMIT
            || w.metadata.len() != ids.len()
            || w.metadata.keys().any(|id| !ids.contains(id))
            || w.traversed.iter().any(|id| !ids.contains(id))
            || w.selected.is_some_and(|id| !ids.contains(&id))
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
        let order: HashSet<_> = w.order.iter().copied().collect();
        if w.order.len() != ids.len()
            || order != ids
            || w.route.len() > NAVIGATION_LIMIT
            || w.route.iter().any(|id| !ids.contains(id))
            || w.cursor.is_some_and(|cursor| cursor >= w.route.len())
            || w.route.is_empty() != w.cursor.is_none()
        {
            return Err(invalid());
        }
        if w.navigating && (!w.recorded || w.cursor.map(|cursor| w.route[cursor]) != w.selected) {
            return Err(invalid());
        }
        if !w.navigating && w.selected.is_some() {
            let prefix: HashSet<_> = w
                .order
                .iter()
                .take_while(|id| Some(**id) != w.selected)
                .copied()
                .collect();
            if prefix != w.traversed {
                return Err(invalid());
            }
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
            order: w.order,
            shuffled: w.shuffled,
            repeat: w.repeat,
            route: w.route,
            cursor: w.cursor,
            navigating: w.navigating,
            recorded: w.recorded,
            navigation_dropped: w.navigation_dropped,
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
    fn observed(q: &mut Queue) {
        q.confirm_presented().unwrap();
    }
    #[test]
    fn repeat_does_not_retry_an_unconfirmed_visit_and_modes_survive_session_replacement() {
        let mut q = Queue::default();
        q.replace(track("A")).unwrap();
        q.set_repeat(Repeat::One).unwrap();
        assert_eq!(q.complete(false).err(), Some(Error::Unconfirmed));
        q.set_repeat(Repeat::All).unwrap();
        assert_eq!(q.complete(false).err(), Some(Error::Unconfirmed));
        q.set_shuffle(true).unwrap();
        q.replace(track("B")).unwrap();
        assert!(q.snapshot().shuffled);
        assert_eq!(q.snapshot().repeat, Repeat::All);
        let cleared = q.empty_session();
        assert!(cleared.snapshot().shuffled);
        assert_eq!(cleared.snapshot().repeat, Repeat::All);
    }
    #[test]
    fn previous_threshold_repeated_back_and_forward_do_not_bounce() {
        let mut q = Queue::default();
        q.replace(track("A")).unwrap();
        q.enqueue(track("B"), false).unwrap();
        q.enqueue(track("C"), false).unwrap();
        observed(&mut q);
        q.advance().unwrap();
        observed(&mut q);
        q.advance().unwrap();
        observed(&mut q);
        q.set_position(3.1).unwrap();
        let before = q.snapshot().revision;
        assert_eq!(q.previous().unwrap().unwrap().title, "C");
        assert_ne!(before, q.snapshot().revision);
        assert_eq!(q.position(), 0.0);
        assert_eq!(q.previous().unwrap().unwrap().title, "B");
        observed(&mut q);
        assert_eq!(q.previous().unwrap().unwrap().title, "A");
        observed(&mut q);
        assert_eq!(q.advance().unwrap().unwrap().title, "B");
        assert_eq!(q.advance().unwrap().unwrap().title, "C");
    }
    #[test]
    fn unpresented_selections_do_not_become_previous_history() {
        let mut q = Queue::default();
        q.replace(track("A")).unwrap();
        let b = q.enqueue(track("B"), false).unwrap();
        let c = q.enqueue(track("C"), false).unwrap();
        observed(&mut q);
        q.select(b).unwrap();
        q.select(c).unwrap();
        observed(&mut q);
        assert_eq!(q.previous().unwrap().unwrap().title, "A");
        assert_eq!(q.advance().unwrap().unwrap().title, "C");
    }
    #[test]
    fn repeat_one_only_applies_to_natural_completion_and_all_keeps_order() {
        let mut q = Queue::default();
        q.replace(track("A")).unwrap();
        q.enqueue(track("B"), false).unwrap();
        q.set_repeat(Repeat::One).unwrap();
        assert_eq!(q.complete(true).unwrap().unwrap().title, "A");
        assert_eq!(q.advance().unwrap().unwrap().title, "B");
        q.set_repeat(Repeat::All).unwrap();
        assert_eq!(q.complete(true).unwrap().unwrap().title, "A");
        assert_eq!(q.complete(true).unwrap().unwrap().title, "B");
        q.set_repeat(Repeat::Off).unwrap();
        assert!(q.complete(true).unwrap().is_none());
    }
    #[test]
    fn shuffle_is_stable_and_unshuffle_keeps_deliberate_edits_and_fifo() {
        let mut q = Queue::default();
        let x = q.replace(track("X")).unwrap();
        let a = q.enqueue(track("A"), false).unwrap();
        let b = q.enqueue(track("B"), false).unwrap();
        let c = q.enqueue(track("C"), false).unwrap();
        let mut seed = 10_u128;
        q.shuffle_with(true, || {
            seed -= 1;
            uuid::Uuid::from_u128(seed)
        })
        .unwrap();
        assert_eq!(titles(&q), ["X", "C", "B", "A"]);
        assert_eq!(q.selected(), Some(x));
        let copy: Queue = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(titles(&copy), titles(&q));
        q.move_before(c, Some(a)).unwrap();
        q.remove(b).unwrap();
        q.enqueue(track("D"), false).unwrap();
        q.enqueue(track("P"), true).unwrap();
        q.enqueue(track("Q"), true).unwrap();
        q.set_shuffle(false).unwrap();
        assert_eq!(titles(&q), ["X", "P", "Q", "C", "A", "D"]);
    }
    #[test]
    fn removed_and_cleared_entries_do_not_return_in_repeat_all() {
        let mut q = Queue::default();
        let a = q.replace(track("A")).unwrap();
        let b = q.enqueue(track("B"), false).unwrap();
        q.enqueue(track("C"), false).unwrap();
        observed(&mut q);
        q.advance().unwrap();
        observed(&mut q);
        q.clear_upcoming().unwrap();
        q.remove(b).unwrap();
        q.set_repeat(Repeat::All).unwrap();
        assert_eq!(
            q.advance().unwrap().unwrap().reference,
            q.metadata[&a].reference
        );
        assert_eq!(q.snapshot().membership_count, 1);
        assert_eq!(q.empty_session().snapshot().repeat, Repeat::All);
    }
    #[test]
    fn navigation_and_modes_roundtrip_without_fabricating_legacy_visits() {
        let mut q = Queue::default();
        q.replace(track("A")).unwrap();
        q.enqueue(track("B"), false).unwrap();
        observed(&mut q);
        q.advance().unwrap();
        observed(&mut q);
        q.previous().unwrap();
        q.set_repeat(Repeat::All).unwrap();
        q.set_shuffle(true).unwrap();
        let mut copy: Queue = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(copy.advance().unwrap().unwrap().title, "B");
        assert_eq!(copy.snapshot().repeat, Repeat::All);
    }
    #[test]
    fn mixed_navigation_edits_and_mode_changes_always_roundtrip() {
        let mut q = Queue::default();
        let mut seed = 7_u64;
        for i in 0..1000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let pending = q.pending();
            match seed % 12 {
                0 => {
                    let _ = q.enqueue(track(&format!("T{i}")), false);
                }
                1 => {
                    let _ = q.enqueue(track(&format!("N{i}")), true);
                }
                2 => {
                    let _ = q.advance();
                }
                3 => {
                    let _ = q.complete(true);
                }
                4 => {
                    let _ = q.previous();
                }
                5 => {
                    let _ = q.confirm_presented();
                }
                6 => {
                    let _ = q.set_shuffle(!q.shuffled);
                }
                7 => {
                    let _ = q.set_repeat(
                        [Repeat::Off, Repeat::One, Repeat::All][(seed as usize / 13) % 3],
                    );
                }
                8 => {
                    if let Some(id) = pending.first() {
                        let _ = q.remove(*id);
                    }
                }
                9 => {
                    if let Some(id) = pending.last() {
                        let _ = q.move_before(*id, pending.first().copied());
                    }
                }
                10 => {
                    let _ = q.seek_to(2.0);
                }
                _ => {
                    if q.selected.is_none()
                        && let Some(id) = pending.first()
                    {
                        let _ = q.select(*id);
                    }
                }
            }
            let value = serde_json::to_string(&q).unwrap();
            let copy: Queue = serde_json::from_str(&value).unwrap_or_else(|e| {
                panic!(
                    "invalid state at step {i}, action {}: {e}; {value}",
                    seed % 12
                )
            });
            assert_eq!(
                serde_json::to_value(q.snapshot()).unwrap(),
                serde_json::to_value(copy.snapshot()).unwrap()
            );
        }
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
