//! Metadata from one read-only descriptor. No shell, network or external references.
use super::*;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
#[derive(Default)]
pub(super) struct Metadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration: Option<u64>,
    pub disc: u32,
    pub number: u32,
    pub year: Option<u32>,
}
pub(super) async fn available() -> Result<(), ProviderError> {
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::process::Command::new("ffprobe")
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await;
    if status.is_ok_and(|s| s.is_ok_and(|s| s.success())) {
        Ok(())
    } else {
        Err(ProviderError::LocalProbeUnavailable)
    }
}
fn fallback(relative: &Path) -> bool {
    matches!(
        relative
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "wav"
            | "wave"
            | "flac"
            | "mp3"
            | "m4a"
            | "mp4"
            | "aac"
            | "ogg"
            | "oga"
            | "opus"
            | "aif"
            | "aiff"
            | "ape"
            | "wv"
            | "wma"
            | "alac"
            | "dsf"
            | "dff"
            | "mka"
            | "tta"
            | "mod"
            | "s3m"
            | "xm"
            | "it"
            | "spc"
            | "nsf"
            | "sid"
            | "amr"
            | "ac3"
            | "dts"
            | "mp2"
            | "mpc"
            | "au"
            | "caf"
            | "tak"
    )
}
pub(super) async fn read(root: &Root, relative: &Path, expected: &str) -> Option<Metadata> {
    let file = media::open(&root.path, relative).ok()?;
    if media::version(&file.metadata().ok()?) != expected {
        return None;
    }
    let mut child = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error", "-protocol_whitelist", "fd",
            "-probesize", "1048576", "-analyzeduration", "2000000",
            "-show_entries",
            concat!(
                "format=duration:format_tags=title,artist,album,album_artist,albumartist,date,track,tracknumber,disc,discnumber:",
                "stream=codec_type:stream_tags=title,artist,album,album_artist,albumartist,date,track,tracknumber,disc,discnumber"
            ),
            "-of", "json", "-i", "fd:",
        ])
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(65537)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > 65536 {
            return None;
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        serde_json::from_slice::<Value>(&bytes).ok()
    })
    .await
    .ok()
    .flatten();
    // Always kill/reap after overflow or timeout; cancellation also has kill_on_drop.
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let current = media::open(&root.path, relative).ok()?;
    if media::version(&current.metadata().ok()?) != expected {
        return None;
    }
    let Some(value) = result else {
        return fallback(relative).then(Metadata::default);
    };
    if !value["streams"]
        .as_array()
        .is_some_and(|streams| streams.iter().any(|s| s["codec_type"] == "audio"))
    {
        return None;
    }
    let mut tags = HashMap::new();
    for object in value["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| &s["tags"])
        .chain(std::iter::once(&value["format"]["tags"]))
    {
        if let Some(object) = object.as_object() {
            for (key, value) in object {
                if let Some(text) = value
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.len() <= 1024)
                {
                    tags.insert(key.to_ascii_lowercase(), text.trim().to_owned());
                }
            }
        }
    }
    let index = |key: &str| {
        tags.get(key)
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let year = tags
        .get("date")
        .and_then(|s| s.get(..4))
        .and_then(|s| s.parse().ok());
    let duration = value["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0 && *n < 31_536_000.0)
        .map(|n| (n * 1000.0) as u64);
    Some(Metadata {
        disc: index("disc").max(index("discnumber")),
        number: index("track").max(index("tracknumber")),
        year,
        duration,
        title: tags.remove("title"),
        artist: tags.remove("artist"),
        album: tags.remove("album"),
        album_artist: tags
            .remove("album_artist")
            .or_else(|| tags.remove("albumartist")),
    })
}
