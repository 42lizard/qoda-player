//! Bounded ID3v2.3/v2.4 TXXX; tag instances and APE fields never pool values.
use super::{INCOMPLETE, LIMIT, Reader, SCALAR, SPAN, Tags, audio_signature, candidate};
use crate::gain::{Fields, TagSource};
const INVALID: &str = "Invalid embedded ID3 metadata";
const UNSUPPORTED: &str = "Embedded ID3 feature is not supported; unity gain";
const FRAMES: usize = 1024;
fn synchsafe(bytes: &[u8]) -> Result<usize, &'static str> {
    if bytes.len() != 4 || bytes.iter().any(|byte| byte & 0x80 != 0) {
        return Err(INVALID);
    }
    Ok(bytes
        .iter()
        .fold(0, |n, byte| (n << 7) | usize::from(*byte)))
}
fn undo(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        out.push(byte);
        i += 1;
        if byte == 0xff {
            if bytes.get(i) == Some(&0) {
                i += 1;
            } else if bytes.get(i).is_none_or(|next| *next >= 0xe0) {
                return Err(INVALID);
            }
        }
    }
    Ok(out)
}
#[derive(Clone, Copy, PartialEq)]
enum Endian {
    Le,
    Be,
}
fn unicode(bytes: &[u8], endian: Endian) -> Result<String, &'static str> {
    if !bytes.len().is_multiple_of(2) {
        return Err(INVALID);
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| match endian {
            Endian::Le => u16::from_le_bytes([pair[0], pair[1]]),
            Endian::Be => u16::from_be_bytes([pair[0], pair[1]]),
        })
        .collect();
    String::from_utf16(&units).map_err(|_| INVALID)
}
fn bom(bytes: &[u8]) -> Option<Endian> {
    match bytes.get(..2) {
        Some([0xff, 0xfe]) => Some(Endian::Le),
        Some([0xfe, 0xff]) => Some(Endian::Be),
        _ => None,
    }
}
fn txxx(bytes: &[u8], version: u8, fields: &mut Fields) -> Result<(), &'static str> {
    let (&encoding, text) = bytes.split_first().ok_or(INVALID)?;
    if encoding > 3 || version == 3 && encoding > 1 {
        return Err(UNSUPPORTED);
    }
    let (description, value) = if encoding == 1 || encoding == 2 {
        let delimiter = text
            .as_chunks::<2>()
            .0
            .iter()
            .position(|pair| *pair == [0, 0])
            .ok_or(INVALID)?
            * 2;
        let (description, rest) = text.split_at(delimiter);
        let endian = if encoding == 1 {
            bom(description).ok_or(INVALID)?
        } else {
            Endian::Be
        };
        if encoding == 2 && bom(description).is_some() {
            return Err(INVALID);
        }
        let description = unicode(
            if encoding == 1 {
                &description[2..]
            } else {
                description
            },
            endian,
        )?;
        let mut value = &rest[2..];
        if let Some(order) = bom(value) {
            if encoding != 1 || order != endian {
                return Err(INVALID);
            }
            value = &value[2..];
        }
        // A missing second BOM inherits the verified description's byte order;
        // no arbitrary encoding guess or lossy Unicode replacement is used.
        (description, unicode(value, endian).ok())
    } else {
        let delimiter = text.iter().position(|byte| *byte == 0).ok_or(INVALID)?;
        let decode = |bytes: &[u8]| -> Result<String, &'static str> {
            if encoding == 0 {
                Ok(bytes.iter().map(|byte| char::from(*byte)).collect())
            } else {
                std::str::from_utf8(bytes)
                    .map(str::to_owned)
                    .map_err(|_| INVALID)
            }
        };
        (
            decode(&text[..delimiter])?,
            decode(&text[delimiter + 1..]).ok(),
        )
    };
    let value = value.map(|mut value| {
        if value.ends_with('\0') {
            value.pop();
        }
        value
    });
    fields.push(
        &description,
        value.as_deref().filter(|value| !value.contains('\0')),
    );
    Ok(())
}
struct Body<'a, 's> {
    reader: &'a mut Reader<'s>,
    base: usize,
    decoded: Option<Vec<u8>>,
    len: usize,
}
impl Body<'_, '_> {
    async fn get(&mut self, pos: usize, size: usize) -> Result<Vec<u8>, &'static str> {
        let end = pos
            .checked_add(size)
            .filter(|end| *end <= self.len)
            .ok_or(INVALID)?;
        if let Some(bytes) = &self.decoded {
            return Ok(bytes[pos..end].to_vec());
        }
        self.reader
            .get(self.base.checked_add(pos).ok_or(INVALID)?, size)
            .await
    }
}
pub(super) async fn read(
    reader: &mut Reader<'_>,
    start: usize,
    limit: usize,
) -> Result<(Tags, usize), &'static str> {
    let header = reader.get(start, 10).await?;
    let (version, flags) = (header[3], header[5]);
    if &header[..3] != b"ID3" || ![3, 4].contains(&version) || header[4] != 0 {
        return Err(UNSUPPORTED);
    }
    if flags & if version == 3 { 0x1f } else { 0x0f } != 0 {
        return Err(INVALID);
    }
    let size = synchsafe(&header[6..10])?;
    if size > SPAN {
        return Err(LIMIT);
    }
    let base = start.checked_add(10).ok_or(INVALID)?;
    let body_end = base.checked_add(size).ok_or(INVALID)?;
    let footer = version == 4 && flags & 0x10 != 0;
    let end = body_end
        .checked_add(if footer { 10 } else { 0 })
        .filter(|end| *end <= limit && *end <= reader.total)
        .ok_or(INCOMPLETE)?;
    if footer {
        let tail = reader.get(body_end, 10).await?;
        if &tail[..3] != b"3DI" || tail[3..] != header[3..] {
            return Err(INVALID);
        }
    }
    let decoded = if version == 3 && flags & 0x80 != 0 {
        Some(undo(&reader.get(base, size).await?)?)
    } else {
        None
    };
    let len = decoded.as_ref().map_or(size, Vec::len);
    let mut body = Body {
        reader,
        base,
        decoded,
        len,
    };
    let mut pos = 0;
    let mut padding = None;
    if flags & 0x40 != 0 {
        let first = body.get(0, 6).await?;
        if version == 3 {
            let ext = u32::from_be_bytes(first[..4].try_into().unwrap()) as usize;
            if first[4..] != [0, 0] || ext != 6 {
                return Err(UNSUPPORTED);
            }
            let rest = body.get(6, 4).await?;
            padding = Some(u32::from_be_bytes(rest.try_into().unwrap()) as usize);
            pos = 10;
        } else {
            let ext = synchsafe(&first[..4])?;
            if ext != 6 || first[4] != 1 || first[5] != 0 {
                return Err(UNSUPPORTED);
            }
            pos = ext;
        }
    }
    let frame_end = len
        .checked_sub(padding.unwrap_or(0))
        .filter(|end| *end >= pos)
        .ok_or(INVALID)?;
    if let Some(size) = padding
        && size > 0
        && body
            .get(frame_end, size.min(10))
            .await?
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(INVALID);
    }
    let mut fields = Fields::default();
    for _ in 0..FRAMES {
        if pos == frame_end {
            return Ok((fields.finish(TagSource::Id3), end));
        }
        let left = frame_end - pos;
        let frame = body.get(pos, left.min(10)).await?;
        if frame.iter().all(|byte| *byte == 0) {
            if footer || padding.is_some() {
                return Err(INVALID);
            }
            // Padding is an opaque declared remainder, like FLAC padding. Its
            // first framing bytes are checked; no later valid frame can follow.
            return Ok((fields.finish(TagSource::Id3), end));
        }
        if frame.len() != 10
            || !frame[..4]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(INVALID);
        }
        let size = if version == 3 {
            u32::from_be_bytes(frame[4..8].try_into().unwrap()) as usize
        } else {
            synchsafe(&frame[4..8])?
        };
        let (status, format) = (frame[8], frame[9]);
        if status & if version == 3 { 0x1f } else { 0x8f } != 0
            || format & if version == 3 { 0x1f } else { 0xb0 } != 0
        {
            return Err(UNSUPPORTED);
        }
        if version == 4 && format & 8 != 0 && format & 1 == 0 {
            return Err(INVALID);
        }
        let data = pos.checked_add(10).ok_or(INVALID)?;
        let next = data
            .checked_add(size)
            .filter(|next| *next <= frame_end)
            .ok_or(INVALID)?;
        if size == 0 {
            return Err(INVALID);
        }
        if &frame[..4] == b"SEEK" {
            return Err("Updated or linked ID3 tags are not qualified; unity gain");
        }
        if &frame[..4] == b"TXXX" {
            if size > SCALAR {
                return Err(LIMIT);
            }
            if version == 3 && format & 0xc0 != 0 || version == 4 && format & 0x0c != 0 {
                return Err(UNSUPPORTED);
            }
            let bytes = body.get(data, size).await?;
            let bytes = if version == 4 && (flags & 0x80 != 0 || format & 2 != 0) {
                undo(&bytes)?
            } else {
                bytes
            };
            let group = if version == 3 {
                format & 0x20 != 0
            } else {
                format & 0x40 != 0
            };
            let mut payload = bytes.get(usize::from(group)..).ok_or(INVALID)?;
            if version == 4 && format & 1 != 0 {
                let declared = synchsafe(payload.get(..4).ok_or(INVALID)?)?;
                payload = &payload[4..];
                if declared != payload.len() {
                    return Err(INVALID);
                }
            }
            txxx(payload, version, &mut fields)?;
        }
        pos = next;
    }
    Err(LIMIT)
}
async fn appended(reader: &mut Reader<'_>, end: usize) -> Result<Option<usize>, &'static str> {
    if end < 10 {
        return Ok(None);
    }
    let footer = reader.get(end - 10, 10).await?;
    if &footer[..3] != b"3DI" {
        return Ok(None);
    }
    if footer[3] != 4 || footer[4] != 0 || footer[5] & 0x10 == 0 {
        return Err(INVALID);
    }
    let size = synchsafe(&footer[6..])?;
    Ok(Some(
        end.checked_sub(size.checked_add(20).ok_or(INVALID)?)
            .ok_or(INVALID)?,
    ))
}
pub(super) async fn scan(reader: &mut Reader<'_>) -> Result<Tags, &'static str> {
    let (ape, tail) = super::ape::tail(reader).await?;
    let mut found = None;
    let mut prefix_end = 0;
    if reader.prefix.starts_with(b"ID3") {
        let (tags, end) = read(reader, 0, tail).await?;
        prefix_end = end;
        candidate(&mut found, tags)?;
    }
    let mut audio_end = tail;
    if let Some(start) = appended(reader, tail).await? {
        if start == 0 && prefix_end == tail {
            audio_end = tail;
        } else {
            if prefix_end != 0 {
                return Err("Multiple ID3 tag instances are not qualified; unity gain");
            }
            if start < prefix_end {
                return Err(INVALID);
            }
            let (tags, end) = read(reader, start, tail).await?;
            if end != tail {
                return Err(INVALID);
            }
            candidate(&mut found, tags)?;
            audio_end = start;
        }
    }
    if prefix_end.checked_add(4).is_none_or(|end| end > audio_end) {
        return Err(INCOMPLETE);
    }
    if !audio_signature(&reader.get(prefix_end, 4).await?) {
        return Err("ID3-wrapped container is not qualified; unity gain");
    }
    if let Some(tags) = ape {
        candidate(&mut found, tags)?;
    }
    Ok(found.unwrap_or_default())
}
