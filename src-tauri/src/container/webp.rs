//! Strict still WebP container validation and lossless metadata removal.
//! Rendering chunks are copied verbatim; only VP8X metadata flags and RIFF size change.
use super::{dimensions, AppResult, Format, Structure, MAX_METADATA};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

fn uint24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

pub(super) fn parse(data: &[u8]) -> AppResult<Structure> {
    let declared =
        u32::from_le_bytes(data[4..8].try_into().map_err(|_| "WebP size error")?) as usize;
    if declared < 4 || !declared.is_multiple_of(2) || declared.checked_add(8) != Some(data.len()) {
        return Err("WebP RIFF size mismatch, truncation or trailing data.".into());
    }
    let mut offset = 12usize;
    let mut chunks = Vec::new();
    let mut residual = Vec::new();
    let mut seen = HashSet::new();
    let mut canvas = None;
    let mut image_size = None;
    let mut flags = 0u8;
    let mut alpha = false;
    let mut metadata_size = 0usize;
    let mut clean = data[..12].to_vec();
    while offset < data.len() {
        if chunks.len() >= 10000 {
            return Err("Too many WebP chunks.".into());
        }
        let header = data
            .get(offset..offset + 8)
            .ok_or("Truncated WebP chunk header.")?;
        let kind: &[u8; 4] = header[..4].try_into().map_err(|_| "WebP type error")?;
        if !kind.iter().all(|v| (0x20..=0x7e).contains(v)) {
            return Err("Invalid WebP chunk identifier.".into());
        }
        let len =
            u32::from_le_bytes(header[4..8].try_into().map_err(|_| "WebP length error")?) as usize;
        let end = offset
            .checked_add(8)
            .and_then(|v| v.checked_add(len))
            .ok_or("WebP size overflow.")?;
        let padded_end = end.checked_add(len % 2).ok_or("WebP padding overflow.")?;
        let raw = data
            .get(offset..padded_end)
            .ok_or("Truncated WebP chunk or padding.")?;
        let payload = &raw[8..8 + len];
        if !len.is_multiple_of(2) && raw.last() != Some(&0) {
            return Err("WebP padding must be zero.".into());
        }
        let known = matches!(
            kind,
            b"VP8X" | b"ICCP" | b"ALPH" | b"VP8 " | b"VP8L" | b"EXIF" | b"XMP "
        );
        if known && !seen.insert(*kind) {
            return Err(format!(
                "Duplicate WebP chunk: {}.",
                String::from_utf8_lossy(kind)
            ));
        }
        let rendering = matches!(kind, b"VP8X" | b"ICCP" | b"ALPH" | b"VP8 " | b"VP8L");
        if !matches!(kind, b"VP8 " | b"VP8L" | b"ALPH") {
            metadata_size = metadata_size
                .checked_add(len)
                .ok_or("WebP metadata size overflow.")?;
            if metadata_size > MAX_METADATA {
                return Err("WebP metadata exceeds the 8 MiB limit.".into());
            }
        }
        match kind {
            b"VP8X" => {
                if !chunks.is_empty()
                    || len != 10
                    || payload[0] & 0xc1 != 0
                    || payload[1..4] != [0; 3]
                {
                    return Err("Invalid WebP extended header or reserved feature bits.".into());
                }
                flags = payload[0];
                if flags & 0x02 != 0 {
                    return Err("Animated WebP is not supported; animation would require separate preservation checks.".into());
                }
                let size = (uint24(&payload[4..7]) + 1, uint24(&payload[7..10]) + 1);
                dimensions(size.0, size.1)?;
                canvas = Some(size);
            }
            b"ANIM" | b"ANMF" => return Err("Animated WebP is not supported.".into()),
            b"ICCP" => {
                if canvas.is_none() || image_size.is_some() || seen.contains(b"ALPH") || len < 128 {
                    return Err("Invalid or misplaced WebP ICC profile.".into());
                }
                if payload[36..40] != *b"acsp"
                    || u32::from_be_bytes(payload[..4].try_into().map_err(|_| "ICC size error")?)
                        as usize
                        != len
                {
                    return Err("Invalid WebP ICC profile header or size.".into());
                }
            }
            b"ALPH" => {
                if canvas.is_none()
                    || image_size.is_some()
                    || len < 2
                    || payload[0] & 0xc0 != 0
                    || payload[0] & 0x03 > 1
                    || (payload[0] >> 4) & 0x03 > 1
                {
                    return Err("Invalid or misplaced WebP alpha chunk.".into());
                }
                alpha = true;
            }
            b"VP8 " => {
                if alpha && chunks.last().map(String::as_str) != Some("ALPH") {
                    return Err("WebP alpha chunk must immediately precede VP8 image data.".into());
                }
                if image_size.is_some()
                    || len < 10
                    || payload[0] & 1 != 0
                    || (payload[0] >> 1) & 7 > 3
                    || payload[0] & 0x10 == 0
                    || payload[3..6] != [0x9d, 1, 0x2a]
                {
                    return Err("Invalid WebP VP8 still-image header.".into());
                }
                let width = u16::from_le_bytes([payload[6], payload[7]]);
                let height = u16::from_le_bytes([payload[8], payload[9]]);
                if width & 0xc000 != 0 || height & 0xc000 != 0 {
                    return Err("Scaled WebP VP8 images are not supported.".into());
                }
                image_size = Some((u32::from(width), u32::from(height)));
            }
            b"VP8L" => {
                if image_size.is_some()
                    || seen.contains(b"ALPH")
                    || len < 5
                    || payload[0] != 0x2f
                    || payload[4] & 0xe0 != 0
                {
                    return Err("Invalid WebP VP8L still-image header.".into());
                }
                let bits =
                    u32::from_le_bytes(payload[1..5].try_into().map_err(|_| "VP8L header error")?);
                image_size = Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1));
                alpha = bits & (1 << 28) != 0;
            }
            b"EXIF" | b"XMP " if canvas.is_none() => {
                return Err("WebP metadata requires an extended header.".into());
            }
            _ => {}
        }
        if rendering {
            let start = clean.len();
            clean.extend_from_slice(raw);
            if kind == b"VP8X" {
                clean[start + 8] &= !0x0c; // Clear only removed EXIF/XMP flags.
            }
        } else {
            residual.push(String::from_utf8_lossy(kind).into_owned());
        }
        chunks.push(String::from_utf8_lossy(kind).into_owned());
        offset = padded_end;
    }
    let (width, height) = image_size.ok_or("WebP image data is missing.")?;
    dimensions(width, height)?;
    if let Some(size) = canvas {
        if size != (width, height) {
            return Err("WebP canvas and encoded image dimensions disagree.".into());
        }
        for (bit, present) in [
            (0x20, seen.contains(b"ICCP")),
            (0x10, alpha),
            (0x08, seen.contains(b"EXIF")),
            (0x04, seen.contains(b"XMP ")),
        ] {
            if (flags & bit != 0) != present {
                return Err("WebP feature flags do not match its chunks.".into());
            }
        }
    }
    let size = u32::try_from(clean.len() - 8).map_err(|_| "WebP output size overflow")?;
    clean[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(Structure {
        format: Format::Webp,
        width,
        height,
        fingerprint: Sha256::digest(&clean).into(),
        chunks,
        sanitized_container: Some(clean),
        residual_regions: residual,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut data = b"RIFF\0\0\0\0WEBP".to_vec();
        for (kind, payload) in chunks {
            data.extend_from_slice(*kind);
            data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            data.extend_from_slice(payload);
            if !payload.len().is_multiple_of(2) {
                data.push(0);
            }
        }
        let len = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&len.to_le_bytes());
        data
    }

    #[test]
    fn rejects_ambiguous_malformed_animated_and_oversized_containers() {
        let header = [0, 0, 0, 0, 17, 0, 0, 11, 0, 0]; // 18×12 canvas.
        let vp8 = &include_bytes!("../../tests/fixtures/lossy.webp")[20..];
        let vp8_len = u32::from_le_bytes(
            include_bytes!("../../tests/fixtures/lossy.webp")[16..20]
                .try_into()
                .unwrap(),
        ) as usize;
        let vp8 = &vp8[..vp8_len];
        let valid = wrap(&[(b"VP8X", &header), (b"VP8 ", vp8)]);
        assert!(super::super::parse(&valid).is_ok());
        for size in 0..valid.len() {
            assert!(super::super::parse(&valid[..size]).is_err());
        }
        let mut invalids = vec![
            wrap(&[(b"VP8X", &header)]),
            wrap(&[(b"VP8 ", vp8), (b"VP8 ", vp8)]),
            wrap(&[(b"VP8X", &header), (b"VP8X", &header), (b"VP8 ", vp8)]),
            wrap(&[(b"VP8 ", vp8), (b"VP8X", &header)]),
            wrap(&[(b"VP8X", &header), (b"ANIM", &[0; 6]), (b"VP8 ", vp8)]),
            wrap(&[(b"VP8X", &header), (b"ANMF", &[0; 16]), (b"VP8 ", vp8)]),
            wrap(&[
                (b"VP8X", &header),
                (b"EXIF", b"x"),
                (b"EXIF", b"y"),
                (b"VP8 ", vp8),
            ]),
            wrap(&[(b"VP8X", &header), (b"VP8 ", vp8), (b"ICCP", &[0; 128])]),
            wrap(&[(b"EXIF", b"x"), (b"VP8 ", vp8)]),
            wrap(&[
                (b"VP8X", &header),
                (b"ALPH", &[0; 2]),
                (b"prIv", b"x"),
                (b"VP8 ", vp8),
            ]),
            wrap(&[(b"VP8X", &header), (b"ALPH", &[0xc0; 2]), (b"VP8 ", vp8)]),
            wrap(&[(b"VP8X", &header), (b"VP8L", &[0x2f, 0, 0, 0, 0xe0])]),
            wrap(&[(b"VP8X", &header), (b"VP8 ", &[0; 10])]),
            wrap(&[
                (b"VP8X", &header),
                (b"VP8 ", vp8),
                (b"prIv", &vec![0; MAX_METADATA + 1]),
            ]),
        ];
        for flag in [1, 2, 4, 8, 16, 32, 64, 128] {
            let mut other = valid.clone();
            other[20] = flag;
            invalids.push(other);
        }
        for (at, value) in [(21, 1), (24, 18), (27, 12), (24, 255), (16, 11)] {
            let mut other = valid.clone();
            other[at] = value;
            invalids.push(other);
        }
        let mut trailer = valid.clone();
        trailer.push(42);
        invalids.push(trailer);
        let mut bad_padding = wrap(&[(b"VP8 ", vp8), (b"prIv", b"x")]);
        *bad_padding.last_mut().unwrap() = 1;
        invalids.push(bad_padding);
        let mut overflow = valid.clone();
        overflow[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        invalids.push(overflow);
        for (index, data) in invalids.iter().enumerate() {
            assert!(
                super::super::parse(data).is_err(),
                "Malformed WebP case {index} accepted"
            );
        }
    }

    #[test]
    fn private_chunks_are_removed_and_rendering_tampering_changes_fingerprint() {
        let original = include_bytes!("../../tests/fixtures/lossy-alpha.webp");
        let mut data = original.to_vec();
        data.extend_from_slice(&wrap(&[(b"prIv", b"private workflow")])[12..]);
        let len = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&len.to_le_bytes());
        let before = super::super::parse(&data).unwrap();
        let clean = before.sanitized_container.unwrap();
        assert_eq!(clean, original);
        assert_eq!(before.residual_regions, ["prIv"]);
        assert!(super::super::parse(&clean)
            .unwrap()
            .residual_regions
            .is_empty());
        // Change encoded payload while leaving the RIFF structure valid.
        let mut changed = clean.clone();
        let last = changed.len() - 2;
        changed[last] ^= 1;
        assert_ne!(
            before.fingerprint,
            super::super::parse(&changed).unwrap().fingerprint
        );
        // Alpha changes are rendering changes too.
        let alpha = clean.windows(4).position(|v| v == b"ALPH").unwrap();
        let mut changed = clean;
        changed[alpha + 9] ^= 1;
        assert_ne!(
            before.fingerprint,
            super::super::parse(&changed).unwrap().fingerprint
        );
    }
}
