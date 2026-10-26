//! Isolated lossless source decoder. Source identifiers use a private socket.
//! The PCM writer may block; the owner kills and reaps the process on every exit.
use super::description::Description;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
pub struct Input<'a> {
    pub uri: &'a str,
    pub description: Description,
    pub gain: f64,
}
pub trait Target: Send + Sync {
    fn cancelled(&self) -> bool;
    fn write(&self, bytes: &[u8]) -> Result<(), &'static str>;
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
    fn request(&mut self, command: Value, target: &dyn Target) -> Result<Value, &'static str> {
        if target.cancelled() {
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
            if target.cancelled() {
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
fn checked_samples(bytes: &mut [u8], gain: f64, bits: u16) -> Result<(), &'static str> {
    if !bytes.len().is_multiple_of(16) {
        return Err("Truncated stereo frame");
    }
    let scale = match bits {
        16 => 32768.0,
        24 => 8388608.0,
        _ => return Err("Unqualified source precision"),
    };
    for sample in bytes.as_chunks_mut::<8>().0 {
        let value = f64::from_ne_bytes(*sample);
        if !value.is_finite() || !(-1.0..1.0).contains(&value) || (value * scale).fract() != 0.0 {
            return Err("Decoded PCM does not match the verified integer precision");
        }
        let value = value * gain;
        if !value.is_finite() {
            return Err("Non-finite PCM sample");
        }
        sample.copy_from_slice(&value.to_ne_bytes());
    }
    Ok(())
}
pub fn run(source: &Input<'_>, target: &dyn Target) -> Result<u64, &'static str> {
    let description = source.description;
    if source.uri.is_empty()
        || source.uri.len() > 8192
        || source.uri.contains('\0')
        || description.frames == 0
        || !matches!(description.rate, 44100 | 48000 | 96000)
        || !matches!(description.bits, 16 | 24)
        || !source.gain.is_finite()
        || !(0.0..=1_000_000.0).contains(&source.gain)
    {
        return Err("Unqualified continuous decoder input");
    }
    let expected = description
        .frames
        .checked_mul(16)
        .ok_or("PCM length overflow")?;
    let (parent, child_socket) = UnixStream::pair().map_err(|_| "Private decoder socket failed")?;
    parent
        .set_read_timeout(Some(Duration::from_millis(20)))
        .map_err(|_| "IPC timeout setup failed")?;
    parent
        .set_write_timeout(Some(Duration::from_millis(20)))
        .map_err(|_| "IPC timeout setup failed")?;
    let stdin: std::os::fd::OwnedFd = child_socket.into();
    let mut child = Owned(
        Command::new("/usr/bin/mpv")
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
                "--audio-fallback-to-null=no",
                "--ad-lavc-threads=1",
                "--cache=no",
                "--demuxer-max-bytes=256KiB",
                "--demuxer-max-back-bytes=0",
                "--demuxer-readahead-secs=0",
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
    socket.request(json!(["loadfile", source.uri, "replace"]), target)?;
    let mut stdout = child.0.stdout.take().ok_or("Decoder pipe unavailable")?;
    // No blocking pipe read may strand the owner during cancellation/starvation.
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err("Decoder pipe setup failed");
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if target.cancelled() {
            return Err("Decoder cancelled");
        }
        let params = socket.request(json!(["get_property", "audio-params"]), target);
        let output = socket.request(json!(["get_property", "audio-out-params"]), target);
        if let (Ok(params), Ok(output)) = (params, output) {
            if params["samplerate"] != description.rate
                || params["channels"] != "stereo"
                || output["samplerate"] != description.rate
                || output["channels"] != "stereo"
                || output["format"] != "double"
            {
                return Err("Decoder format mismatch");
            }
            break;
        }
        if child
            .0
            .try_wait()
            .map_err(|_| "Decoder verification failed")?
            .is_some()
        {
            return Err("Decoder exited before source verification");
        }
        if Instant::now() >= deadline {
            return Err("Decoder parameters unavailable");
        }
        thread::sleep(Duration::from_millis(5));
    }
    socket.request(json!(["set_property", "pause", false]), target)?;
    let mut total = 0_u64;
    let mut tail = Vec::with_capacity(16_400);
    let mut buffer = [0_u8; 16_384];
    let mut last_data = Instant::now();
    loop {
        if target.cancelled() {
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
        total = total
            .checked_add(size as u64)
            .ok_or("PCM length overflow")?;
        if total > expected {
            return Err("Decoder emitted excess frames");
        }
        tail.extend_from_slice(&buffer[..size]);
        let complete = tail.len() / 16 * 16;
        checked_samples(&mut tail[..complete], source.gain, description.bits)?;
        target.write(&tail[..complete])?;
        // Time spent blocked by intentional pause/backpressure is not a source
        // transport stall. Start the inactivity budget only after admission.
        last_data = Instant::now();
        tail.drain(..complete);
    }
    if total != expected || !tail.is_empty() {
        return Err("Decoder frame count mismatch");
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if target.cancelled() {
            return Err("Decoder cancelled");
        }
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(|_| "Decoder completion failed")?
        {
            return if status.success() {
                Ok(description.frames)
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
#[cfg(test)]
mod tests {
    use super::super::ring::Ring;
    use super::*;
    #[test]
    fn frames_are_finite_and_gain_is_applied_once() {
        let mut bytes = [0.25_f64.to_ne_bytes(), (-0.5_f64).to_ne_bytes()].concat();
        checked_samples(&mut bytes, 0.5, 16).unwrap();
        assert_eq!(f64::from_ne_bytes(bytes[..8].try_into().unwrap()), 0.125);
        assert_eq!(f64::from_ne_bytes(bytes[8..].try_into().unwrap()), -0.25);
        assert!(checked_samples(&mut bytes[..15], 1.0, 16).is_err());
        bytes[..8].copy_from_slice(&(1.0_f64 / 8388608.0).to_ne_bytes());
        assert!(checked_samples(&mut bytes, 1.0, 16).is_err());
        assert!(checked_samples(&mut bytes, 1.0, 24).is_ok());
        bytes[..8].copy_from_slice(&f64::NAN.to_ne_bytes());
        assert!(checked_samples(&mut bytes, 1.0, 16).is_err());
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
            tx.send(ipc.request(json!(["get_property", "idle-active"]), copy.as_ref()))
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
}
