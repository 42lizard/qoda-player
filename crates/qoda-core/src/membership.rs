use crate::{Counter, CounterExhausted, OccurrenceId, SessionId, SourceId};
use serde::{Deserialize, Deserializer, Serialize, de::Error};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderTrackKey(String);
impl TryFrom<String> for ProviderTrackKey {
    type Error = MembershipError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            Err(MembershipError::EmptyTrackKey)
        } else {
            Ok(Self(value))
        }
    }
}
impl From<ProviderTrackKey> for String {
    fn from(value: ProviderTrackKey) -> Self {
        value.0
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackRef {
    pub source: SourceId,
    pub key: ProviderTrackKey,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: OccurrenceId,
    pub track: TrackRef,
}

/// Ordered session membership, not the current/upcoming presentation projection.
/// The coordinator commits edits only after the required playback barrier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueueMembership {
    session: SessionId,
    revision: Counter,
    entries: Vec<QueueEntry>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MembershipError {
    EmptyTrackKey,
    RevisionConflict,
    MissingOccurrence,
    CounterExhausted,
}
impl std::fmt::Display for MembershipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MembershipError {}
impl From<CounterExhausted> for MembershipError {
    fn from(_: CounterExhausted) -> Self {
        Self::CounterExhausted
    }
}
impl Default for QueueMembership {
    fn default() -> Self {
        Self::new()
    }
}
impl QueueMembership {
    pub fn new() -> Self {
        Self {
            session: SessionId::new(),
            revision: Counter::ZERO,
            entries: Vec::new(),
        }
    }
    pub fn session_id(&self) -> SessionId {
        self.session
    }
    pub fn revision(&self) -> Counter {
        self.revision
    }
    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }
    pub fn append(&mut self, track: TrackRef) -> Result<OccurrenceId, MembershipError> {
        self.commit_insert(self.revision, track, None)
    }
    pub fn commit_insert(
        &mut self,
        expected: Counter,
        track: TrackRef,
        before: Option<OccurrenceId>,
    ) -> Result<OccurrenceId, MembershipError> {
        self.check_revision(expected)?;
        let index = match before {
            Some(id) => self.index(id)?,
            None => self.entries.len(),
        };
        let revision = self.revision.checked_next()?;
        let id = OccurrenceId::new();
        self.entries.insert(index, QueueEntry { id, track });
        self.revision = revision;
        Ok(id)
    }
    pub fn commit_move(
        &mut self,
        expected: Counter,
        id: OccurrenceId,
        before: Option<OccurrenceId>,
    ) -> Result<(), MembershipError> {
        self.check_revision(expected)?;
        let index = self.index(id)?;
        let target = match before {
            Some(id) => self.index(id)?,
            None => self.entries.len(),
        };
        if target == index || target == index + 1 {
            return Ok(());
        }
        let revision = self.revision.checked_next()?;
        let entry = self.entries.remove(index);
        self.entries
            .insert(if target > index { target - 1 } else { target }, entry);
        self.revision = revision;
        Ok(())
    }
    pub fn commit_remove_many(
        &mut self,
        expected: Counter,
        ids: &[OccurrenceId],
    ) -> Result<(), MembershipError> {
        self.check_revision(expected)?;
        for id in ids {
            self.index(*id)?;
        }
        if ids.is_empty() {
            return Ok(());
        }
        let revision = self.revision.checked_next()?;
        let ids: HashSet<_> = ids.iter().collect();
        self.entries.retain(|entry| !ids.contains(&entry.id));
        self.revision = revision;
        Ok(())
    }
    fn check_revision(&self, expected: Counter) -> Result<(), MembershipError> {
        if expected != self.revision {
            Err(MembershipError::RevisionConflict)
        } else {
            Ok(())
        }
    }
    fn index(&self, id: OccurrenceId) -> Result<usize, MembershipError> {
        self.entries
            .iter()
            .position(|e| e.id == id)
            .ok_or(MembershipError::MissingOccurrence)
    }
    pub fn commit_remove(
        &mut self,
        expected: Counter,
        id: OccurrenceId,
    ) -> Result<(), MembershipError> {
        self.commit_remove_many(expected, &[id])
    }
}
impl<'de> Deserialize<'de> for QueueMembership {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            session: SessionId,
            revision: Counter,
            entries: Vec<QueueEntry>,
        }
        let raw = Wire::deserialize(deserializer)?;
        let mut ids = HashSet::new();
        if raw.entries.iter().any(|entry| !ids.insert(entry.id)) {
            return Err(D::Error::custom("duplicate occurrence identity"));
        }
        Ok(Self {
            session: raw.session,
            revision: raw.revision,
            entries: raw.entries,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> TrackRef {
        TrackRef {
            source: SourceId::new(),
            key: "fixture:track:1".to_owned().try_into().unwrap(),
        }
    }
    #[test]
    fn edits_are_atomic_and_anchored_by_identity() {
        let mut q = QueueMembership::new();
        let a = q.append(track()).unwrap();
        let b = q.append(track()).unwrap();
        let c = q.commit_insert(q.revision(), track(), Some(b)).unwrap();
        assert_eq!(
            q.entries().iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![a, c, b]
        );
        q.commit_move(q.revision(), b, Some(a)).unwrap();
        assert_eq!(q.entries()[0].id, b);
        let before = q.clone();
        assert!(
            q.commit_remove_many(q.revision(), &[a, OccurrenceId::new()])
                .is_err()
        );
        assert_eq!(q, before);
        assert!(q.commit_move(Counter::ZERO, a, None).is_err());
        assert_eq!(q, before);
        q.commit_remove_many(q.revision(), &[a, c]).unwrap();
        assert_eq!(q.entries().len(), 1);
    }
    #[test]
    fn duplicate_tracks_keep_independent_occurrences_across_restore() {
        let mut queue = QueueMembership::new();
        let track = track();
        let first = queue.append(track.clone()).unwrap();
        let second = queue.append(track.clone()).unwrap();
        assert_ne!(first, second);
        let mut restored: QueueMembership =
            serde_json::from_str(&serde_json::to_string(&queue).unwrap()).unwrap();
        assert_eq!(restored, queue);
        let third = restored.append(track).unwrap();
        assert_ne!(third, first);
        assert_ne!(third, second);
        restored.commit_remove(restored.revision(), second).unwrap();
        assert_eq!(
            restored.entries().iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![first, third]
        );
    }
    #[test]
    fn stale_edit_does_not_change_membership() {
        let mut queue = QueueMembership::new();
        let first = queue.append(track()).unwrap();
        let stale = queue.revision();
        queue.append(track()).unwrap();
        let before = queue.clone();
        assert_eq!(
            queue.commit_remove(stale, first),
            Err(MembershipError::RevisionConflict)
        );
        assert_eq!(queue, before);
    }
    #[test]
    fn corrupt_persisted_duplicate_occurrences_are_rejected() {
        let mut queue = QueueMembership::new();
        queue.append(track()).unwrap();
        let mut value = serde_json::to_value(queue).unwrap();
        let first = value["entries"][0].clone();
        value["entries"].as_array_mut().unwrap().push(first);
        assert!(serde_json::from_value::<QueueMembership>(value).is_err());
    }
}
