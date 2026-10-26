//! Bounded callback transport. Starvation is distinct from final EOF.
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
const CAPACITY: usize = 65_536;
#[derive(Default)]
struct State {
    bytes: VecDeque<u8>,
    finished: bool,
}
#[derive(Default)]
pub struct Ring {
    state: Mutex<State>,
    changed: Condvar,
    cancelled: AtomicBool,
    pub high_water: AtomicUsize,
}
impl Ring {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn cancel(&self) {
        // mpv may invoke cancel on another thread. Never take the read lock here.
        self.cancelled.store(true, Ordering::Release);
        self.changed.notify_all();
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn write(&self, mut input: &[u8]) -> Result<(), &'static str> {
        let mut state = self.state.lock().map_err(|_| "PCM ring poisoned")?;
        while !input.is_empty() {
            if self.cancelled() || state.finished {
                return Err("PCM input cancelled or sealed");
            }
            let size = input.len().min(CAPACITY - state.bytes.len());
            if size != 0 {
                state.bytes.extend(&input[..size]);
                self.high_water
                    .fetch_max(state.bytes.len(), Ordering::Relaxed);
                input = &input[size..];
                self.changed.notify_all();
            } else {
                state = self
                    .changed
                    .wait_timeout(state, Duration::from_millis(20))
                    .map_err(|_| "PCM ring poisoned")?
                    .0;
            }
        }
        Ok(())
    }
    pub fn read(&self, output: &mut [u8]) -> Result<usize, &'static str> {
        let mut state = self.state.lock().map_err(|_| "PCM ring poisoned")?;
        loop {
            if self.cancelled() {
                return Err("PCM input cancelled");
            }
            if !state.bytes.is_empty() || state.finished || output.is_empty() {
                let size = output.len().min(state.bytes.len());
                for byte in &mut output[..size] {
                    *byte = state.bytes.pop_front().ok_or("PCM ring underflow")?;
                }
                self.changed.notify_all();
                return Ok(size);
            }
            // The timeout also covers a cancellation notification racing the
            // atomic predicate and Condvar wait. cancel itself cannot block.
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .map_err(|_| "PCM ring poisoned")?
                .0;
        }
    }
    pub fn finish(&self) -> Result<(), &'static str> {
        self.state.lock().map_err(|_| "PCM ring poisoned")?.finished = true;
        self.changed.notify_all();
        Ok(())
    }
}

impl super::decode::Target for Ring {
    fn cancelled(&self) -> bool {
        Ring::cancelled(self)
    }
    fn write(&self, bytes: &[u8]) -> Result<(), &'static str> {
        Ring::write(self, bytes)
    }
}

impl super::input::Reader for Ring {
    fn read(&self, bytes: &mut [u8]) -> Result<usize, &'static str> {
        Ring::read(self, bytes)
    }
    fn cancel(&self) {
        Ring::cancel(self)
    }
    fn cancelled(&self) -> bool {
        Ring::cancelled(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Instant};
    #[test]
    fn starvation_is_not_eof_and_byte_sized_reads_preserve_frames() {
        let ring = Ring::new();
        let copy = ring.clone();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut bytes = [0; 1];
            for expected in 0..32 {
                assert_eq!(copy.read(&mut bytes), Ok(1));
                assert_eq!(bytes[0], expected);
            }
            assert_eq!(copy.read(&mut bytes), Ok(0));
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        ring.write(&(0..32).collect::<Vec<_>>()).unwrap();
        ring.finish().unwrap();
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert!(ring.write(&[1]).is_err());
    }
    #[test]
    fn cancel_unblocks_empty_reader_and_full_writer() {
        for full in [false, true] {
            let ring = Ring::new();
            let copy = ring.clone();
            let (tx, rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                let result = if full {
                    copy.write(&vec![1; CAPACITY * 4]).map(|()| 0)
                } else {
                    copy.read(&mut [0; 1])
                };
                tx.send(result.is_err()).unwrap();
            });
            assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
            let start = Instant::now();
            ring.cancel();
            assert!(rx.recv_timeout(Duration::from_millis(250)).unwrap());
            assert!(start.elapsed() < Duration::from_millis(250));
            worker.join().unwrap();
            assert!(ring.high_water.load(Ordering::Relaxed) <= CAPACITY);
        }
    }
}
