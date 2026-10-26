//! Ordinary little-endian RIFF/WAVE ID3 chunks, including tags after audio.
use super::{LIMIT, Reader, Tags};
use crate::gain::TagSource;
const INVALID: &str = "Invalid embedded WAV metadata";
pub(super) async fn scan(reader: &mut Reader<'_>) -> Result<Tags, &'static str> {
    let header = reader.get(0, 12).await?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err("WAV gain tags require ordinary RIFF; unity gain");
    }
    let end = (u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize)
        .checked_add(8)
        .ok_or(INVALID)?;
    if end != reader.total || end < 12 {
        return Err(INVALID);
    }
    let mut pos = 12;
    let mut found = None;
    for _ in 0..1024 {
        if pos == end {
            return Ok(found.unwrap_or_default());
        }
        if end.saturating_sub(pos) < 8 {
            return Err(INVALID);
        }
        let chunk = reader.get(pos, 8).await?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as usize;
        let data = pos + 8;
        let chunk_end = data
            .checked_add(size)
            .filter(|next| *next <= end)
            .ok_or(INVALID)?;
        let next = chunk_end
            .checked_add(size & 1)
            .filter(|next| *next <= end)
            .ok_or(INVALID)?;
        if &chunk[..4] == b"ID3 " || &chunk[..4] == b"id3 " {
            if found.is_some() {
                return Err("Multiple WAV ID3 chunks are not qualified; unity gain");
            }
            let (mut tags, tag_end) = super::id3::read(reader, data, chunk_end).await?;
            if tag_end != chunk_end {
                return Err(INVALID);
            }
            tags.metadata.source = Some(TagSource::WavId3);
            found = Some(tags);
        }
        pos = next;
    }
    Err(LIMIT)
}
