use crate::{BackendInstanceId, Counter, CounterExhausted, SessionId};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerTag {
    instance: BackendInstanceId,
    session: SessionId,
    output_epoch: Counter,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparationTicket {
    owner: OwnerTag,
    intent_generation: Counter,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockDisposition {
    Accepted,
    DuplicateOrOld,
    StaleOutput,
    NeedsSnapshot,
    CounterExhausted,
}
#[derive(Debug)]
pub struct RuntimeFence {
    owner: OwnerTag,
    intent_generation: Counter,
    sequence: Counter,
    highest_seen: Counter,
    needs_snapshot: bool,
}
impl RuntimeFence {
    pub fn new(session: SessionId) -> Self {
        Self {
            owner: OwnerTag {
                instance: BackendInstanceId::new(),
                session,
                output_epoch: Counter::ZERO,
            },
            intent_generation: Counter::ZERO,
            sequence: Counter::ZERO,
            highest_seen: Counter::ZERO,
            needs_snapshot: false,
        }
    }
    pub fn owner_tag(&self) -> OwnerTag {
        self.owner
    }
    pub fn preparation_ticket(&self) -> PreparationTicket {
        PreparationTicket {
            owner: self.owner,
            intent_generation: self.intent_generation,
        }
    }
    /// Checks freshness only; the coordinator also validates membership, media/access and gain bindings.
    pub fn accepts_preparation(&self, ticket: PreparationTicket) -> bool {
        ticket == self.preparation_ticket()
    }
    /// Pause invalidates automatic resume authority but retains real output evidence.
    pub fn advance_intent(&mut self) -> Result<(), CounterExhausted> {
        self.intent_generation = self.intent_generation.checked_next()?;
        Ok(())
    }
    /// Seek/reset/replacement atomically invalidate output and resume authority.
    pub fn invalidate_output(&mut self) -> Result<(), CounterExhausted> {
        let intent = self.intent_generation.checked_next()?;
        let epoch = self.owner.output_epoch.checked_next()?;
        self.intent_generation = intent;
        self.owner.output_epoch = epoch;
        self.sequence = Counter::ZERO;
        self.highest_seen = Counter::ZERO;
        self.needs_snapshot = false;
        Ok(())
    }
    pub fn observe_clock(&mut self, owner: OwnerTag, sequence: Counter) -> ClockDisposition {
        if owner != self.owner {
            return ClockDisposition::StaleOutput;
        }
        self.highest_seen = self.highest_seen.max(sequence);
        if self.needs_snapshot {
            return ClockDisposition::NeedsSnapshot;
        }
        if sequence <= self.sequence {
            return ClockDisposition::DuplicateOrOld;
        }
        let Ok(expected) = self.sequence.checked_next() else {
            return ClockDisposition::CounterExhausted;
        };
        if sequence != expected {
            self.needs_snapshot = true;
            return ClockDisposition::NeedsSnapshot;
        }
        self.sequence = sequence;
        ClockDisposition::Accepted
    }
    pub fn resynchronize(&mut self, owner: OwnerTag, sequence: Counter) -> ClockDisposition {
        if owner != self.owner {
            return ClockDisposition::StaleOutput;
        }
        if sequence < self.highest_seen {
            return ClockDisposition::NeedsSnapshot;
        }
        self.sequence = sequence;
        self.highest_seen = sequence;
        self.needs_snapshot = false;
        ClockDisposition::Accepted
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_rejects_old_preparation_without_erasing_real_output() {
        let mut fence = RuntimeFence::new(SessionId::new());
        let prep = fence.preparation_ticket();
        let output = fence.owner_tag();
        fence.advance_intent().unwrap();
        assert!(!fence.accepts_preparation(prep));
        assert_eq!(
            fence.observe_clock(output, Counter::new(1)),
            ClockDisposition::Accepted
        );
    }
    #[test]
    fn seek_and_backend_restart_reject_old_output() {
        let session = SessionId::new();
        let mut fence = RuntimeFence::new(session);
        let old = fence.owner_tag();
        fence.invalidate_output().unwrap();
        assert_eq!(
            fence.observe_clock(old, Counter::new(99)),
            ClockDisposition::StaleOutput
        );
        let mut restored = RuntimeFence::new(session);
        assert_eq!(
            restored.observe_clock(fence.owner_tag(), Counter::new(1)),
            ClockDisposition::StaleOutput
        );
    }
    #[test]
    fn failed_epoch_increment_is_atomic() {
        let mut fence = RuntimeFence::new(SessionId::new());
        fence.owner.output_epoch = Counter::new(u64::MAX);
        let tag = fence.owner_tag();
        let ticket = fence.preparation_ticket();
        assert_eq!(
            fence.observe_clock(tag, Counter::new(1)),
            ClockDisposition::Accepted
        );
        assert_eq!(fence.invalidate_output(), Err(CounterExhausted));
        assert_eq!(fence.owner_tag(), tag);
        assert_eq!(fence.preparation_ticket(), ticket);
        assert_eq!(
            fence.observe_clock(tag, Counter::new(2)),
            ClockDisposition::Accepted
        );
    }
    #[test]
    fn gap_requires_a_fresh_snapshot_before_accepting_further_progress() {
        let mut fence = RuntimeFence::new(SessionId::new());
        let tag = fence.owner_tag();
        assert_eq!(
            fence.observe_clock(tag, Counter::new(1)),
            ClockDisposition::Accepted
        );
        assert_eq!(
            fence.observe_clock(tag, Counter::new(9)),
            ClockDisposition::NeedsSnapshot
        );
        assert_eq!(
            fence.observe_clock(tag, Counter::new(2)),
            ClockDisposition::NeedsSnapshot
        );
        assert_eq!(
            fence.resynchronize(tag, Counter::new(8)),
            ClockDisposition::NeedsSnapshot
        );
        assert_eq!(
            fence.resynchronize(tag, Counter::new(9)),
            ClockDisposition::Accepted
        );
        assert_eq!(
            fence.observe_clock(tag, Counter::new(10)),
            ClockDisposition::Accepted
        );
        assert_eq!(
            fence.observe_clock(tag, Counter::new(9)),
            ClockDisposition::DuplicateOrOld
        );
    }
}
