//! Provider-neutral, backend-only timeline data. Never a UI request DTO.
use qoda_core::TrackRef;
use std::num::NonZeroU64;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Playing,
    Paused,
    Stopped,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub track: TrackRef,
    /// A private listening identifier, independent of authentication identity.
    pub session: Uuid,
    pub state: State,
    pub position_ms: u64,
    pub duration_ms: NonZeroU64,
    pub continuing: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// Deliberately omits server-controlled text and identifiers.
    ServerTermination,
}
