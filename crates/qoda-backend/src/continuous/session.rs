//! Bounded independent source prewarming and ordered admission into one stream.
use super::{
    decode,
    ring::Ring,
    stream::{Input, Observation, Removal, Stream},
};
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    thread,
};
struct Destination {
    buffer: Arc<Ring>,
    stream: Arc<Stream>,
    generation: u64,
}
impl decode::Target for Destination {
    fn cancelled(&self) -> bool {
        self.buffer.cancelled() || !self.stream.live(self.generation)
    }
    fn write(&self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.cancelled() {
            return Err("Decoder input retired");
        }
        self.buffer.write(bytes)
    }
}
struct Worker {
    buffer: Arc<Ring>,
    result: Arc<Mutex<Option<Result<(), &'static str>>>>,
    join: Mutex<Option<thread::JoinHandle<()>>>,
}
impl Worker {
    fn spawn(input: Input, stream: Arc<Stream>) -> Result<Arc<Self>, &'static str> {
        let buffer = Ring::new();
        let result = Arc::new(Mutex::new(None));
        let worker = Arc::new(Self {
            buffer: buffer.clone(),
            result: result.clone(),
            join: Mutex::new(None),
        });
        let destination = Destination {
            buffer: buffer.clone(),
            stream,
            generation: input.generation,
        };
        let join = thread::Builder::new()
            .name("qoda-pcm-source".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    decode::run(
                        &decode::Input {
                            uri: &input.uri,
                            description: input.description,
                            gain: input.gain,
                        },
                        &destination,
                    )
                    .map(|_| ())
                }))
                .unwrap_or(Err("Continuous decoder worker failed"));
                *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
                if outcome.is_ok() {
                    let _ = buffer.finish();
                } else {
                    buffer.cancel();
                }
            })
            .map_err(|_| "Continuous decoder worker unavailable")?;
        *worker.join.lock().unwrap_or_else(|e| e.into_inner()) = Some(join);
        Ok(worker)
    }
    fn outcome(&self) -> Option<Result<(), &'static str>> {
        *self.result.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn cancel(&self) {
        self.buffer.cancel();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.buffer.cancel();
        if let Some(join) = self
            .join
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = join.join();
        }
    }
}
type Workers = Arc<Mutex<HashMap<u64, Arc<Worker>>>>;
pub struct Session {
    pub stream: Arc<Stream>,
    workers: Workers,
    mux: Option<thread::JoinHandle<()>>,
}
fn retire(workers: &Workers, generation: u64) {
    let worker = workers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&generation);
    if let Some(worker) = worker {
        worker.cancel();
    }
}
fn multiplex(stream: &Stream, workers: &Workers) -> Result<(), &'static str> {
    loop {
        let Some(input) = stream.next_input()? else {
            return Ok(());
        };
        let source = workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&input.generation)
            .cloned();
        let Some(source) = source else {
            if !stream.live(input.generation) {
                continue;
            }
            return Err("Continuous source worker disappeared");
        };
        let mut buffer = [0_u8; 16384];
        // Ring reads can be byte-sized. Preserve complete double stereo frames
        // before the admission API; no segment EOF may pad or fabricate samples.
        let mut pending = Vec::with_capacity(16400);
        let result = (|| {
            loop {
                let size = source.buffer.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                pending.extend_from_slice(&buffer[..size]);
                let complete = pending.len() / 16 * 16;
                stream.write(input.generation, &pending[..complete])?;
                pending.drain(..complete);
            }
            if !pending.is_empty() {
                return Err("Incomplete warmed PCM frame");
            }
            match source.outcome() {
                Some(Ok(())) => stream.complete(input.generation),
                Some(Err(error)) => Err(error),
                None => Err("Continuous decoder completion missing"),
            }
        })();
        let result =
            result.map_err(|error| source.outcome().and_then(Result::err).unwrap_or(error));
        retire(workers, input.generation);
        drop(source);
        if let Err(error) = result {
            if !stream.live(input.generation) {
                continue;
            }
            if stream.reject_uncommitted(
                input.generation,
                "Next decoder failed before admission; independent loading required",
            ) {
                continue;
            }
            return Err(error);
        }
    }
}
impl Session {
    pub fn new(input: Input, minimum_frames: u64) -> Result<Self, &'static str> {
        let stream = Stream::new(input.clone(), minimum_frames)?;
        let worker = Worker::spawn(input.clone(), stream.clone())?;
        let workers = Arc::new(Mutex::new(HashMap::from([(input.generation, worker)])));
        let copy = stream.clone();
        let registry = workers.clone();
        let mux = match thread::Builder::new()
            .name("qoda-pcm-admission".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| multiplex(&copy, &registry)))
                    .unwrap_or(Err("Continuous admission worker failed"));
                if let Err(error) = outcome
                    && !copy.cancelled()
                {
                    copy.fail(error);
                }
            }) {
            Ok(join) => join,
            Err(_) => {
                stream.cancel();
                for worker in workers.lock().unwrap_or_else(|e| e.into_inner()).values() {
                    worker.cancel();
                }
                return Err("Continuous admission worker unavailable");
            }
        };
        Ok(Self {
            stream,
            workers,
            mux: Some(mux),
        })
    }
    pub fn stage(&self, owner: u64, input: Input) -> Result<(), &'static str> {
        // Publish authorization while holding the registry lock so a mux that
        // immediately claims next cannot observe a missing half-registered worker.
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        if workers.len() >= 2 {
            return Err("Continuous source worker limit reached");
        }
        let generation = input.generation;
        self.stream.stage(owner, input.clone())?;
        match Worker::spawn(input, self.stream.clone()) {
            Ok(worker) => {
                workers.insert(generation, worker);
                Ok(())
            }
            Err(error) => {
                self.stream.reject_uncommitted(
                    generation,
                    "Next decoder unavailable; independent loading required",
                );
                Err(error)
            }
        }
    }
    pub fn remove(&self, owner: u64, next: u64) -> Result<Removal, &'static str> {
        let result = self.stream.remove(owner, next)?;
        if result == Removal::Untouched {
            retire(&self.workers, next);
        }
        Ok(result)
    }
    pub fn observe(&self, pts: f64) -> Result<Observation, &'static str> {
        self.stream.observe(pts)
    }
    pub fn seal(&self, owner: u64) -> Result<(), &'static str> {
        self.stream.seal(owner)
    }
    pub fn poll(&self) {
        let failed: Vec<_> = self
            .workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|(generation, worker)| {
                worker
                    .outcome()
                    .and_then(Result::err)
                    .map(|error| (*generation, error))
            })
            .collect();
        for (generation, error) in failed {
            if !self.stream.live(generation) {
                continue;
            }
            if self.stream.reject_uncommitted(
                generation,
                "Next decoder failed before admission; independent loading required",
            ) {
                retire(&self.workers, generation);
            } else {
                self.stream.fail(error);
            }
        }
    }
    pub fn cancel(&self) {
        self.stream.cancel();
        let workers: Vec<_> = self
            .workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for worker in workers {
            worker.cancel();
        }
    }
    #[cfg(test)]
    fn warming(&self, generation: u64) -> usize {
        self.workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&generation)
            .map_or(0, |w| {
                w.buffer
                    .high_water
                    .load(std::sync::atomic::Ordering::Relaxed)
            })
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel();
        if let Some(mux) = self.mux.take() {
            let _ = mux.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::description::Description;
    use super::*;
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::PathBuf,
        time::{Duration, Instant},
    };
    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("qoda-pcm-workers-{}", uuid::Uuid::new_v4()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            Self { root }
        }
        fn input(&self, generation: u64, sample: i16, gain: f64) -> Input {
            let path = self.root.join(format!("synthetic-{generation}.wav"));
            let frames = 96000_u32;
            let bytes = frames * 4;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            for part in [
                b"RIFF".to_vec(),
                (bytes + 36).to_le_bytes().to_vec(),
                b"WAVEfmt ".to_vec(),
                16_u32.to_le_bytes().to_vec(),
                1_u16.to_le_bytes().to_vec(),
                2_u16.to_le_bytes().to_vec(),
                48000_u32.to_le_bytes().to_vec(),
                192000_u32.to_le_bytes().to_vec(),
                4_u16.to_le_bytes().to_vec(),
                16_u16.to_le_bytes().to_vec(),
                b"data".to_vec(),
                bytes.to_le_bytes().to_vec(),
            ] {
                file.write_all(&part).unwrap();
            }
            file.write_all(
                &[sample.to_le_bytes(), (-sample).to_le_bytes()]
                    .concat()
                    .repeat(frames as usize),
            )
            .unwrap();
            Input {
                generation,
                uri: path.to_str().unwrap().to_owned(),
                description: Description {
                    rate: 48000,
                    bits: 16,
                    frames: u64::from(frames),
                },
                gain,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[test]
    fn future_decoder_prewarms_without_final_admission_and_cancelled_samples_never_escape() {
        let fixture = Fixture::new();
        let session = Session::new(fixture.input(1, 1024, 1.0), 96000).unwrap();
        session.stage(1, fixture.input(2, 16384, 1.0)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while session.warming(2) < 65536 && Instant::now() < deadline {
            session.poll();
            assert_eq!(session.stream.error(), None);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.warming(2), 65536);
        // The final transport is full with current, so prewarmed future remains
        // uncommitted and can be retired while its process has a full pipe.
        assert_eq!(session.stream.owner(), Ok(1));
        let start = Instant::now();
        assert_eq!(session.remove(1, 2).unwrap(), Removal::Untouched);
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(!session.stream.live(2));
        session.stage(1, fixture.input(3, 2048, 0.25)).unwrap();
        let stream = session.stream.clone();
        let reader = thread::spawn(move || {
            let mut all = Vec::new();
            let mut bytes = [0; 16384];
            let mut promoted = false;
            loop {
                let size = stream.read(&mut bytes).unwrap();
                if size == 0 {
                    break;
                }
                all.extend_from_slice(&bytes[..size]);
                // Synthetic test clock only: native paced clock qualification is
                // separate. It must not be confused with real listening credit.
                if !promoted && all.len() > 96000 * 16 {
                    assert_eq!(stream.observe(2.0).unwrap().generation, 3);
                    stream.seal(3).unwrap();
                    promoted = true;
                }
            }
            assert!(promoted);
            all
        });
        let all = reader.join().unwrap();
        assert_eq!(all.len(), 192000 * 16);
        for (index, pair) in all.as_chunks::<16>().0.iter().enumerate() {
            let expected = if index < 96000 { 0.03125 } else { 0.015625 };
            assert_eq!(f64::from_ne_bytes(pair[..8].try_into().unwrap()), expected);
            assert_eq!(f64::from_ne_bytes(pair[8..].try_into().unwrap()), -expected);
        }
        assert_eq!(session.stream.error(), None);
        assert_eq!(session.observe(2.5).unwrap().generation, 3);
        let start = Instant::now();
        drop(session);
        assert!(start.elapsed() < Duration::from_millis(500));
    }
    #[test]
    fn full_current_and_future_workers_cancel_and_reap_without_a_reader() {
        let fixture = Fixture::new();
        let session = Session::new(fixture.input(1, 1024, 1.0), 96000).unwrap();
        session.stage(1, fixture.input(2, 2048, 1.0)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while session.warming(2) < 65536 && Instant::now() < deadline {
            let worker = session.workers.lock().unwrap().get(&2).cloned().unwrap();
            assert!(
                !matches!(worker.outcome(), Some(Err(_))),
                "{:?}",
                worker.outcome()
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.warming(2), 65536);
        assert!(session.seal(1).is_err());
        let start = Instant::now();
        session.cancel();
        drop(session);
        assert!(start.elapsed() < Duration::from_millis(500));
    }
    #[test]
    fn failed_uncommitted_future_ends_current_group_without_poisoning_current() {
        let fixture = Fixture::new();
        let session = Session::new(fixture.input(1, 1024, 1.0), 96000).unwrap();
        let mut bad = fixture.input(2, 2048, 1.0);
        bad.uri = fixture
            .root
            .join("missing.wav")
            .to_str()
            .unwrap()
            .to_owned();
        session.stage(1, bad).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.stream.fallback().is_none() && Instant::now() < deadline {
            session.poll();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(session.stream.fallback().is_some());
        assert_eq!(session.stream.error(), None);
        assert_eq!(session.stream.owner(), Ok(1));
        let mut frames = 0;
        let mut bytes = [0; 16384];
        loop {
            let size = session.stream.read(&mut bytes).unwrap();
            if size == 0 {
                break;
            }
            frames += size;
        }
        assert_eq!(frames, 96000 * 16);
        assert!(session.stage(1, fixture.input(3, 1024, 1.0)).is_err());
    }
}
