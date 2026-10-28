//! On-demand bounded images; audio is opened read-only and never rewritten.
use super::*;
use std::{
    fs::File,
    io::{Read, Seek},
    process::Stdio,
};
use tokio::io::AsyncReadExt;

async fn output(mut command: tokio::process::Command, limit: u64) -> Option<Vec<u8>> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        (bytes.len() as u64 <= limit && child.wait().await.ok()?.success()).then_some(bytes)
    })
    .await
    .ok()
    .flatten();
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}
async fn image(mut file: File, embedded: bool) -> Option<Vec<u8>> {
    if !embedded && file.metadata().ok()?.len() > 16 * 1024 * 1024 {
        return None;
    }
    if !embedded && file.metadata().ok()?.len() <= 256 * 1024 {
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if artwork::bounded_jpeg(&bytes) {
            return Some(bytes);
        }
        file.rewind().ok()?;
    }
    let mut probe = tokio::process::Command::new("ffprobe");
    probe
        .args([
            "-v",
            "error",
            "-max_alloc",
            "67108864",
            "-threads",
            "1",
            "-protocol_whitelist",
            "fd",
            "-probesize",
            "1048576",
            "-analyzeduration",
            "2000000",
            "-show_entries",
            "stream=index,codec_type,width,height:stream_disposition=attached_pic",
            "-of",
            "json",
            "-i",
            "fd:",
        ])
        .stdin(Stdio::from(file.try_clone().ok()?));
    let metadata: Value = serde_json::from_slice(&output(probe, 65536).await?).ok()?;
    let stream = metadata["streams"].as_array()?.iter().find(|s| {
        let w = s["width"].as_u64().unwrap_or(0);
        let h = s["height"].as_u64().unwrap_or(0);
        s["codec_type"] == "video"
            && (!embedded || s["disposition"]["attached_pic"] == 1)
            && w > 0
            && h > 0
            && w <= 8192
            && h <= 8192
            && w * h <= 16_777_216
    })?;
    let index = stream["index"].as_u64()?;
    file.rewind().ok()?;
    let mut decode = tokio::process::Command::new("ffmpeg");
    decode
        .args([
            "-nostdin",
            "-v",
            "error",
            "-max_alloc",
            "67108864",
            "-threads",
            "1",
            "-filter_threads",
            "1",
            "-protocol_whitelist",
            "fd",
            "-probesize",
            "1048576",
            "-analyzeduration",
            "2000000",
            "-i",
            "fd:",
            "-map",
        ])
        .arg(format!("0:{index}"))
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=320:320:force_original_aspect_ratio=decrease",
            "-c:v",
            "mjpeg",
            "-q:v",
            "3",
            "-f",
            "image2pipe",
            "pipe:1",
        ])
        .stdin(Stdio::from(file));
    output(decode, 256 * 1024)
        .await
        .filter(|bytes| artwork::bounded_jpeg(bytes))
}
fn checked(entry: &Entry) -> Result<File, ProviderError> {
    let file = media::open(&entry.root.path, &entry.relative)?;
    if media::version(
        &file
            .metadata()
            .map_err(|_| ProviderError::LocalUnavailable)?,
    ) != entry.version
    {
        return Err(ProviderError::LocalChanged);
    }
    Ok(file)
}
pub(super) async fn read(entries: Vec<Arc<Entry>>) -> Result<Vec<u8>, ProviderError> {
    for entry in entries.iter().take(4) {
        let file = checked(entry)?;
        let sidecar = cover_sidecar(&entry.relative);
        if let Ok(cover) = media::open(&entry.root.path, &sidecar)
            && let Some(bytes) = image(cover, false).await
        {
            checked(entry)?;
            return Ok(bytes);
        }
        if let Some(bytes) = image(file, true).await {
            checked(entry)?;
            return Ok(bytes);
        }
        let parent = entry.relative.parent().unwrap_or(Path::new(""));
        for name in [
            "cover.jpg",
            "folder.jpg",
            "front.jpg",
            "cover.png",
            "folder.png",
        ] {
            if let Ok(cover) = media::open(&entry.root.path, &parent.join(name))
                && let Some(bytes) = image(cover, false).await
            {
                checked(entry)?;
                return Ok(bytes);
            }
        }
    }
    Err(ProviderError::MissingTrack)
}
