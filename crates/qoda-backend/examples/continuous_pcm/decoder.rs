//! Synthetic local-file manifest adapter for the shared source decoder.
use super::{decode, ring::Ring};
use serde::Deserialize;
use std::{io::Read, path::PathBuf, sync::Arc};
#[derive(Deserialize)]
pub struct Source {
    pub path: PathBuf,
    pub frames: u64,
    pub rate: u32,
    pub gain: f64,
}
pub fn describe(source: &Source) -> Result<super::description::Description, &'static str> {
    if !source.path.is_absolute() || !source.path.is_file() {
        return Err("Unsupported experiment source");
    }
    let file = std::fs::File::open(&source.path).map_err(|_| "Source header unavailable")?;
    let length = file
        .metadata()
        .map_err(|_| "Source length unavailable")?
        .len();
    let mut prefix = Vec::with_capacity(65536);
    file.take(65536)
        .read_to_end(&mut prefix)
        .map_err(|_| "Source header read failed")?;
    let description = super::description::parse(&prefix, length)
        .ok_or("Unqualified continuous source framing")?;
    if description.frames != source.frames || description.rate != source.rate {
        return Err("Source frame descriptor mismatch");
    }
    Ok(description)
}
pub fn decode(source: &Source, ring: &Ring) -> Result<u64, &'static str> {
    let description = describe(source)?;
    decode::run(
        &decode::Input {
            uri: source
                .path
                .to_str()
                .ok_or("Source path encoding unsupported")?,
            description,
            gain: source.gain,
        },
        ring,
    )
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
    use std::{
        io::Write,
        thread,
        time::{Duration, Instant},
    };
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
