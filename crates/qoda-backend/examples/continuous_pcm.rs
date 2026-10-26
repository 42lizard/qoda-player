//! Device-free #41 qualification using the shared production decoder/stream core.
//! This executable has no provider grants or queue coordinator.
//! Manifest contains original synthetic local sources; output is PCM-file only.
#[path = "../src/continuous/decode.rs"]
mod decode;
#[path = "continuous_pcm/decoder.rs"]
mod decoder;
#[path = "../src/continuous/description.rs"]
mod description;
#[path = "../src/continuous/input.rs"]
mod input;
#[path = "continuous_pcm/native.rs"]
mod native;
#[path = "../src/continuous/ring.rs"]
mod ring;
#[path = "../src/continuous/session.rs"]
pub mod session;
#[path = "../src/continuous/stream.rs"]
pub mod stream;
use std::{path::PathBuf, sync::atomic::Ordering};
fn run() -> Result<(), String> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let use_session = args.first().is_some_and(|arg| arg == "--session");
    if use_session {
        args.remove(0);
    }
    if args == ["--horizon"] {
        println!("{}", native::horizon()?);
        return Ok(());
    }
    if !matches!(args.len(), 4 | 5) {
        return Err(
            "Expected synthetic manifest, new capture, graph, output bits, optional target rate"
                .into(),
        );
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
    let target_rate = args
        .get(4)
        .map_or(Ok(48000), |rate| rate.parse::<u32>())
        .map_err(|_| "Invalid target rate")?;
    if use_session {
        let inputs: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                Ok(stream::Input {
                    generation: index as u64 + 1,
                    uri: source
                        .path
                        .to_str()
                        .ok_or("Source path encoding unsupported")?
                        .to_owned(),
                    description: decoder::describe(source)?,
                    gain: source.gain,
                })
            })
            .collect::<Result<_, &'static str>>()?;
        let session = session::Session::new(inputs[0].clone(), 1)?;
        let native =
            native::Native::for_session(&session, &args[1], rate, &args[2], bits, target_rate)?;
        let result = native.run_session(&session, &inputs[1..]);
        drop(native);
        let observations = result?;
        let summary = serde_json::json!({"source_frame_boundaries":inputs.iter().scan(0_u64,|sum,input|{*sum+=input.description.frames;Some(*sum)}).collect::<Vec<_>>(),
            "ring_high_water_bytes":session.stream.high_water.load(Ordering::Relaxed),"capture_complete":true,"nominal_observations":observations});
        drop(session);
        println!("{summary}");
        return Ok(());
    }
    let ring = ring::Ring::new();
    let native = native::Native::new(ring.clone(), &args[1], rate, &args[2], bits, target_rate)?;
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
