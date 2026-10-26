//! Exact frame spans for a deliberately narrow continuous-input profile.
//! Provider duration/bitrate hints never substitute for these source headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub rate: u32,
    pub bits: u16,
    pub frames: u64,
}
fn checked(rate: u32, bits: u16, channels: u16, frames: u64) -> Option<Description> {
    (matches!(rate, 44100 | 48000 | 96000)
        && matches!(bits, 16 | 24)
        && channels == 2
        && frames > 0)
        .then_some(Description { rate, bits, frames })
}
fn u16le(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}
fn u32le(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}
pub fn parse(prefix: &[u8], length: u64) -> Option<Description> {
    if length < prefix.len() as u64 {
        return None;
    }
    if prefix.starts_with(b"fLaC") {
        let header = prefix.get(4..8)?;
        if header[0] & 127 != 0 || header[1..] != [0, 0, 34] {
            return None;
        }
        let info = prefix.get(8..42)?;
        let packed = u64::from_be_bytes(info[10..18].try_into().ok()?);
        return checked(
            (packed >> 44) as u32,
            (((packed >> 36) & 31) + 1) as u16,
            (((packed >> 41) & 7) + 1) as u16,
            packed & ((1_u64 << 36) - 1),
        );
    }
    if prefix.get(..4)? != b"RIFF" || prefix.get(8..12)? != b"WAVE" {
        return None;
    }
    let end = u64::from(u32le(prefix.get(4..8)?)?).checked_add(8)?;
    if end != length {
        return None;
    }
    let mut offset = 12_usize;
    let mut format = None;
    for _ in 0..64 {
        let header = prefix.get(offset..offset.checked_add(8)?)?;
        let size = u32le(&header[4..8])? as usize;
        offset = offset.checked_add(8)?;
        let chunk_end = offset.checked_add(size)?.checked_add(size & 1)?;
        if chunk_end as u64 > end {
            return None;
        }
        match &header[..4] {
            b"fmt " => {
                if format.is_some() || size != 16 {
                    return None;
                }
                let data = prefix.get(offset..offset.checked_add(size)?)?;
                if u16le(&data[..2])? != 1 {
                    return None;
                }
                let channels = u16le(&data[2..4])?;
                let rate = u32le(&data[4..8])?;
                let byte_rate = u32le(&data[8..12])?;
                let align = u16le(&data[12..14])?;
                let bits = u16le(&data[14..16])?;
                checked(rate, bits, channels, 1)?;
                if align != channels.checked_mul(bits / 8)?
                    || byte_rate != rate.checked_mul(u32::from(align))?
                {
                    return None;
                }
                format = Some((rate, bits, channels, align));
            }
            b"data" => {
                let (rate, bits, channels, align) = format?;
                // Avoid inferring the first data chunk is the complete decoded
                // span when unexamined trailing chunks could contain more audio.
                // Such WAV layouts retain the independent native route.
                if chunk_end as u64 != end || !(size as u64).is_multiple_of(u64::from(align)) {
                    return None;
                }
                return checked(rate, bits, channels, size as u64 / u64::from(align));
            }
            b"LIST" => {
                if size < 4 || prefix.get(offset..offset.checked_add(4)?)? != b"INFO" {
                    return None;
                }
            }
            b"JUNK" | b"PAD " | b"id3 " | b"ID3 " | b"bext" | b"iXML" | b"axml" | b"fact"
            | b"cue " => {}
            _ => return None,
        }
        offset = chunk_end;
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    fn wave(rate: u32, bits: u16, frames: u32) -> (Vec<u8>, u64) {
        let align = 2 * bits / 8;
        let bytes = frames * u32::from(align);
        let mut header = Vec::new();
        for part in [
            b"RIFF".to_vec(),
            (bytes + 36).to_le_bytes().to_vec(),
            b"WAVEfmt ".to_vec(),
            16_u32.to_le_bytes().to_vec(),
            1_u16.to_le_bytes().to_vec(),
            2_u16.to_le_bytes().to_vec(),
            rate.to_le_bytes().to_vec(),
            (rate * u32::from(align)).to_le_bytes().to_vec(),
            align.to_le_bytes().to_vec(),
            bits.to_le_bytes().to_vec(),
            b"data".to_vec(),
            bytes.to_le_bytes().to_vec(),
        ] {
            header.extend(part);
        }
        (header, u64::from(bytes) + 44)
    }
    #[test]
    fn wave_span_uses_data_bytes_and_valid_alignment_not_duration_hints() {
        for rate in [44100, 48000, 96000] {
            for bits in [16, 24] {
                let (header, length) = wave(rate, bits, rate + 17);
                assert_eq!(
                    parse(&header, length),
                    Some(Description {
                        rate,
                        bits,
                        frames: u64::from(rate) + 17
                    })
                );
                for truncated in 0..header.len() {
                    assert_eq!(parse(&header[..truncated], length), None);
                }
                assert_eq!(parse(&header, length - 1), None);
            }
        }
        let (mut header, length) = wave(48000, 16, 48000);
        header[32] = 3;
        assert_eq!(parse(&header, length), None);
        let (header, length) = wave(48000, 16, 0);
        assert_eq!(parse(&header, length), None);
    }
    #[test]
    fn ambiguous_trailing_audio_and_wave_lists_require_independent_playback() {
        let (mut header, length) = wave(48000, 16, 48000);
        header[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        assert_eq!(parse(&header, length + 8), None);
        let (header, length) = wave(48000, 16, 48000);
        let mut list = header[..36].to_vec();
        list.extend(b"LIST\x04\0\0\0wavl");
        list.extend(&header[36..]);
        list[4..8].copy_from_slice(&((length + 12 - 8) as u32).to_le_bytes());
        assert_eq!(parse(&list, length + 12), None);
    }
    #[test]
    fn flac_total_samples_must_be_present_and_the_qualified_format_exact() {
        for rate in [44100_u32, 48000, 96000] {
            for bits in [16_u16, 24] {
                let mut header = b"fLaC\x80\0\0\x22".to_vec();
                let mut info = [0_u8; 34];
                let packed =
                    (u64::from(rate) << 44) | (1 << 41) | ((u64::from(bits) - 1) << 36) | 123457;
                info[10..18].copy_from_slice(&packed.to_be_bytes());
                header.extend(info);
                assert_eq!(
                    parse(&header, 1000),
                    Some(Description {
                        rate,
                        bits,
                        frames: 123457
                    })
                );
                header[25] &= 0xf0;
                header[26..].fill(0);
                // Use an explicit zero total-samples field rather than a duration guess.
                let packed = packed & !((1_u64 << 36) - 1);
                header[18..26].copy_from_slice(&packed.to_be_bytes());
                assert_eq!(parse(&header, 1000), None);
            }
        }
    }
}
