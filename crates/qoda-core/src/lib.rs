//! Backend domain primitives. No I/O, provider credentials or UI authority.
//! Freshness tags are not authentication; the transport verifies local peer identity.
mod fence;
mod identity;
mod membership;
pub use fence::{ClockDisposition, OwnerTag, PreparationTicket, RuntimeFence};
pub use identity::{
    BackendInstanceId, Counter, CounterExhausted, OccurrenceId, SessionId, SourceId, VisitId,
};
pub use membership::{MembershipError, ProviderTrackKey, QueueEntry, QueueMembership, TrackRef};
