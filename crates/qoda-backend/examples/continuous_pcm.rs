//! Device-free #41 experiment. Not wired into the player or its provider grants.
//! Manifest contains original synthetic local sources; output is PCM-file only.
#[path = "continuous_pcm/decoder.rs"]
mod decoder;
#[path = "../src/continuous/description.rs"]
mod description;
#[path = "continuous_pcm/native.rs"]
mod native;
#[path = "continuous_pcm/ring.rs"]
mod ring;
#[cfg(test)]
#[path = "../src/continuous/stream.rs"]
mod stream;
use std::{path::PathBuf, sync::atomic::Ordering};
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--horizon"] {
        println!("{}", native::horizon()?);
        return Ok(());
    }
    if args.len() != 4 {
        return Err("Expected synthetic manifest, new capture, graph, output bits".into());
    }
    let manifest = std::fs::read(&args[0]).map_err(|_| "Manifest read failed")?;
    if manifest.len() > 65_536 {
        return Err("Manifest too large".into());
    }
    let sources: Vec<decoder::Source> =
        serde_json::from_slice(&manifest).map_err(|_| "Invalid manifest")?;
    if sources.is_empty() || sources.len() > 16 {
        return Err("Expected one to sixteen synthetic sources".into());
    }
    let rate = sources[0].rate;
    let capture = PathBuf::from(&args[1]);
    if !capture.is_absolute() || capture.exists() {
        return Err("Capture must be a new absolute path".into());
    }
    let bits: u16 = args[3].parse().map_err(|_| "Invalid output bits")?;
    if !matches!(bits, 16 | 32) {
        return Err("Unsupported output bits".into());
    }
    let ring = ring::Ring::new();
    let native = native::Native::new(ring.clone(), &args[1], rate, &args[2], bits)?;
    let copy = ring.clone();
    let producer = std::thread::spawn(move || {
        let result = decoder::produce(&sources, &copy);
        if result.is_err() {
            copy.cancel();
        }
        result
    });
    let rendered = native.run();
    drop(native); // cancels/wakes producer on every success/failure path
    let produced = producer.join().map_err(|_| "Producer panicked")?;
    let boundaries = produced?;
    rendered?;
    println!(
        "{}",
        serde_json::json!({"source_frame_boundaries":boundaries,
        "ring_high_water_bytes":ring.high_water.load(Ordering::Relaxed), "capture_complete":true})
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
