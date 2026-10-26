//! Bounded, provider-neutral descriptors for the deliberately narrow fidelity scope.
use futures_util::StreamExt;
use qoda_provider::{ByteRange, MediaSource};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub rate: u32,
    pub bits: u16,
}
impl Format {
    fn checked(rate: u32, bits: u16, channels: u16) -> Result<Self, &'static str> {
        if channels != 2 || !matches!(bits, 16 | 24) || !matches!(rate, 44100 | 48000 | 96000) {
            return Err(
                "Fidelity requires stereo integer 16/24-bit WAV/FLAC at 44.1, 48 or 96 kHz; choose Desktop or another track.",
            );
        }
        Ok(Self { rate, bits })
    }
}
pub fn parse(bytes: &[u8]) -> Result<Format, &'static str> {
    let invalid = "Source format could not be verified before fidelity output; choose Desktop or another track.";
    if bytes.starts_with(b"fLaC") {
        let header = bytes.get(4..8).ok_or(invalid)?;
        if header[0] & 127 != 0 || header[1..] != [0, 0, 34] {
            return Err(invalid);
        }
        let info = bytes.get(8..42).ok_or(invalid)?;
        let packed = u64::from_be_bytes(info[10..18].try_into().map_err(|_| invalid)?);
        return Format::checked(
            (packed >> 44) as u32,
            (((packed >> 36) & 31) + 1) as u16,
            (((packed >> 41) & 7) + 1) as u16,
        );
    }
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(invalid);
    }
    let mut offset = 12usize;
    for _ in 0..1024 {
        let header = bytes.get(offset..offset + 8).ok_or(invalid)?;
        let size = u32::from_le_bytes(header[4..8].try_into().map_err(|_| invalid)?) as usize;
        offset = offset.checked_add(8).ok_or(invalid)?;
        let data = bytes
            .get(offset..offset.checked_add(size).ok_or(invalid)?)
            .ok_or(invalid)?;
        if header[..4] == *b"fmt " {
            let data = data.get(..).filter(|d| d.len() >= 16).ok_or(invalid)?;
            let encoding = u16::from_le_bytes([data[0], data[1]]);
            let channels = u16::from_le_bytes([data[2], data[3]]);
            let rate = u32::from_le_bytes(data[4..8].try_into().map_err(|_| invalid)?);
            let byte_rate = u32::from_le_bytes(data[8..12].try_into().map_err(|_| invalid)?);
            let align = u16::from_le_bytes([data[12], data[13]]);
            let container = u16::from_le_bytes([data[14], data[15]]);
            let bits = if encoding == 1 {
                container
            } else if encoding == 0xfffe && data.len() >= 40 {
                let valid = u16::from_le_bytes([data[18], data[19]]);
                let mask = u32::from_le_bytes(data[20..24].try_into().map_err(|_| invalid)?);
                if u16::from_le_bytes([data[16], data[17]]) < 22
                    || mask != 3
                    || data[24..40] != [1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]
                {
                    return Err(invalid);
                }
                valid
            } else {
                return Err(invalid);
            };
            if !matches!(container, 16 | 24 | 32)
                || bits > container
                || u32::from(align) != u32::from(channels) * u32::from(container) / 8
                || rate.checked_mul(u32::from(align)) != Some(byte_rate)
            {
                return Err(invalid);
            }
            return Format::checked(rate, bits, channels);
        }
        if header[..4] == *b"data" {
            return Err(invalid);
        }
        offset = offset.checked_add(size + (size & 1)).ok_or(invalid)?;
    }
    Err(invalid)
}
pub async fn probe(source: &dyn MediaSource) -> Result<Format, &'static str> {
    let mut response = source
        .open(
            Some(ByteRange::try_from("bytes=0-65535").map_err(|_| "Invalid source range")?),
            false,
        )
        .await
        .map_err(|_| "Source format read failed; fidelity stays paused.")?;
    if !matches!(response.status, 200 | 206)
        || response.status == 206
            && !response
                .content_range
                .as_deref()
                .is_some_and(|s| s.starts_with("bytes 0-"))
    {
        return Err("Source format range was not confirmed; fidelity stays paused.");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.body.next().await {
        let chunk = chunk.map_err(|_| "Source format read interrupted; fidelity stays paused.")?;
        bytes.extend_from_slice(&chunk[..chunk.len().min(65536 - bytes.len())]);
        if let Ok(format) = parse(&bytes) {
            return Ok(format);
        }
        if bytes.len() == 65536 {
            break;
        }
    }
    parse(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wave(rate: u32, bits: u16, channels: u16) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend(36_u32.to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        bytes.extend((channels * bits / 8).to_le_bytes());
        bytes.extend(bits.to_le_bytes());
        bytes
    }
    #[test]
    fn exact_lossless_headers_are_required_instead_of_provider_hints() {
        for rate in [44100, 48000, 96000] {
            for bits in [16, 24] {
                assert_eq!(parse(&wave(rate, bits, 2)).unwrap(), Format { rate, bits });
            }
        }
        for bytes in [
            wave(48000, 24, 1),
            wave(192000, 24, 2),
            wave(48000, 32, 2),
            b"ID3 fake metadata".to_vec(),
        ] {
            assert!(parse(&bytes).is_err());
        }
        let mut invalid = wave(48000, 16, 2);
        invalid[20] = 3;
        assert!(parse(&invalid).is_err());
        let valid = wave(48000, 24, 2);
        for size in 0..valid.len() {
            assert!(parse(&valid[..size]).is_err());
        }
    }
    #[test]
    fn flac_streaminfo_is_parsed_from_its_fixed_width_fields() {
        let mut bytes = b"fLaC\x80\x00\x00\x22".to_vec();
        let mut info = [0u8; 34];
        let fields = (96000u64 << 44) | (1 << 41) | (23 << 36) | 96000;
        info[10..18].copy_from_slice(&fields.to_be_bytes());
        bytes.extend(info);
        assert_eq!(
            parse(&bytes).unwrap(),
            Format {
                rate: 96000,
                bits: 24
            }
        );
        bytes[7] = 33;
        assert!(parse(&bytes).is_err());
    }
}
