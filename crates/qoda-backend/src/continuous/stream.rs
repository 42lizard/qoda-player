//! One current/one future PCM admission, byte transport and nominal frame ledger.
//! None of the input types can be deserialized from UI/provider URLs.
use super::description::Description;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
const CAPACITY: usize = 65536;
const FRAME_BYTES: u64 = 16;
#[derive(Clone)]
pub struct Input {
    pub generation: u64,
    pub uri: String,
    pub description: Description,
    pub gain: f64,
}
impl Input {
    fn valid(&self, minimum: u64) -> bool {
        let d = self.description;
        self.generation > 0
            && !self.uri.is_empty()
            && self.uri.len() <= 8192
            && !self.uri.contains('\0')
            && matches!(d.rate, 44100 | 48000 | 96000)
            && matches!(d.bits, 16 | 24)
            && d.frames >= minimum
            && d.frames > 0
            && d.frames <= (1_u64 << 52)
            && d.frames.checked_mul(FRAME_BYTES).is_some()
            && self.gain.is_finite()
            && (0.0..=1_000_000.0).contains(&self.gain)
    }
}
struct Slot {
    input: Input,
    start: u64,
    claimed: bool,
    decoded: bool,
    written: u64,
}
impl Slot {
    fn new(input: Input, start: u64) -> Self {
        Self {
            input,
            start,
            claimed: false,
            decoded: false,
            written: 0,
        }
    }
    fn end(&self) -> Option<u64> {
        self.start.checked_add(self.input.description.frames)
    }
}
struct State {
    current: Slot,
    highest_generation: u64,
    future: Option<Slot>,
    bytes: VecDeque<u8>,
    sealed: bool,
    finished: bool,
    handoff: Option<u64>,
    error: Option<&'static str>,
    fallback: Option<&'static str>,
}
impl State {
    fn reject_uncommitted(&mut self, generation: u64, reason: &'static str) -> bool {
        if !self
            .future
            .as_ref()
            .is_some_and(|future| future.input.generation == generation && future.written == 0)
        {
            return false;
        }
        self.future = None;
        self.sealed = true;
        self.fallback = Some(reason);
        true
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    Untouched,
    Committed,
    Stale,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    pub generation: u64,
    pub position: Option<f64>,
    pub handoff_from: Option<u64>,
    pub staged_generation: Option<u64>,
}
pub struct Stream {
    state: Mutex<State>,
    changed: Condvar,
    cancelled: AtomicBool,
    minimum: u64,
    writer_active: AtomicBool,
    pub high_water: AtomicUsize,
}
impl Stream {
    pub fn new(input: Input, minimum: u64) -> Result<Arc<Self>, &'static str> {
        if !input.valid(minimum) {
            return Err("Unqualified continuous source");
        }
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                highest_generation: input.generation,
                current: Slot::new(input, 0),
                future: None,
                bytes: VecDeque::new(),
                sealed: false,
                finished: false,
                handoff: None,
                error: None,
                fallback: None,
            }),
            changed: Condvar::new(),
            cancelled: AtomicBool::new(false),
            minimum,
            writer_active: AtomicBool::new(false),
            high_water: AtomicUsize::new(0),
        }))
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.changed.notify_all();
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn fail(&self, message: &'static str) {
        if let Ok(mut state) = self.state.lock() {
            state.error.get_or_insert(message);
        }
        self.cancel();
    }
    pub fn finished(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.finished)
    }
    pub fn error(&self) -> Option<&'static str> {
        self.state
            .lock()
            .map_or(Some("Continuous input unavailable"), |s| s.error)
    }
    pub fn live(&self, generation: u64) -> bool {
        !self.cancelled()
            && self.state.lock().is_ok_and(|s| {
                s.current.input.generation == generation
                    || s.future
                        .as_ref()
                        .is_some_and(|f| f.input.generation == generation)
            })
    }
    pub fn owner(&self) -> Result<u64, &'static str> {
        self.state
            .lock()
            .map(|s| s.current.input.generation)
            .map_err(|_| "Continuous input unavailable")
    }
    pub fn fallback(&self) -> Option<&'static str> {
        self.state.lock().ok().and_then(|s| s.fallback)
    }
    /// A failed speculative decoder cannot interrupt current if none of its
    /// bytes entered the shared final input. Ending the group is irreversible.
    pub fn reject_uncommitted(&self, generation: u64, reason: &'static str) -> bool {
        let Ok(mut s) = self.state.lock() else {
            return false;
        };
        if self.cancelled() || !s.reject_uncommitted(generation, reason) {
            return false;
        }
        self.changed.notify_all();
        true
    }
    /// Both the polling coordinator and mux can report one decoder's failure.
    /// Classify and retire under one lock: a stale report must never turn an
    /// already retired future into a fatal failure of the current input.
    pub fn report_failure(&self, generation: u64, error: &'static str) {
        let Ok(mut state) = self.state.lock() else {
            self.cancel();
            return;
        };
        if self.cancelled() {
            return;
        }
        if state.reject_uncommitted(
            generation,
            "Next decoder failed before admission; independent loading required",
        ) {
            self.changed.notify_all();
        } else if state.current.input.generation == generation
            || state
                .future
                .as_ref()
                .is_some_and(|future| future.input.generation == generation)
        {
            state.error.get_or_insert(error);
            self.cancel();
        }
    }
    pub fn stage(&self, owner: u64, input: Input) -> Result<(), &'static str> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        if self.cancelled()
            || s.sealed
            || s.finished
            || s.current.input.generation != owner
            || input.generation <= s.highest_generation
            || s.future.is_some()
        {
            return Err("Continuous preload window closed");
        }
        if !input.valid(self.minimum)
            || input.description.rate != s.current.input.description.rate
            || input.description.bits != s.current.input.description.bits
        {
            return Err("Continuous successor requires independent playback");
        }
        let start = s.current.end().ok_or("Continuous timeline exhausted")?;
        if start
            .checked_add(input.description.frames)
            .is_none_or(|end| end > (1_u64 << 52))
        {
            return Err("Continuous timeline exhausted");
        }
        s.highest_generation = input.generation;
        s.future = Some(Slot::new(input, start));
        self.changed.notify_all();
        Ok(())
    }
    pub fn remove(&self, owner: u64, next: u64) -> Result<Removal, &'static str> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        if self.cancelled() || s.current.input.generation != owner {
            return Ok(Removal::Stale);
        }
        let Some(future) = &s.future else {
            return Ok(Removal::Untouched);
        };
        if future.input.generation != next {
            return Ok(Removal::Stale);
        }
        if future.written != 0 {
            return Ok(Removal::Committed);
        }
        s.future = None;
        self.changed.notify_all();
        Ok(Removal::Untouched)
    }
    pub fn seal(&self, owner: u64) -> Result<(), &'static str> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        if self.cancelled() || s.current.input.generation != owner || s.future.is_some() {
            return Err("Continuous completion owner changed");
        }
        s.sealed = true;
        self.changed.notify_all();
        Ok(())
    }
    /// The producer waits for authorization; temporary absence is never EOF.
    pub fn next_input(&self) -> Result<Option<Input>, &'static str> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        loop {
            if self.cancelled() {
                return Err("Continuous input cancelled");
            }
            if !s.current.claimed {
                s.current.claimed = true;
                return Ok(Some(s.current.input.clone()));
            }
            if s.current.decoded {
                if let Some(future) = &mut s.future {
                    if !future.claimed {
                        future.claimed = true;
                        return Ok(Some(future.input.clone()));
                    }
                } else if s.sealed {
                    s.finished = true;
                    self.changed.notify_all();
                    return Ok(None);
                }
            }
            s = self
                .changed
                .wait_timeout(s, Duration::from_millis(20))
                .map_err(|_| "Continuous input unavailable")?
                .0;
        }
    }
    pub fn complete(&self, generation: u64) -> Result<(), &'static str> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        let slot = if s.current.input.generation == generation {
            &mut s.current
        } else {
            s.future
                .as_mut()
                .filter(|f| f.input.generation == generation)
                .ok_or("Continuous source retired")?
        };
        if !slot.claimed || slot.written != slot.input.description.frames * FRAME_BYTES {
            return Err("Continuous source frame count incomplete");
        }
        slot.decoded = true;
        self.changed.notify_all();
        Ok(())
    }
    fn slot(s: &State, generation: u64) -> Option<&Slot> {
        if s.current.input.generation == generation {
            Some(&s.current)
        } else {
            s.future
                .as_ref()
                .filter(|f| f.input.generation == generation)
        }
    }
    pub fn write(&self, generation: u64, mut input: &[u8]) -> Result<(), &'static str> {
        if self
            .writer_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("Concurrent continuous PCM writer refused");
        }
        struct Writer<'a>(&'a AtomicBool);
        impl Drop for Writer<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _writer = Writer(&self.writer_active);
        if !(input.len() as u64).is_multiple_of(FRAME_BYTES) {
            return Err("Incomplete continuous PCM frame");
        }
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        let slot = Self::slot(&s, generation).ok_or("Continuous source retired")?;
        if !slot.claimed
            || slot.decoded
            || generation != s.current.input.generation && !s.current.decoded
        {
            return Err("Continuous source is not writable");
        }
        if slot
            .written
            .checked_add(input.len() as u64)
            .is_none_or(|bytes| bytes > slot.input.description.frames * FRAME_BYTES)
        {
            return Err("Excess continuous PCM frames");
        }
        loop {
            if self.cancelled() || s.finished {
                return Err("Continuous input cancelled");
            }
            if Self::slot(&s, generation).is_none() {
                return Err("Continuous source retired");
            }
            if input.is_empty() {
                return Ok(());
            }
            let size = input.len().min(CAPACITY - s.bytes.len());
            if size == 0 {
                s = self
                    .changed
                    .wait_timeout(s, Duration::from_millis(20))
                    .map_err(|_| "Continuous input unavailable")?
                    .0;
                continue;
            }
            let slot = Self::slot(&s, generation).ok_or("Continuous source retired")?;
            if slot
                .written
                .checked_add(size as u64)
                .is_none_or(|bytes| bytes > slot.input.description.frames * FRAME_BYTES)
            {
                return Err("Excess continuous PCM frames");
            }
            s.bytes.extend(&input[..size]);
            input = &input[size..];
            let slot = if s.current.input.generation == generation {
                &mut s.current
            } else {
                s.future
                    .as_mut()
                    .filter(|f| f.input.generation == generation)
                    .ok_or("Continuous source retired")?
            };
            slot.written += size as u64;
            self.high_water.fetch_max(s.bytes.len(), Ordering::Relaxed);
            self.changed.notify_all();
        }
    }
    pub fn read(&self, output: &mut [u8]) -> Result<usize, &'static str> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        loop {
            if self.cancelled() {
                return Err("Continuous input cancelled");
            }
            if !s.bytes.is_empty() || s.finished || output.is_empty() {
                let size = output.len().min(s.bytes.len());
                for byte in &mut output[..size] {
                    *byte = s.bytes.pop_front().ok_or("Continuous buffer underflow")?;
                }
                self.changed.notify_all();
                return Ok(size);
            }
            s = self
                .changed
                .wait_timeout(s, Duration::from_millis(20))
                .map_err(|_| "Continuous input unavailable")?
                .0;
        }
    }
    /// Only a fresh, externally qualified native nominal clock calls this.
    pub fn observe(&self, pts: f64) -> Result<Observation, &'static str> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "Continuous input unavailable")?;
        if !pts.is_finite() || pts < 0.0 || self.cancelled() && !s.finished {
            return Err("Continuous nominal clock unavailable");
        }
        let rate = f64::from(s.current.input.description.rate);
        let promote = s
            .future
            .as_ref()
            .is_some_and(|f| pts >= f.start as f64 / rate && f.written >= FRAME_BYTES);
        let candidate = if promote {
            s.future
                .as_ref()
                .ok_or("Continuous successor disappeared")?
        } else {
            &s.current
        };
        let position = pts - candidate.start as f64 / rate;
        let admitted = candidate.written / FRAME_BYTES;
        if position > (admitted as f64 + 3.0) / rate {
            return Err("Continuous clock exceeds admitted source span");
        }
        let position = (position >= 0.0)
            .then(|| position.min(candidate.input.description.frames as f64 / rate));
        if promote {
            let previous = s.current.input.generation;
            s.current = s.future.take().ok_or("Continuous successor disappeared")?;
            s.handoff = Some(previous);
            self.changed.notify_all();
        }
        Ok(Observation {
            generation: s.current.input.generation,
            position,
            handoff_from: s.handoff,
            staged_generation: s.future.as_ref().map(|f| f.input.generation),
        })
    }
}

impl super::input::Reader for Stream {
    fn read(&self, bytes: &mut [u8]) -> Result<usize, &'static str> {
        Stream::read(self, bytes)
    }
    fn cancel(&self) {
        Stream::cancel(self)
    }
    fn cancelled(&self) -> bool {
        Stream::cancelled(self) && !self.finished()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Instant};
    fn input(generation: u64, frames: u64) -> Input {
        Input {
            generation,
            uri: "private fixture".into(),
            description: Description {
                rate: 48000,
                bits: 16,
                frames,
            },
            gain: 1.0,
        }
    }
    fn claim(stream: &Stream, generation: u64) {
        assert_eq!(stream.next_input().unwrap().unwrap().generation, generation);
    }
    #[test]
    fn retired_failure_is_ignored_but_current_and_committed_failures_remain_fatal() {
        let stream = Stream::new(input(1, 1), 1).unwrap();
        stream.stage(1, input(2, 1)).unwrap();
        assert_eq!(stream.remove(1, 2), Ok(Removal::Untouched));
        stream.report_failure(2, "Retired failure");
        assert_eq!(stream.error(), None);
        assert!(!stream.cancelled());
        stream.report_failure(1, "Current failure");
        assert_eq!(stream.error(), Some("Current failure"));
        assert!(stream.cancelled());

        let stream = Stream::new(input(1, 1), 1).unwrap();
        claim(&stream, 1);
        stream.write(1, &[0; 16]).unwrap();
        stream.complete(1).unwrap();
        stream.stage(1, input(2, 1)).unwrap();
        claim(&stream, 2);
        stream.write(2, &[0; 16]).unwrap();
        stream.report_failure(2, "Committed failure");
        assert_eq!(stream.error(), Some("Committed failure"));
        assert!(stream.cancelled());
    }
    #[test]
    fn concurrent_failed_future_reports_do_not_poison_the_current_input() {
        for _ in 0..40 {
            let stream = Stream::new(input(1, 1), 1).unwrap();
            stream.stage(1, input(2, 1)).unwrap();
            let start = std::sync::Arc::new(std::sync::Barrier::new(16));
            std::thread::scope(|scope| {
                for _ in 0..16 {
                    let stream = stream.clone();
                    let start = start.clone();
                    scope.spawn(move || {
                        start.wait();
                        stream.report_failure(2, "Synthetic future failure");
                    });
                }
            });
            assert_eq!(stream.error(), None);
            assert!(!stream.cancelled());
            assert_eq!(stream.owner(), Ok(1));
            assert!(stream.fallback().is_some());
        }
    }
    #[test]
    fn one_successor_ledger_preserves_frame_spans_and_does_not_credit_admission() {
        let stream = Stream::new(input(1, 2), 1).unwrap();
        claim(&stream, 1);
        stream.stage(1, input(2, 3)).unwrap();
        assert!(stream.stage(1, input(3, 1)).is_err());
        assert_eq!(stream.observe(0.0).unwrap().generation, 1);
        assert!(stream.write(2, &[2; 16]).is_err());
        stream.write(1, &[1; 32]).unwrap();
        stream.complete(1).unwrap();
        claim(&stream, 2);
        stream.write(2, &[2; 16]).unwrap();
        assert_eq!(stream.remove(1, 2).unwrap(), Removal::Committed);
        assert_eq!(stream.observe(1.0 / 48000.0).unwrap().generation, 1);
        let observation = stream.observe(2.5 / 48000.0).unwrap();
        assert_eq!(observation.generation, 2);
        assert_eq!(observation.handoff_from, Some(1));
        assert_eq!(observation.staged_generation, None);
        assert!((observation.position.unwrap() - 0.5 / 48000.0).abs() < 1e-12);
        assert!(stream.write(1, &[9; 16]).is_err());
        assert!(stream.complete(2).is_err());
        stream.write(2, &[2; 32]).unwrap();
        stream.complete(2).unwrap();
        stream.stage(2, input(3, 1)).unwrap();
        assert_eq!(stream.remove(1, 2).unwrap(), Removal::Stale);
        assert_eq!(stream.observe(0.0).unwrap().position, None);
    }
    #[test]
    fn uncommitted_decoder_is_retired_atomically_and_can_be_replaced() {
        let stream = Stream::new(input(1, 1), 1).unwrap();
        claim(&stream, 1);
        stream.write(1, &[1; 16]).unwrap();
        stream.complete(1).unwrap();
        stream.stage(1, input(2, 1)).unwrap();
        claim(&stream, 2);
        assert_eq!(stream.remove(1, 2).unwrap(), Removal::Untouched);
        assert!(stream.stage(1, input(2, 1)).is_err());
        assert!(!stream.live(2));
        assert!(stream.write(2, &[2; 16]).is_err());
        assert!(stream.complete(2).is_err());
        stream.stage(1, input(3, 1)).unwrap();
        claim(&stream, 3);
        stream.write(3, &[3; 16]).unwrap();
        let mut bytes = [0; 32];
        assert_eq!(stream.read(&mut bytes), Ok(32));
        assert_eq!(&bytes[..16], &[1; 16]);
        assert_eq!(&bytes[16..], &[3; 16]);
    }
    #[test]
    fn starvation_waits_for_authorization_and_seal_is_irreversible_final_eof() {
        let stream = Stream::new(input(1, 1), 1).unwrap();
        claim(&stream, 1);
        stream.write(1, &[1; 16]).unwrap();
        stream.complete(1).unwrap();
        let copy = stream.clone();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || tx.send(copy.next_input().unwrap().is_none()).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        stream.seal(1).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(250)).unwrap());
        worker.join().unwrap();
        assert!(stream.stage(1, input(2, 1)).is_err());
        let mut byte = [0];
        for _ in 0..16 {
            assert_eq!(stream.read(&mut byte), Ok(1));
            assert_eq!(byte[0], 1);
        }
        assert_eq!(stream.read(&mut byte), Ok(0));
        assert!(stream.write(1, &[0; 16]).is_err());
    }
    #[test]
    fn malformed_profile_and_clock_do_not_promote_or_change_ownership() {
        assert!(Stream::new(input(0, 1), 1).is_err());
        assert!(Stream::new(input(1, 1), 2).is_err());
        let stream = Stream::new(input(1, 10), 2).unwrap();
        claim(&stream, 1);
        stream.write(1, &[1; 160]).unwrap();
        stream.complete(1).unwrap();
        let mut incompatible = input(2, 10);
        incompatible.description.rate = 44100;
        assert!(stream.stage(1, incompatible).is_err());
        assert!(stream.stage(1, input(2, 1)).is_err());
        stream.stage(1, input(2, 10)).unwrap();
        claim(&stream, 2);
        stream.write(2, &[2; 16]).unwrap();
        assert!(stream.observe(f64::NAN).is_err());
        assert!(stream.observe(-1.0).is_err());
        assert!(stream.observe(15.0 / 48000.0).is_err());
        assert_eq!(stream.observe(9.0 / 48000.0).unwrap().generation, 1);
    }
    #[test]
    fn bounded_transport_cancels_empty_reads_and_full_writes_without_stale_frames() {
        for full in [false, true] {
            let stream = Stream::new(input(1, 10000), 1).unwrap();
            let copy = stream.clone();
            let (tx, rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                let result = if full {
                    claim(&copy, 1);
                    copy.write(1, &vec![1; 160000]).map(|()| 0)
                } else {
                    copy.read(&mut [0])
                };
                tx.send(result.is_err()).unwrap();
            });
            assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
            let start = Instant::now();
            stream.cancel();
            assert!(rx.recv_timeout(Duration::from_millis(250)).unwrap());
            assert!(start.elapsed() < Duration::from_millis(250));
            worker.join().unwrap();
            assert!(stream.high_water.load(Ordering::Relaxed) <= CAPACITY);
            assert!(!stream.live(1));
        }
        let stream = Stream::new(input(1, 1), 1).unwrap();
        stream.fail("fixture failed");
        assert_eq!(stream.error(), Some("fixture failed"));
        assert!(stream.next_input().is_err());
    }
    #[test]
    fn speculative_failure_before_commit_seals_only_the_current_group() {
        let stream = Stream::new(input(1, 1), 1).unwrap();
        claim(&stream, 1);
        stream.write(1, &[1; 16]).unwrap();
        stream.complete(1).unwrap();
        stream.stage(1, input(2, 1)).unwrap();
        claim(&stream, 2);
        assert!(stream.reject_uncommitted(2, "Next decoder failed; independent loading required"));
        assert_eq!(stream.owner(), Ok(1));
        assert!(!stream.live(2));
        assert!(!stream.cancelled());
        assert!(stream.fallback().is_some());
        assert!(stream.next_input().unwrap().is_none());
        let mut bytes = [0; 32];
        assert_eq!(stream.read(&mut bytes), Ok(16));
        assert_eq!(&bytes[..16], &[1; 16]);
        assert_eq!(stream.read(&mut bytes), Ok(0));
        assert!(stream.stage(1, input(3, 1)).is_err());
    }
    #[test]
    fn a_partially_committed_successor_failure_requires_a_full_output_reset() {
        let stream = Stream::new(input(1, 1), 1).unwrap();
        claim(&stream, 1);
        stream.write(1, &[1; 16]).unwrap();
        stream.complete(1).unwrap();
        stream.stage(1, input(2, 2)).unwrap();
        claim(&stream, 2);
        stream.write(2, &[2; 16]).unwrap();
        assert!(!stream.reject_uncommitted(2, "cannot discard admitted bytes"));
        assert_eq!(stream.fallback(), None);
        assert_eq!(stream.remove(1, 2).unwrap(), Removal::Committed);
        stream.fail("Committed decoder failed; output reset required");
        assert!(stream.read(&mut [0; 32]).is_err());
        assert!(stream.observe(0.0).is_err());
    }
    #[test]
    fn simultaneous_writers_are_refused_without_waiting_for_transport_capacity() {
        let stream = Stream::new(input(1, 10000), 1).unwrap();
        claim(&stream, 1);
        let copy = stream.clone();
        let worker = thread::spawn(move || copy.write(1, &vec![1; 160000]));
        let deadline = Instant::now() + Duration::from_secs(1);
        while stream.high_water.load(Ordering::Relaxed) < CAPACITY && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(stream.high_water.load(Ordering::Relaxed), CAPACITY);
        assert_eq!(
            stream.write(1, &[9; 16]),
            Err("Concurrent continuous PCM writer refused")
        );
        stream.cancel();
        assert!(worker.join().unwrap().is_err());
    }
}
