//! One footer-delimited APEv2 instance; binary artwork is never downloaded.
use super::{INCOMPLETE, LIMIT, Reader, SCALAR, SPAN};
use crate::gain::{Fields, TagSource, Tags};
const INVALID: &str = "Invalid embedded APEv2 tags";
fn le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

pub(super) async fn tail(reader: &mut Reader<'_>) -> Result<(Option<Tags>, usize), &'static str> {
    let mut end = reader.total;
    let mut footer = reader.get(end.saturating_sub(32), end.min(32)).await?;
    // Probe the actual EOF first; TAG-like audio bytes cannot hide a valid footer.
    if !footer.starts_with(b"APETAGEX") && end >= 128 {
        let trailer = reader.get(end - 128, 128).await?;
        if trailer.starts_with(b"TAG") {
            end -= 128;
            footer = reader.get(end.saturating_sub(32), end.min(32)).await?;
        }
    }
    if !footer.starts_with(b"APETAGEX") {
        // A recognizable partial footer is not evidence that tags are absent.
        if footer.windows(4).enumerate().any(|(offset, prefix)| {
            let suffix = &footer[offset..];
            prefix == b"APET"
                && suffix.len() < 32
                && suffix[..suffix.len().min(8)] == b"APETAGEX"[..suffix.len().min(8)]
        }) {
            return Err(INCOMPLETE);
        }
        return Ok((None, end));
    }
    if footer.len() != 32 || le(&footer[8..12]) != 2000 {
        return Err("Unsupported embedded APE version");
    }
    let size = le(&footer[12..16]) as usize;
    let count = le(&footer[16..20]) as usize;
    let flags = le(&footer[20..24]);
    if size < 32
        || count > 16384
        || flags & 0x7fff_ffff != 0
        || footer[24..].iter().any(|byte| *byte != 0)
    {
        return Err(INVALID);
    }
    if size > SPAN {
        return Err(LIMIT);
    }
    let start = end.checked_sub(size).ok_or(INCOMPLETE)?;
    let mut tag_start = start;
    if flags & 0x8000_0000 != 0 {
        tag_start = start.checked_sub(32).ok_or(INCOMPLETE)?;
        let header = reader.get(tag_start, 32).await?;
        if header[..20] != footer[..20]
            || le(&header[20..24]) != flags | 0x2000_0000
            || header[24..] != footer[24..]
        {
            return Err(INVALID);
        }
    }
    let items_end = end - 32;
    let mut position = start;
    let mut fields = Fields::default();
    for _ in 0..count {
        if items_end.saturating_sub(position) < 10 {
            return Err(INVALID);
        }
        let item = reader.get(position, 8).await?;
        let length = le(&item[..4]) as usize;
        let flags = le(&item[4..]);
        if flags & !7 != 0 || flags & 6 == 6 {
            return Err(INVALID);
        }
        position += 8;
        let key_bytes = reader
            .get(position, (items_end - position).min(256))
            .await?;
        let key_len = key_bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(INVALID)?;
        if !(2..=255).contains(&key_len)
            || !key_bytes[..key_len]
                .iter()
                .all(|byte| (32..=126).contains(byte) && *byte != b'=')
        {
            return Err(INVALID);
        }
        let key = std::str::from_utf8(&key_bytes[..key_len]).map_err(|_| INVALID)?;
        position = position.checked_add(key_len + 1).ok_or(INVALID)?;
        let next = position
            .checked_add(length)
            .filter(|next| *next <= items_end)
            .ok_or(INVALID)?;
        if Fields::index(key).is_some() {
            if flags & 6 != 0 || length > SCALAR {
                fields.push(key, None);
            } else {
                let value = reader.get(position, length).await?;
                let text = std::str::from_utf8(&value)
                    .ok()
                    .filter(|text| !text.contains('\0'));
                fields.push(key, text);
            }
        }
        position = next;
    }
    if position != items_end {
        return Err(INVALID);
    }
    Ok((Some(fields.finish(TagSource::Ape)), tag_start))
}
