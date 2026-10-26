//! Packet metadata and bounded stream framing, not an audio-payload validator.
use super::{LIMIT, Reader, Tags};
use crate::gain::{TagSource, comment_fields};
const INVALID: &str = "Invalid embedded Ogg metadata";
const LINKS: &str = "Chained or multiplexed Ogg gain is not qualified; unity gain";
const OPUS: &str = "Opus gain layout or competing normalization is not qualified; unity gain";
const WINDOW: usize = 65536;
pub(super) fn crc(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for (index, byte) in bytes.iter().enumerate() {
        crc ^= u32::from(if (22..26).contains(&index) { 0 } else { *byte }) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}
fn le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}
struct Page {
    flags: u8,
    serial: u32,
    sequence: u32,
    laces: Vec<u8>,
    body: Vec<u8>,
    end: usize,
}
fn page_bytes(bytes: &[u8]) -> Result<(usize, usize), &'static str> {
    if bytes.len() < 27 || &bytes[..4] != b"OggS" || bytes[4] != 0 || bytes[5] & !7 != 0 {
        return Err(INVALID);
    }
    let table = 27 + usize::from(bytes[26]);
    let laces = bytes.get(27..table).ok_or(INVALID)?;
    let size = table + laces.iter().map(|byte| usize::from(*byte)).sum::<usize>();
    Ok((table, size))
}
async fn page(reader: &mut Reader<'_>, pos: usize) -> Result<Page, &'static str> {
    let header = reader.get(pos, 27).await?;
    if &header[..4] != b"OggS" || header[4] != 0 || header[5] & !7 != 0 {
        return Err(INVALID);
    }
    let laces = reader.get(pos + 27, usize::from(header[26])).await?;
    let size = 27 + laces.len() + laces.iter().map(|byte| usize::from(*byte)).sum::<usize>();
    let bytes = reader.get(pos, size).await?;
    if bytes[..27] != header || bytes[27..27 + laces.len()] != laces {
        return Err(INVALID);
    }
    if crc(&bytes) != le(&bytes[22..26]) {
        return Err(INVALID);
    }
    Ok(Page {
        flags: header[5],
        serial: le(&header[14..18]),
        sequence: le(&header[18..22]),
        body: bytes[27 + laces.len()..].to_vec(),
        laces,
        end: pos.checked_add(size).ok_or(INVALID)?,
    })
}
async fn final_page(
    reader: &mut Reader<'_>,
    serial: u32,
    sequence: u32,
) -> Result<(), &'static str> {
    let start = reader.total.saturating_sub(WINDOW);
    let bytes = reader.get(start, reader.total - start).await?;
    let mut candidates = 0;
    let mut found = None;
    for (pos, magic) in bytes.windows(4).enumerate() {
        if magic != b"OggS" {
            continue;
        }
        let suffix = &bytes[pos..];
        let Ok((_, size)) = page_bytes(suffix) else {
            continue;
        };
        if size != suffix.len() {
            continue;
        }
        candidates += 1;
        if candidates > 16 {
            return Err(LIMIT);
        }
        if crc(suffix) != le(&suffix[22..26]) {
            continue;
        }
        // Multiple valid EOF-ending candidates are ambiguous, even when one
        // happens to match our serial. Never select a magic string in payload.
        if found.is_some() {
            return Err(LINKS);
        }
        found = Some((suffix[5], le(&suffix[14..18]), le(&suffix[18..22])));
    }
    let (flags, last_serial, last_sequence) = found.ok_or(INVALID)?;
    if flags & 4 == 0 || last_serial != serial || last_sequence < sequence {
        return Err(LINKS);
    }
    // Matching endpoints alone can miss an interleaved second stream. Walk all
    // page headers, skipping opaque audio bodies under the shared read budget.
    let mut pos = 0;
    let mut continued = false;
    for expected in 0..1024_u32 {
        let header = reader.get(pos, 27).await?;
        if &header[..4] != b"OggS" || header[4] != 0 || header[5] & !7 != 0 {
            return Err(INVALID);
        }
        let flags = header[5];
        if le(&header[14..18]) != serial
            || le(&header[18..22]) != expected
            || (flags & 2 != 0) != (expected == 0)
            || (flags & 1 != 0) != continued
        {
            return Err(LINKS);
        }
        let laces = reader.get(pos + 27, usize::from(header[26])).await?;
        let size = 27 + laces.len() + laces.iter().map(|n| usize::from(*n)).sum::<usize>();
        pos = pos
            .checked_add(size)
            .filter(|end| *end <= reader.total)
            .ok_or(INVALID)?;
        if let Some(last) = laces.last() {
            continued = *last == 255;
        }
        if flags & 4 != 0 {
            return if pos == reader.total && expected == last_sequence && !continued {
                Ok(())
            } else {
                Err(LINKS)
            };
        }
        if pos == reader.total {
            return Err(INVALID);
        }
    }
    Err(LIMIT)
}
#[derive(Clone, Copy)]
enum Codec {
    Vorbis,
    Opus,
}
fn codec(header: &[u8]) -> Result<Codec, &'static str> {
    if header.starts_with(b"\x01vorbis") {
        if header.len() != 30
            || header[7..11] != [0; 4]
            || header[11] == 0
            || le(&header[12..16]) == 0
            || header[29] != 1
        {
            return Err(INVALID);
        }
        let (small, large) = (header[28] & 15, header[28] >> 4);
        if !(6..=13).contains(&small) || !(small..=13).contains(&large) {
            return Err(INVALID);
        }
        Ok(Codec::Vorbis)
    } else if header.starts_with(b"OpusHead") {
        if header.len() != 19
            || header[8] != 1
            || ![1, 2].contains(&header[9])
            || header[16..18] != [0, 0]
            || header[18] != 0
        {
            return Err(OPUS);
        }
        Ok(Codec::Opus)
    } else {
        Err("Ogg gain tags unavailable for this codec")
    }
}
pub(super) async fn scan(reader: &mut Reader<'_>) -> Result<Tags, &'static str> {
    let mut pos = 0;
    let mut serial = None;
    let mut sequence = 0;
    let mut packet = Vec::new();
    let mut packets = Vec::new();
    let mut format = None;
    for index in 0..64 {
        let page = page(reader, pos).await?;
        if index == 0 {
            if page.flags & 3 != 2 || page.sequence != 0 {
                return Err(LINKS);
            }
            serial = Some(page.serial);
        } else if page.serial != serial.unwrap()
            || page.flags & 2 != 0
            || page.sequence != sequence + 1
        {
            return Err(LINKS);
        }
        if (page.flags & 1 != 0) != !packet.is_empty() {
            return Err(INVALID);
        }
        sequence = page.sequence;
        let mut offset = 0;
        for lace in &page.laces {
            let len = usize::from(*lace);
            if packet.len().saturating_add(len) > super::BYTES {
                return Err(LIMIT);
            }
            packet.extend_from_slice(&page.body[offset..offset + len]);
            offset += len;
            if *lace < 255 {
                if packets.is_empty() {
                    format = Some(codec(&packet)?);
                }
                packets.push(std::mem::take(&mut packet));
                let required = if matches!(format, Some(Codec::Vorbis)) {
                    3
                } else {
                    2
                };
                if packets.len() == required {
                    final_page(reader, serial.unwrap(), sequence).await?;
                    let (prefix, source) = match format.unwrap() {
                        Codec::Vorbis => {
                            if !packets[2].starts_with(b"\x05vorbis") {
                                return Err(INVALID);
                            }
                            (b"\x03vorbis".as_slice(), TagSource::Vorbis)
                        }
                        Codec::Opus => (b"OpusTags".as_slice(), TagSource::Opus),
                    };
                    let comments = packets[1].strip_prefix(prefix).ok_or(INVALID)?;
                    let (fields, trailing) = comment_fields(comments)?;
                    match format.unwrap() {
                        Codec::Vorbis if trailing != [1] => return Err(INVALID),
                        Codec::Opus if fields.r128 => return Err(OPUS),
                        _ => {}
                    }
                    return Ok(fields.finish(source));
                }
            }
        }
        if page.flags & 4 != 0 {
            return Err(INVALID);
        }
        pos = page.end;
    }
    Err(LIMIT)
}
