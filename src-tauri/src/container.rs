//! Structural verification deliberately supports conservative still-image subsets.
mod webp;
use crate::model::AppResult;
use sha2::{Digest, Sha256};
use std::io::Read;

pub const MAX_FILE: usize = 128 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 40_000_000;
pub const MAX_METADATA: usize = 8 * 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_KEEP: &[&[u8; 4]] = &[
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"cHRM", b"gAMA", b"iCCP", b"sRGB", b"sBIT",
    b"bKGD", b"pHYs",
];

#[derive(Clone, Debug, PartialEq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

impl Format {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
        }
    }
}

#[derive(Debug)]
pub struct Structure {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub fingerprint: [u8; 32],
    pub chunks: Vec<String>,
    pub sanitized_container: Option<Vec<u8>>,
    pub residual_regions: Vec<String>,
}

fn dimensions(width: u32, height: u32) -> AppResult<()> {
    if width == 0
        || height == 0
        || width > 32768
        || height > 32768
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(
            "Image dimensions exceed the supported limit (40 megapixels, 32768 per side).".into(),
        );
    }
    Ok(())
}

pub fn parse(data: &[u8]) -> AppResult<Structure> {
    if data.len() > MAX_FILE {
        return Err("Image exceeds the 128 MiB file limit.".into());
    }
    if data.starts_with(PNG_SIGNATURE) {
        png(data)
    } else if data.starts_with(&[0xff, 0xd8]) {
        jpeg(data)
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        webp::parse(data)
    } else {
        Err("Unsupported image. FauxMetadata accepts PNG, JPEG and still WebP files only.".into())
    }
}

fn png(data: &[u8]) -> AppResult<Structure> {
    let mut offset = 8usize;
    let mut keep = PNG_SIGNATURE.to_vec();
    let mut chunks = vec![];
    let mut residual = vec![];
    let mut width = 0;
    let mut height = 0;
    let mut seen_idat = false;
    let mut ended_idat = false;
    let mut ended = false;
    let mut seen = std::collections::HashSet::new();
    let mut metadata_size = 0usize;
    while offset < data.len() {
        if chunks.len() >= 10000 {
            return Err("Too many PNG chunks.".into());
        }
        let header = data
            .get(offset..offset + 8)
            .ok_or("Truncated PNG chunk header.")?;
        let len =
            u32::from_be_bytes(header[..4].try_into().map_err(|_| "PNG length error")?) as usize;
        let kind: &[u8; 4] = header[4..8].try_into().map_err(|_| "PNG type error")?;
        if !kind.iter().all(u8::is_ascii_alphabetic) || kind[2].is_ascii_lowercase() {
            return Err("Invalid PNG chunk type.".into());
        }
        let end = offset
            .checked_add(len)
            .and_then(|v| v.checked_add(12))
            .ok_or("PNG size overflow.")?;
        let raw = data.get(offset..end).ok_or("Truncated PNG chunk.")?;
        let expected = u32::from_be_bytes(
            raw[raw.len() - 4..]
                .try_into()
                .map_err(|_| "PNG CRC error")?,
        );
        if crc32fast::hash(&raw[4..raw.len() - 4]) != expected {
            return Err("PNG chunk CRC mismatch; file may be damaged.".into());
        }
        let payload = &raw[8..raw.len() - 4];
        metadata_size = metadata_size
            .checked_add(metadata_cost(kind, payload)?)
            .ok_or("PNG metadata size overflow.")?;
        if metadata_size > MAX_METADATA {
            return Err("PNG metadata exceeds the 8 MiB expanded-data limit.".into());
        }
        let name = String::from_utf8_lossy(kind).into_owned();
        if chunks.is_empty() && kind != b"IHDR" {
            return Err("PNG must start with IHDR.".into());
        }
        if kind != b"IDAT" && !seen.insert(*kind) && PNG_KEEP.contains(&kind) {
            return Err(format!("Duplicate rendering chunk: {name}."));
        }
        match kind {
            b"IHDR" => {
                if len != 13 {
                    return Err("Invalid PNG header.".into());
                }
                width = u32::from_be_bytes(payload[..4].try_into().map_err(|_| "PNG width error")?);
                height =
                    u32::from_be_bytes(payload[4..8].try_into().map_err(|_| "PNG height error")?);
                dimensions(width, height)?;
            }
            b"acTL" | b"fcTL" | b"fdAT" => {
                return Err(
                    "Animated PNG is not supported; export would risk changing animation.".into(),
                )
            }
            b"cICP" | b"mDCV" | b"cLLI" => {
                return Err("HDR PNG colour metadata is not supported yet.".into())
            }
            b"IDAT" => {
                if ended_idat {
                    return Err("Non-consecutive PNG image data.".into());
                }
                seen_idat = true;
            }
            b"IEND" => {
                if len != 0 || !seen_idat {
                    return Err("Invalid PNG end marker or missing image data.".into());
                }
                ended = true;
            }
            _ if kind[0].is_ascii_uppercase() && kind != b"PLTE" => {
                return Err(format!("Unknown critical PNG chunk: {name}."))
            }
            _ => {}
        }
        if seen_idat && kind != b"IDAT" {
            ended_idat = true;
        }
        if PNG_KEEP.contains(&kind) {
            keep.extend_from_slice(raw);
        } else {
            residual.push(name.clone());
        }
        chunks.push(name);
        offset = end;
        if ended {
            break;
        }
    }
    if !ended {
        return Err("PNG is missing IEND.".into());
    }
    if offset < data.len() {
        chunks.push("Trailer".into());
        residual.push("Trailer".into());
    }
    let fingerprint = Sha256::digest(&keep).into();
    Ok(Structure {
        format: Format::Png,
        width,
        height,
        fingerprint,
        chunks,
        sanitized_container: Some(keep),
        residual_regions: residual,
    })
}

fn metadata_cost(kind: &[u8; 4], payload: &[u8]) -> AppResult<usize> {
    let compressed = match kind {
        b"zTXt" | b"iCCP" => {
            let nul = payload
                .iter()
                .position(|v| *v == 0)
                .ok_or("Invalid PNG compressed metadata header.")?;
            if !(1..=79).contains(&nul) || payload.get(nul + 1) != Some(&0) {
                return Err("Invalid PNG metadata compression method.".into());
            }
            Some(
                payload
                    .get(nul + 2..)
                    .ok_or("Truncated PNG compressed metadata.")?,
            )
        }
        b"iTXt" => {
            let nul = payload
                .iter()
                .position(|v| *v == 0)
                .ok_or("Invalid PNG international text header.")?;
            if !(1..=79).contains(&nul) || payload.get(nul + 2) != Some(&0) {
                return Err("Invalid PNG international text compression method.".into());
            }
            let flag = *payload.get(nul + 1).ok_or("Truncated PNG text.")?;
            if flag > 1 {
                return Err("Invalid PNG text compression flag.".into());
            }
            let mut start = nul + 3;
            for _ in 0..2 {
                let length = payload
                    .get(start..)
                    .and_then(|v| v.iter().position(|b| *b == 0))
                    .ok_or("Truncated PNG text language header.")?;
                start += length + 1;
            }
            if flag == 1 {
                Some(&payload[start..])
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(compressed) = compressed {
        let mut decoded = vec![];
        flate2::read::ZlibDecoder::new(compressed)
            .take(MAX_METADATA as u64 + 1)
            .read_to_end(&mut decoded)
            .map_err(|e| format!("Invalid compressed PNG metadata: {e}"))?;
        if decoded.len() > MAX_METADATA {
            return Err("Compressed PNG metadata exceeds the 8 MiB expansion limit.".into());
        }
        Ok(decoded.len().saturating_add(payload.len()))
    } else if !PNG_KEEP.contains(&kind) {
        Ok(payload.len())
    } else {
        Ok(0)
    }
}

fn jpeg(data: &[u8]) -> AppResult<Structure> {
    let mut pos = 2;
    let mut hash = Sha256::new();
    hash.update([0xff, 0xd8]);
    let mut chunks = vec![];
    let mut residual = vec![];
    let mut width = 0;
    let mut height = 0;
    let mut scans = 0;
    let mut ended = false;
    while pos < data.len() {
        if chunks.len() >= 10000 {
            return Err("Too many JPEG segments.".into());
        }
        let start = pos;
        if data[pos] != 0xff {
            return Err("Invalid JPEG marker boundary.".into());
        }
        while data.get(pos) == Some(&0xff) {
            pos += 1;
        }
        let marker = *data.get(pos).ok_or("Truncated JPEG marker.")?;
        pos += 1;
        if marker == 0xd9 {
            hash.update(&data[start..pos]);
            ended = true;
            chunks.push("EOI".into());
            break;
        }
        if marker == 0 || marker == 0xd8 || (0xd0..=0xd7).contains(&marker) {
            return Err("Unexpected JPEG standalone marker.".into());
        }
        let size_bytes = data
            .get(pos..pos + 2)
            .ok_or("Truncated JPEG segment size.")?;
        let size = u16::from_be_bytes([size_bytes[0], size_bytes[1]]) as usize;
        if size < 2 {
            return Err("Invalid JPEG segment size.".into());
        }
        let end = pos.checked_add(size).ok_or("JPEG size overflow.")?;
        let payload = data.get(pos + 2..end).ok_or("Truncated JPEG segment.")?;
        let app = (0xe0..=0xef).contains(&marker);
        if (0xc0..=0xcf).contains(&marker) && ![0xc0, 0xc1, 0xc2, 0xc4].contains(&marker) {
            return Err(
                "Unsupported JPEG encoding; baseline and progressive Huffman JPEG are supported."
                    .into(),
            );
        }
        if [0xc0, 0xc1, 0xc2].contains(&marker) {
            if payload.len() < 6 || payload[0] != 8 || width != 0 {
                return Err("Unsupported JPEG frame header.".into());
            }
            height = u16::from_be_bytes([payload[1], payload[2]]) as u32;
            width = u16::from_be_bytes([payload[3], payload[4]]) as u32;
            dimensions(width, height)?;
        }
        let label = if marker == 0xe1 && payload.starts_with(b"Exif\0\0") {
            "EXIF".into()
        } else if app {
            format!("APP{}", marker - 0xe0)
        } else {
            format!("{marker:02X}")
        };
        chunks.push(label.clone());
        let keep_app = match marker {
            0xe0 if payload.starts_with(b"JFIF\0") => {
                if payload.len() != 14 || payload[12] != 0 || payload[13] != 0 {
                    return Err("JPEG JFIF thumbnails or extensions are not supported.".into());
                }
                true
            }
            0xe2 if payload.starts_with(b"ICC_PROFILE\0") => true,
            0xee if payload.starts_with(b"Adobe") && payload.len() == 12 => true,
            _ => false,
        };
        if (marker == 0xe2 && !keep_app)
            || marker == 0xeb
            || (marker == 0xe1
                && (payload.windows(7).any(|v| v == b"hdrgm:V")
                    || payload.windows(11).any(|v| v == b"hdr-gain-map")))
        {
            return Err("Extended, multi-picture, or HDR JPEG is not supported.".into());
        }
        if !app && marker != 0xfe || keep_app {
            hash.update(&data[start..end]);
        }
        if app && !keep_app || marker == 0xfe {
            residual.push(label);
        }
        pos = end;
        if marker == 0xda {
            scans += 1;
            let scan_start = pos;
            loop {
                if pos >= data.len() {
                    return Err("Truncated JPEG scan.".into());
                }
                if data[pos] != 0xff {
                    pos += 1;
                    continue;
                }
                let mut next = pos + 1;
                while data.get(next) == Some(&0xff) {
                    next += 1;
                }
                match data.get(next) {
                    Some(0x00) | Some(0xd0..=0xd7) => pos = next + 1,
                    Some(_) => break,
                    None => return Err("Truncated JPEG scan marker.".into()),
                }
            }
            hash.update(&data[scan_start..pos]);
        }
    }
    if !ended || width == 0 || scans == 0 {
        return Err("Incomplete JPEG image.".into());
    }
    if pos < data.len() {
        chunks.push("Trailer".into());
        residual.push("Trailer".into());
    }
    Ok(Structure {
        format: Format::Jpeg,
        width,
        height,
        fingerprint: hash.finalize().into(),
        chunks,
        sanitized_container: None,
        residual_regions: residual,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_never_panics() {
        for size in 0..2048 {
            let mut bytes = vec![0xff; size];
            if size >= 8 {
                bytes[..8].copy_from_slice(PNG_SIGNATURE);
            }
            assert!(parse(&bytes).is_err());
            if size >= 2 {
                bytes[..2].copy_from_slice(&[0xff, 0xd8]);
            }
            assert!(parse(&bytes).is_err());
        }
    }
}
