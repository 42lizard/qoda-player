//! Owned subprocess prevents mpv's blocking PCM AO from blocking backend teardown.
//! Source identifiers are sent over a private socket, never process arguments.
use super::ring::Ring;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
#[derive(Deserialize)]
pub struct Source {
    pub path: PathBuf,
    pub frames: u64,
    pub rate: u32,
    pub gain: f64,
}
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Ipc {
    socket: UnixStream,
    pending: Vec<u8>,
    serial: u64,
}
impl Ipc {
    fn request(&mut self, command: Value, ring: &Ring) -> Result<Value, &'static str> {
        if ring.cancelled() {
            return Err("Decoder cancelled");
        }
        self.serial = self.serial.checked_add(1).ok_or("IPC serial exhausted")?;
        let mut bytes = serde_json::to_vec(&json!({"command":command, "request_id":self.serial}))
            .map_err(|_| "IPC encoding failed")?;
        bytes.push(b'\n');
        self.socket
            .write_all(&bytes)
            .map_err(|_| "Decoder IPC write failed")?;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if ring.cancelled() {
                return Err("Decoder cancelled");
            }
            if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<_> = self.pending.drain(..=end).collect();
                let reply: Value =
                    serde_json::from_slice(&line).map_err(|_| "Invalid decoder reply")?;
                if reply["request_id"] == self.serial {
                    return if reply["error"] == "success" {
                        Ok(reply["data"].clone())
                    } else {
                        Err("Decoder command rejected")
                    };
                }
                continue;
            }
            if Instant::now() >= deadline {
                return Err("Decoder IPC deadline exceeded");
            }
            let mut bytes = [0; 4096];
            match self.socket.read(&mut bytes) {
                Ok(0) => return Err("Decoder IPC closed"),
                Ok(size) => {
                    self.pending.extend_from_slice(&bytes[..size]);
                    if self.pending.len() > 65_536 {
                        return Err("Invalid decoder IPC frame");
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err("Decoder IPC read failed"),
            }
        }
    }
}
fn checked_samples(bytes: &mut [u8], gain: f64) -> Result<(), &'static str> {
    if !bytes.len().is_multiple_of(16) {
        return Err("Truncated stereo frame");
    }
    for sample in bytes.as_chunks_mut::<8>().0 {
        let value = f64::from_ne_bytes(*sample);
        let value = value * gain;
        if !value.is_finite() {
            return Err("Non-finite PCM sample");
        }
        sample.copy_from_slice(&value.to_ne_bytes());
    }
    Ok(())
}
pub fn decode(source: &Source, ring: &Ring) -> Result<u64, &'static str> {
    if !source.path.is_absolute()
        || !source.path.is_file()
        || source.frames == 0
        || !matches!(source.rate, 44100 | 48000 | 96000)
        || !source.gain.is_finite()
        || !(0.0..=64.0).contains(&source.gain)
    {
        return Err("Unsupported experiment source");
    }
    let expected = source.frames.checked_mul(16).ok_or("PCM length overflow")?;
    let (parent, child_socket) = UnixStream::pair().map_err(|_| "Private decoder socket failed")?;
    parent
        .set_read_timeout(Some(Duration::from_millis(20)))
        .map_err(|_| "IPC timeout setup failed")?;
    parent
        .set_write_timeout(Some(Duration::from_millis(20)))
        .map_err(|_| "IPC timeout setup failed")?;
    let stdin: std::os::fd::OwnedFd = child_socket.into();
    let mut child = Owned(
        Command::new("mpv")
            .args([
                "--no-config",
                "--clipboard-backends=",
                "--load-auto-profiles=no",
                "--osc=no",
                "--load-scripts=no",
                "--ytdl=no",
                "--access-references=no",
                "--vo=null",
                "--vid=no",
                "--ao=pcm",
                "--ao-pcm-waveheader=no",
                "--ao-pcm-file=/dev/stdout",
                "--audio-format=double",
                "--volume=100",
                "--volume-gain=0",
                "--replaygain=no",
                "--speed=1",
                "--af=",
                "--pause=yes",
                "--audio-swresample-o=dither_method=none",
                "--gapless-audio=no",
                "--idle=once",
                "--terminal=no",
                "--input-terminal=no",
                "--input-ipc-client=fd://0",
            ])
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Decoder process start failed")?,
    );
    let mut socket = Ipc {
        socket: parent,
        pending: Vec::new(),
        serial: 0,
    };
    socket.request(json!(["loadfile", source.path, "replace"]), ring)?;
    let mut stdout = child.0.stdout.take().ok_or("Decoder pipe unavailable")?;
    // No blocking pipe read may strand the owner during cancellation/starvation.
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err("Decoder pipe setup failed");
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if ring.cancelled() {
            return Err("Decoder cancelled");
        }
        let params = socket.request(json!(["get_property", "audio-params"]), ring);
        let output = socket.request(json!(["get_property", "audio-out-params"]), ring);
        if let (Ok(params), Ok(output)) = (params, output) {
            if params["samplerate"] != source.rate
                || params["channels"] != "stereo"
                || output["samplerate"] != source.rate
                || output["channels"] != "stereo"
                || output["format"] != "double"
            {
                return Err("Decoder format mismatch");
            }
            break;
        }
        if Instant::now() >= deadline {
            return Err("Decoder parameters unavailable");
        }
        thread::sleep(Duration::from_millis(5));
    }
    socket.request(json!(["set_property", "pause", false]), ring)?;
    let mut total = 0_u64;
    let mut tail = Vec::with_capacity(16_400);
    let mut buffer = [0_u8; 16_384];
    let mut last_data = Instant::now();
    loop {
        if ring.cancelled() {
            return Err("Decoder cancelled");
        }
        let size = match stdout.read(&mut buffer) {
            Ok(size) => size,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if last_data.elapsed() > Duration::from_secs(5) {
                    return Err("Decoder stalled");
                }
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("Decoder pipe failed"),
        };
        if size == 0 {
            break;
        }
        last_data = Instant::now();
        total = total
            .checked_add(size as u64)
            .ok_or("PCM length overflow")?;
        if total > expected {
            return Err("Decoder emitted excess frames");
        }
        tail.extend_from_slice(&buffer[..size]);
        let complete = tail.len() / 16 * 16;
        checked_samples(&mut tail[..complete], source.gain)?;
        ring.write(&tail[..complete])?;
        tail.drain(..complete);
    }
    if total != expected || !tail.is_empty() {
        return Err("Decoder frame count mismatch");
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if ring.cancelled() {
            return Err("Decoder cancelled");
        }
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(|_| "Decoder completion failed")?
        {
            return if status.success() {
                Ok(source.frames)
            } else {
                Err("Decoder exited unsuccessfully")
            };
        }
        if Instant::now() >= deadline {
            return Err("Decoder exit deadline exceeded");
        }
        thread::sleep(Duration::from_millis(2));
    }
}
pub fn produce(sources: &[Source], ring: &Arc<Ring>) -> Result<Vec<u64>, &'static str> {
    let rate = sources.first().ok_or("No sources")?.rate;
    let mut cumulative = 0_u64;
    let mut boundaries = Vec::new();
    for source in sources {
        if source.rate != rate {
            return Err("Rate changes require an explicit discontinuity");
        }
        cumulative = cumulative
            .checked_add(decode(source, ring)?)
            .ok_or("Timeline overflow")?;
        boundaries.push(cumulative);
        // No EOF or silence at a logical source boundary.
    }
    ring.finish()?;
    Ok(boundaries)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_are_finite_and_gain_is_applied_once() {
        let mut bytes = [0.25_f64.to_ne_bytes(), (-0.5_f64).to_ne_bytes()].concat();
        checked_samples(&mut bytes, 0.5).unwrap();
        assert_eq!(f64::from_ne_bytes(bytes[..8].try_into().unwrap()), 0.125);
        assert_eq!(f64::from_ne_bytes(bytes[8..].try_into().unwrap()), -0.25);
        assert!(checked_samples(&mut bytes[..15], 1.0).is_err());
        bytes[..8].copy_from_slice(&f64::NAN.to_ne_bytes());
        assert!(checked_samples(&mut bytes, 1.0).is_err());
    }
    #[test]
    fn cancel_interrupts_an_ipc_reply_wait() {
        let (client, _unresponsive) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let ring = Ring::new();
        let copy = ring.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let mut ipc = Ipc {
                socket: client,
                pending: Vec::new(),
                serial: 0,
            };
            tx.send(ipc.request(json!(["get_property", "idle-active"]), &copy))
                .unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        ring.cancel();
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(250)).unwrap(),
            Err("Decoder cancelled")
        );
        worker.join().unwrap();
    }
    #[test]
    fn a_blocked_decoder_pipe_is_killed_and_reaped_after_ring_cancellation() {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let directory =
            std::env::temp_dir().join(format!("qoda-pcm-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let path = directory.join("synthetic.wav");
        let frames = 48000_u32 * 4;
        let length = frames * 4;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        for data in [
            b"RIFF".to_vec(),
            (length + 36).to_le_bytes().to_vec(),
            b"WAVEfmt ".to_vec(),
            16_u32.to_le_bytes().to_vec(),
            1_u16.to_le_bytes().to_vec(),
            2_u16.to_le_bytes().to_vec(),
            48000_u32.to_le_bytes().to_vec(),
            192000_u32.to_le_bytes().to_vec(),
            4_u16.to_le_bytes().to_vec(),
            16_u16.to_le_bytes().to_vec(),
            b"data".to_vec(),
            length.to_le_bytes().to_vec(),
        ] {
            file.write_all(&data).unwrap();
        }
        file.write_all(&vec![0_u8; length as usize]).unwrap();
        drop(file);
        let ring = Ring::new();
        let copy = ring.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let source = Source {
                path,
                frames: u64::from(frames),
                rate: 48000,
                gain: 1.0,
            };
            tx.send(decode(&source, &copy)).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while ring.high_water.load(std::sync::atomic::Ordering::Relaxed) != 65536
            && Instant::now() < deadline
        {
            if let Ok(result) = rx.try_recv() {
                panic!("Decoder stopped before backpressure: {result:?}");
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            ring.high_water.load(std::sync::atomic::Ordering::Relaxed),
            65536
        );
        // Let the owned decoder fill its stdout pipe while the ring stays full.
        assert!(rx.recv_timeout(Duration::from_millis(80)).is_err());
        ring.cancel();
        assert!(
            rx.recv_timeout(Duration::from_millis(500))
                .unwrap()
                .is_err()
        );
        worker.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
