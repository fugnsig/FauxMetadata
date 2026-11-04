//! Reproducible, bounded mutational/property fuzzing on stable Windows Rust.
//! This is a development executable; it is not linked into FauxMetadata.
use fauxmetadata_lib::{
    container,
    engine::{self, Source},
    exiftool::ExifTool,
};
use flate2::{write::ZlibEncoder, Compression};
use image::{DynamicImage, ImageBuffer, Rgba};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Write},
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::Instant,
};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
    fn index(&mut self, max: usize) -> usize {
        if max == 0 {
            0
        } else {
            self.next() as usize % max
        }
    }
    fn bytes(&mut self, count: usize) -> Vec<u8> {
        (0..count).map(|_| self.next() as u8).collect()
    }
}
fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(payload);
    out.extend(crc32fast::hash(&out[4..]).to_be_bytes());
    out
}
fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut out = ZlibEncoder::new(vec![], Compression::default());
    out.write_all(bytes).unwrap();
    out.finish().unwrap()
}
fn raster(format: image::ImageFormat) -> Vec<u8> {
    let img = DynamicImage::ImageRgba8(ImageBuffer::from_fn(32, 24, |x, y| {
        Rgba([(x * 7) as u8, (y * 9) as u8, 128, ((x + y) * 4) as u8])
    }));
    let img = if format == image::ImageFormat::Jpeg {
        DynamicImage::ImageRgb8(img.to_rgb8())
    } else {
        img
    };
    let mut out = Cursor::new(vec![]);
    img.write_to(&mut out, format).unwrap();
    out.into_inner()
}
fn palette() -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = vec![];
    header.extend(32u32.to_be_bytes());
    header.extend(24u32.to_be_bytes());
    header.extend([1, 3, 0, 0, 0]);
    data.extend(chunk(b"IHDR", &header));
    data.extend(chunk(b"PLTE", &[20, 70, 80, 190, 225, 160]));
    data.extend(chunk(b"tRNS", &[40, 255]));
    let rows: Vec<_> = (0..24)
        .flat_map(|y| [0, if y % 2 == 0 { 0xaa } else { 0x55 }, 0x55, 0xaa, 0x55])
        .collect();
    data.extend(chunk(b"IDAT", &zlib(&rows)));
    data.extend(chunk(b"IEND", &[]));
    data
}
fn png_chunks(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut chunks = vec![];
    let mut offset = 8;
    while offset + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let Some(end) = offset.checked_add(len + 12) else {
            break;
        };
        if end > bytes.len() {
            break;
        }
        chunks.push((offset, end));
        offset = end;
    }
    chunks
}
fn riff_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend((payload.len() as u32).to_le_bytes());
    out.extend(payload);
    if !payload.len().is_multiple_of(2) {
        out.push(0);
    }
    out
}
fn riff_size(data: &mut [u8]) {
    let len = (data.len() - 8) as u32;
    data[4..8].copy_from_slice(&len.to_le_bytes());
}
fn webp_metadata(data: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8], flag: u8) {
    if data.get(12..16) != Some(b"VP8X") {
        let structure = container::parse(data).unwrap();
        let mut header = [0u8; 10];
        // The simple lossless header's alpha hint is retained when promoting to VP8X.
        if data.get(12..16) == Some(b"VP8L") && data[24] & 0x10 != 0 {
            header[0] = 0x10;
        }
        header[4..7].copy_from_slice(&(structure.width - 1).to_le_bytes()[..3]);
        header[7..10].copy_from_slice(&(structure.height - 1).to_le_bytes()[..3]);
        data.splice(12..12, riff_chunk(b"VP8X", &header));
    }
    data[20] |= flag;
    data.extend(riff_chunk(kind, payload));
    riff_size(data);
}
fn riff_chunks(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut offset = 12;
    while offset + 8 <= data.len() {
        let len = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let Some(end) = offset.checked_add(8 + len + len % 2) else {
            break;
        };
        if end > data.len() {
            break;
        }
        out.push((offset, end));
        offset = end;
    }
    out
}
fn mutate(seed: &[u8], rng: &mut Rng, operation: usize) -> Vec<u8> {
    let mut data = seed.to_vec();
    match operation {
        0 => {
            for _ in 0..1 + rng.index(16) {
                let at = rng.index(data.len());
                data[at] ^= 1 << rng.index(8);
            }
        }
        1 => data.truncate(rng.index(data.len() + 1)),
        2 => {
            let at = rng.index(data.len() + 1);
            let len = 1 + rng.index(64);
            data.splice(at..at, rng.bytes(len));
        }
        3 => {
            let at = rng.index(data.len());
            let end = (at + 1 + rng.index(64)).min(data.len());
            data.drain(at..end);
        }
        4 if data.starts_with(b"\x89PNG") => {
            let chunks = png_chunks(&data);
            let (start, end) = chunks[rng.index(chunks.len())];
            if end > start + 12 {
                let at = start + 8 + rng.index(end - start - 12);
                data[at] ^= rng.next() as u8;
                let crc = crc32fast::hash(&data[start + 4..end - 4]);
                data[end - 4..end].copy_from_slice(&crc.to_be_bytes());
            }
        }
        5 if data.starts_with(b"\x89PNG") => {
            let chunks = png_chunks(&data);
            let (start, end) = chunks[rng.index(chunks.len())];
            let copy = data[start..end].to_vec();
            data.splice(end..end, copy);
        }
        6 if data.starts_with(b"\x89PNG") => {
            let chunks = png_chunks(&data);
            let (start, _) = chunks[rng.index(chunks.len())];
            let lengths = [0u32, 1, 0x7fffffff, u32::MAX];
            data[start..start + 4]
                .copy_from_slice(&lengths[rng.index(lengths.len())].to_be_bytes());
        }
        4 if data.starts_with(b"RIFF") => {
            let chunks = riff_chunks(&data);
            let (start, end) = chunks[rng.index(chunks.len())];
            if end > start + 8 {
                let at = start + 8 + rng.index(end - start - 8);
                data[at] ^= rng.next() as u8;
            }
        }
        5 if data.starts_with(b"RIFF") => {
            let chunks = riff_chunks(&data);
            let (start, end) = chunks[rng.index(chunks.len())];
            let copy = data[start..end].to_vec();
            data.splice(end..end, copy);
            riff_size(&mut data);
        }
        6 if data.starts_with(b"RIFF") => {
            let chunks = riff_chunks(&data);
            let (start, _) = chunks[rng.index(chunks.len())];
            let lengths = [0u32, 1, 0x7fffffff, u32::MAX];
            data[start + 4..start + 8]
                .copy_from_slice(&lengths[rng.index(lengths.len())].to_le_bytes());
        }
        7 => {
            let count = 1 + rng.index(512);
            data.extend(rng.bytes(count));
        }
        8 if data.starts_with(b"\x89PNG") => {
            let mut text = b"workflow\0\0".to_vec();
            let count = rng.index(2048);
            text.extend(zlib(&rng.bytes(count)));
            let at = data.len() - 12;
            data.splice(at..at, chunk(b"zTXt", &text));
        }
        9 if data.starts_with(b"\x89PNG") => {
            let count = rng.index(2048);
            let mut text = b"prompt\0\x01\0\0\0".to_vec();
            text.extend(rng.bytes(count));
            let at = data.len() - 12;
            data.splice(at..at, chunk(b"iTXt", &text));
        }
        8 if data.starts_with(b"RIFF") => {
            let count = rng.index(2048);
            data.extend(riff_chunk(b"prIv", &rng.bytes(count)));
            riff_size(&mut data);
        }
        9 if data.starts_with(b"RIFF") => {
            let at = 12 + rng.index(10);
            data[at] = rng.next() as u8;
        }
        10 if data.starts_with(&[0xff, 0xd8]) => {
            let len = 1 + rng.index(1024);
            let mut segment = vec![0xff, 0xe0 + rng.index(16) as u8];
            segment.extend(((len + 2) as u16).to_be_bytes());
            segment.extend(rng.bytes(len));
            data.splice(2..2, segment);
        }
        11 => {
            let len = rng.index(4096);
            data = rng.bytes(len);
        }
        _ => {
            let at = rng.index(data.len());
            data[at] = rng.next() as u8;
        }
    }
    data
}
fn record_failure(directory: &std::path::Path, index: usize, data: &[u8], reason: &str) {
    fs::write(directory.join(format!("failure-{index}.bin")), data).unwrap();
    fs::write(directory.join(format!("failure-{index}.txt")), reason).unwrap();
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let argument = |key: &str, default: u64| {
        args.windows(2)
            .find(|pair| pair[0] == key)
            .and_then(|pair| pair[1].parse().ok())
            .unwrap_or(default)
    };
    let cases = argument("--cases", 200_000) as usize;
    let exports = argument("--exports", 128) as usize;
    let malformed = argument("--malformed", 0) as usize;
    let seed = argument("--seed", 20261104);
    assert!(
        seed != 0 && cases <= 5_000_000 && exports <= 10000 && malformed <= 10000,
        "Invalid fuzz campaign bounds"
    );
    let report =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../artifacts/fuzz/seed-{seed}"));
    fs::create_dir_all(&report).unwrap();
    let mut rng = Rng(seed);
    let started = Instant::now();
    let mut corpus = vec![
        raster(image::ImageFormat::Png),
        palette(),
        raster(image::ImageFormat::Jpeg),
        include_bytes!("../tests/fixtures/progressive.jpg").to_vec(),
        include_bytes!("../tests/fixtures/cmyk.jpg").to_vec(),
        raster(image::ImageFormat::WebP),
        include_bytes!("../tests/fixtures/lossy.webp").to_vec(),
        include_bytes!("../tests/fixtures/lossy-alpha.webp").to_vec(),
    ];
    let webp_only = args.iter().any(|v| v == "--webp-only");
    if webp_only {
        corpus.retain(|v| v.starts_with(b"RIFF"));
    }
    let mut accepted = 0usize;
    let mut decoded = 0usize;
    let mut failures = 0usize;
    let mut max_case_ms = 0u128;
    let mut operations = [0usize; 12];
    for index in 0..cases {
        let start = Instant::now();
        let operation = rng.index(12);
        operations[operation] += 1;
        let data = mutate(&corpus[rng.index(corpus.len())], &mut rng, operation);
        let outcome = std::panic::catch_unwind(|| -> Result<(bool, bool), String> {
            let structure = match container::parse(&data) {
                Ok(value) => value,
                Err(_) => return Ok((false, false)),
            };
            // Keep decoder fuzz cases small; production large-dimension limits are separate regressions.
            let small = u64::from(structure.width) * u64::from(structure.height) <= 1_000_000;
            let pixels = if small {
                engine::decode(&data).ok()
            } else {
                None
            };
            if let Some(clean) = structure.sanitized_container {
                let after = container::parse(&clean)
                    .map_err(|e| format!("Normalised container became invalid: {e}"))?;
                if structure.fingerprint != after.fingerprint || !after.residual_regions.is_empty()
                {
                    return Err("Container normalisation invariant failed".into());
                }
                if let Some(pixels) = &pixels {
                    let other = engine::decode(&clean)
                        .map_err(|e| format!("Normalised container no longer decodes: {e}"))?;
                    if pixels.color() != other.color() || pixels.as_bytes() != other.as_bytes() {
                        return Err("Container normalisation changed decoded pixels".into());
                    }
                }
            }
            Ok((true, pixels.is_some()))
        });
        match outcome {
            Ok(Ok((parsed, pixels))) => {
                accepted += usize::from(parsed);
                decoded += usize::from(pixels)
            }
            Ok(Err(error)) => {
                record_failure(&report, index, &data, &error);
                failures += 1
            }
            Err(_) => {
                record_failure(&report, index, &data, "Parser/decoder panic");
                failures += 1
            }
        }
        max_case_ms = max_case_ms.max(start.elapsed().as_millis());
        if index % 20000 == 19999 {
            println!("parse/decode fuzz: {} / {cases}, parsed {accepted}, decoded {decoded}, failures {failures}",index+1);
        }
    }
    let source_dir = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let tool = ExifTool {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/exiftool/exiftool.exe"),
    };
    tool.ensure_version(&AtomicBool::new(false)).unwrap();
    let mut published = 0usize;
    let mut valid_published = 0usize;
    let mut malformed_published = 0usize;
    let mut rejected = 0usize;
    let mut reasons = BTreeMap::<String, usize>::new();
    let total_exports = exports + malformed;
    for index in 0..total_exports {
        let adversarial = index >= exports;
        let mut data = corpus[index % corpus.len()].clone();
        let marker = format!("FAUXMETADATA_PRIVATE_FUZZ_{seed}_{index}");
        let mut value = marker.clone();
        for _ in 0..rng.index(2048) {
            value.push_str(
                ["x", "\"", "'", "<", ">", "&", "\\", "\n", "\t", "花", "é"][rng.index(11)],
            );
        }
        let webp = data.starts_with(b"RIFF");
        if webp {
            data.extend(riff_chunk(b"prIv", value.as_bytes()));
            riff_size(&mut data);
        } else if data.starts_with(b"\x89PNG") {
            let graph =
                json!({"1":{"class_type":"KSampler","inputs":{"seed":rng.next(),"text":value}}})
                    .to_string();
            let mut payload = b"prompt\0\0\0\0\0".to_vec();
            payload.extend(graph.as_bytes());
            let at = data.len() - 12;
            data.splice(at..at, chunk(b"iTXt", &payload));
            let mut workflow = b"workflow\0\0".to_vec();
            workflow.extend(zlib(
                json!({"nodes":[{"id":index,"title":marker}],"links":[]})
                    .to_string()
                    .as_bytes(),
            ));
            let at = data.len() - 12;
            data.splice(at..at, chunk(b"zTXt", &workflow));
        } else {
            let comment = value.as_bytes();
            let mut segment = vec![0xff, 0xfe];
            segment.extend(((comment.len() + 2) as u16).to_be_bytes());
            segment.extend(comment);
            data.splice(2..2, segment);
        }
        let png = data.starts_with(b"\x89PNG");
        if adversarial {
            let noise = rng.bytes(64);
            if webp {
                match index % 4 {
                    0 => {
                        data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
                    }
                    1 => {
                        data.extend(riff_chunk(b"ANMF", &noise));
                        riff_size(&mut data);
                    }
                    2 => {
                        webp_metadata(&mut data, b"EXIF", &noise, 0x08);
                    }
                    _ => {
                        webp_metadata(&mut data, b"XMP ", &noise, 0x04);
                    }
                }
            } else if png {
                let (kind, payload) = match index % 4 {
                    0 => {
                        let mut bytes = b"II*\0\xf0\xff\xff\xff".to_vec();
                        bytes.extend(&noise);
                        (b"eXIf", bytes)
                    }
                    1 => {
                        let mut bytes = b"workflow\0\0".to_vec();
                        bytes.extend(&noise);
                        (b"zTXt", bytes)
                    }
                    2 => (b"iTXt", b"prompt\0\x02\0\0\0invalid".to_vec()),
                    _ => {
                        let mut bytes = b"workflow\0".to_vec();
                        bytes.extend(&noise);
                        (b"tEXt", bytes)
                    }
                };
                let at = data.len() - 12;
                data.splice(at..at, chunk(kind, &payload));
            } else {
                let (marker, payload) = match index % 4 {
                    0 => {
                        let mut bytes = b"Exif\0\0II*\0\xf0\xff\xff\xff".to_vec();
                        bytes.extend(&noise);
                        (0xe1, bytes)
                    }
                    1 => {
                        let mut bytes =
                            b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta><rdf:RDF>".to_vec();
                        bytes.extend(&noise);
                        (0xe1, bytes)
                    }
                    2 => {
                        let mut bytes = b"Photoshop 3.0\0".to_vec();
                        bytes.extend(b"8BIM\x04\x04\0\0\xff\xff\xff\xf0");
                        bytes.extend(&noise);
                        (0xed, bytes)
                    }
                    _ => {
                        let mut bytes = b"ICC_PROFILE\0\x01\x01".to_vec();
                        bytes.extend(&noise);
                        (0xe2, bytes)
                    }
                };
                let mut segment = vec![0xff, marker];
                segment.extend(((payload.len() + 2) as u16).to_be_bytes());
                segment.extend(payload);
                data.splice(2..2, segment);
            }
        }
        if !webp && index % 7 == 0 {
            data.extend(marker.as_bytes());
        }
        let name = format!(
            "-fuzz_{index}_花 [draft] & '={{}}.{}",
            if webp {
                "webp"
            } else if png {
                "png"
            } else {
                "jpg"
            }
        );
        let path = source_dir.path().join(name);
        fs::write(&path, &data).unwrap();
        let source = Source {
            path: path.clone(),
            hash: Sha256::digest(&data).into(),
            presentation_bytes: 0,
        };
        // Pre-existing collision sentinel must remain intact.
        let collision = destination.path().join(format!(
            "{}_clean.{}",
            path.file_stem().unwrap().to_string_lossy(),
            if webp {
                "webp"
            } else if png {
                "png"
            } else {
                "jpg"
            }
        ));
        fs::write(&collision, b"existing output sentinel").unwrap();
        let outcome = std::panic::catch_unwind(|| {
            engine::export_one(
                &tool,
                &source,
                "fuzz",
                destination.path(),
                &AtomicBool::new(false),
                |_| {},
            )
        });
        let mut failure = None;
        match outcome {
            Ok(Ok(result)) => {
                let out = fs::read(result.output.unwrap()).unwrap();
                published += 1;
                if adversarial {
                    malformed_published += 1
                } else {
                    valid_published += 1
                }
                if out.windows(marker.len()).any(|v| v == marker.as_bytes()) {
                    failure = Some("Private fuzz marker survived publication".to_string())
                }
                if !result.encoded_verified || !result.pixels_verified {
                    failure = Some("Published result lacked verification".to_string())
                }
            }
            Ok(Err(error)) => {
                rejected += 1;
                *reasons.entry(error).or_default() += 1;
            }
            Err(_) => failure = Some("Export pipeline panicked".into()),
        }
        if fs::read(&path).unwrap() != data {
            failure = Some("Original source was modified".into())
        }
        if fs::read(&collision).unwrap() != b"existing output sentinel" {
            failure = Some("Existing output was overwritten".into())
        }
        if fs::read_dir(destination.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".fauxmetadata-")
        }) {
            failure = Some("Uncommitted staging leaked".into())
        }
        if let Some(reason) = failure {
            record_failure(&report, cases + index, &data, &reason);
            failures += 1;
        }
        if index % 8 == 7 {
            println!("real export fuzz: {} / {total_exports}, published {published}, safely rejected {rejected}, failures {failures}",index+1);
        }
    }
    // A suite that rejects every valid carrier would be safe but unusable.
    if exports > 0 && valid_published * 4 < exports * 3 {
        failures += 1;
    }
    let summary = json!({"seed":seed,"mutationCases":cases,"operations":operations,"structurallyAccepted":accepted,"decodedCases":decoded,"realExports":total_exports,"validCarriers":exports,"validPublished":valid_published,"malformedMetadataCases":malformed,"malformedPublishedAfterSanitization":malformed_published,"published":published,"safelyRejected":rejected,"rejectionReasons":reasons,"failures":failures,"maxParserDecoderCaseMs":max_case_ms,"elapsedSeconds":started.elapsed().as_secs_f64(),"engine":"ExifTool 13.60","method":"seeded mutational and property fuzzing; not coverage-guided libFuzzer"});
    fs::write(
        report.join("report.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    if failures > 0 {
        std::process::exit(1)
    }
}
