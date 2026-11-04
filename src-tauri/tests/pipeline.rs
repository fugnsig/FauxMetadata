use fauxmetadata_lib::{
    container,
    engine::{self, Engine, Source},
    exiftool::{self, ExifTool},
    preferences::PreferenceStore,
};

#[test]
fn quick_clean_default_destination_and_restart_keep_verified_collision_safe_copies() {
    let root = tempfile::tempdir().unwrap();
    let source_dir = root.path().join("input");
    fs::create_dir(&source_dir).unwrap();
    let original = comfy_png();
    let source = write_source(&source_dir, "Comfy 花.png", &original);
    let settings = root.path().join("config/preferences.json");
    let pictures = root.path().join("Redirected Pictures");
    let mut preferences = PreferenceStore::load(settings.clone(), Ok(pictures.clone()));
    assert!(!pictures.exists());
    let default = preferences.prepare_export().unwrap();
    let first = export(&tool(), &source, &default).unwrap();
    let second = export(&tool(), &source, &default).unwrap();
    assert!(first
        .output
        .as_ref()
        .unwrap()
        .ends_with("Comfy 花_clean.png"));
    assert!(second
        .output
        .as_ref()
        .unwrap()
        .ends_with("Comfy 花_clean (1).png"));
    assert_eq!(
        fs::read(first.output.unwrap()).unwrap(),
        fs::read(second.output.unwrap()).unwrap()
    );
    let custom = root.path().join("My Outputs");
    fs::create_dir(&custom).unwrap();
    preferences.choose(&custom).unwrap();
    let mut restarted = PreferenceStore::load(settings.clone(), Ok(pictures.clone()));
    let third = export(&tool(), &source, &restarted.prepare_export().unwrap()).unwrap();
    assert!(third.encoded_verified && third.pixels_verified);
    assert!(
        Path::new(third.output.as_ref().unwrap()).starts_with(fs::canonicalize(&custom).unwrap())
    );
    restarted.reset().unwrap();
    let reset = PreferenceStore::load(settings, Ok(pictures));
    let fourth = export(&tool(), &source, &reset.prepare_export().unwrap()).unwrap();
    assert!(fourth.output.unwrap().ends_with("Comfy 花_clean (2).png"));
    assert_eq!(fs::read(source.path).unwrap(), original);
    assert_eq!(fs::read_dir(default).unwrap().count(), 3);
}
use flate2::{write::ZlibEncoder, Compression};
use image::{DynamicImage, ImageBuffer, Rgb};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

fn tool() -> ExifTool {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/exiftool/exiftool.exe");
    assert!(
        path.is_file(),
        "Bundled ExifTool is required; these integration tests must not silently skip."
    );
    ExifTool { path }
}
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32fast::hash(&out[4..]).to_be_bytes());
    out
}
fn compressed(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}
fn raster(format: image::ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(18, 12, |x, y| {
        Rgb([(x * 13) as u8, (y * 19) as u8, ((x + y) * 7) as u8])
    }));
    let mut out = Cursor::new(vec![]);
    image.write_to(&mut out, format).unwrap();
    out.into_inner()
}
fn add_png(data: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    let at = data.len() - 12;
    data.splice(at..at, chunk(kind, payload));
}
fn profile() -> Vec<u8> {
    // A small synthetic display profile. No external, proprietary test assets.
    let mut text = b"desc\0\0\0\0".to_vec();
    let description = b"FauxMetadata synthetic profile\0";
    text.extend_from_slice(&(description.len() as u32).to_be_bytes());
    text.extend_from_slice(description);
    text.extend_from_slice(&[0; 78]);
    let mut p = vec![0; 144];
    p[128..132].copy_from_slice(&1u32.to_be_bytes());
    p[132..136].copy_from_slice(b"desc");
    p[136..140].copy_from_slice(&144u32.to_be_bytes());
    p[140..144].copy_from_slice(&(text.len() as u32).to_be_bytes());
    p.extend(text);
    while !p.len().is_multiple_of(4) {
        p.push(0);
    }
    let length = p.len() as u32;
    p[..4].copy_from_slice(&length.to_be_bytes());
    p[8..12].copy_from_slice(&[2, 0x10, 0, 0]);
    p[12..16].copy_from_slice(b"mntr");
    p[16..20].copy_from_slice(b"RGB ");
    p[20..24].copy_from_slice(b"XYZ ");
    for (index, value) in [2026u16, 1, 1, 0, 0, 0].iter().enumerate() {
        p[24 + index * 2..26 + index * 2].copy_from_slice(&value.to_be_bytes());
    }
    p[36..40].copy_from_slice(b"acsp");
    p[40..44].copy_from_slice(b"MSFT");
    p[68..72].copy_from_slice(&63190i32.to_be_bytes());
    p[72..76].copy_from_slice(&65536i32.to_be_bytes());
    p[76..80].copy_from_slice(&54061i32.to_be_bytes());
    p
}
fn comfy_png() -> Vec<u8> {
    let mut data = raster(image::ImageFormat::Png);
    add_png(
        &mut data,
        b"tEXt",
        b"workflow\0{\"nodes\":[{\"id\":1,\"type\":\"KSampler\"}],\"links\":[]}",
    );
    add_png(
        &mut data,
        b"tEXt",
        b"prompt\0{\"1\":{\"class_type\":\"KSampler\",\"inputs\":{\"seed\":1234567}}}",
    );
    add_png(&mut data, b"tEXt", b"prompt\0duplicate private prompt");
    let mut z = b"parameters\0\0".to_vec();
    z.extend(compressed(
        b"private prompt, Steps: 20, Seed: 42, Model: private.safetensors",
    ));
    add_png(&mut data, b"zTXt", &z);
    add_png(
        &mut data,
        b"iTXt",
        b"Comment\0\0\0en\0\0<private> Unicode: \xe8\x8a\xb1",
    );
    add_png(&mut data, b"tEXt", b"Software\0ComfyUI");
    add_png(&mut data, b"vpAg", b"private unknown ancillary data");
    // Rendering chunks belong before image data.
    let mut appearance = chunk(b"gAMA", &45455u32.to_be_bytes());
    appearance.extend(chunk(b"pHYs", &[0, 0, 0x0e, 0xc4, 0, 0, 0x0e, 0xc4, 1]));
    let mut iccp = b"FauxMetadata synthetic\0\0".to_vec();
    iccp.extend(compressed(&profile()));
    appearance.extend(chunk(b"iCCP", &iccp));
    data.splice(33..33, appearance);
    data.extend_from_slice(b"PRIVATE TRAILER");
    data
}
fn write_source(dir: &Path, name: &str, data: &[u8]) -> Source {
    let path = dir.join(name);
    fs::write(&path, data).unwrap();
    Source {
        path,
        hash: Sha256::digest(data).into(),
        presentation_bytes: 0,
    }
}
fn export(
    tool: &ExifTool,
    source: &Source,
    dest: &Path,
) -> fauxmetadata_lib::model::AppResult<fauxmetadata_lib::model::ExportResult> {
    engine::export_one(tool, source, "1", dest, &AtomicBool::new(false), |_| {})
}
fn jpeg_with_metadata(dir: &Path, name: &str, orientation: u8) -> Source {
    let mut source = write_source(dir, name, &raster(image::ImageFormat::Jpeg));
    let tool = tool();
    let arg = format!("-IFD0:Orientation#={orientation}");
    let icc_path = dir.join("test.icc");
    fs::write(&icc_path, profile()).unwrap();
    let icc = format!("-ICC_Profile<={}", icc_path.to_string_lossy());
    tool.run(
        &[
            "-overwrite_original",
            &arg,
            "-EXIF:Artist=Private author",
            "-EXIF:Software=ComfyUI",
            "-EXIF:UserComment=private prompt and workflow",
            "-GPSLatitude=37.8",
            "-GPSLongitude=144.9",
            "-XMP-dc:Description=private generation parameters",
            "-IPTC:Keywords=private model",
            &icc,
        ],
        Some(&source.path),
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut bytes = fs::read(&source.path).unwrap();
    bytes.extend_from_slice(b"private trailer");
    fs::write(&source.path, &bytes).unwrap();
    source.hash = Sha256::digest(bytes).into();
    source
}

#[test]
fn comfy_png_exact_payload_colour_unknown_chunks_duplicates_and_trailer() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let bytes = comfy_png();
    let source = write_source(dir.path(), "Comfy 花 & [workflow].png", &bytes);
    let tool = tool();
    let before = tool.inspect(&source.path, &AtomicBool::new(false)).unwrap();
    assert!(
        before.iter().filter(|f| f.key.ends_with(":Prompt")).count() >= 2,
        "Duplicate prompts must be visible: {before:?}"
    );
    let result = export(&tool, &source, dest.path()).unwrap();
    assert_eq!(result.status, "verified");
    assert!(result
        .removed
        .iter()
        .any(|v| v.key.to_lowercase().contains("workflow")));
    assert!(result
        .removed
        .iter()
        .any(|v| v.key.to_lowercase().contains("prompt")));
    assert!(result.after.iter().all(|f| exiftool::permitted(&f.key)));
    assert!(result.after.iter().any(|f| f.key.starts_with("ICC")));
    let clean = fs::read(result.output.unwrap()).unwrap();
    assert!(!clean.windows(7).any(|v| v == b"PRIVATE"));
    assert_eq!(
        container::parse(&bytes).unwrap().fingerprint,
        container::parse(&clean).unwrap().fingerprint
    );
    assert_eq!(fs::read(&source.path).unwrap(), bytes);
    assert!(fs::read_dir(dest.path()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".fauxmetadata-")));
}

#[test]
fn jpeg_all_eight_orientations_icc_gps_xmp_iptc_and_source_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let tool = tool();
    for orientation in 1..=8 {
        let source =
            jpeg_with_metadata(dir.path(), &format!("photo-{orientation}.jpg"), orientation);
        let original = fs::read(&source.path).unwrap();
        let result = export(&tool, &source, dest.path()).unwrap();
        assert!(result
            .after
            .iter()
            .any(|f| f.key.ends_with(":Orientation") && f.value == orientation.to_string()));
        assert!(result.after.iter().any(|f| f.key.starts_with("ICC")));
        assert!(result.removed.iter().any(|f| f.key.contains("GPS")));
        assert!(result.removed.iter().any(|f| f.key.starts_with("XMP")));
        assert!(result.removed.iter().any(|f| f.key.starts_with("IPTC")));
        assert_eq!(fs::read(&source.path).unwrap(), original);
        let output = fs::read(result.output.unwrap()).unwrap();
        assert_eq!(
            container::parse(&original).unwrap().fingerprint,
            container::parse(&output).unwrap().fingerprint
        );
    }
}

#[test]
fn collision_safe_names_same_folder_unicode_and_command_like_filename() {
    let dir = tempfile::tempdir().unwrap();
    let tool = tool();
    let source = write_source(
        dir.path(),
        "-all= 花 (draft).png",
        &raster(image::ImageFormat::Png),
    );
    let first = export(&tool, &source, dir.path()).unwrap().output.unwrap();
    let content = fs::read(&first).unwrap();
    let second = export(&tool, &source, dir.path()).unwrap().output.unwrap();
    assert_ne!(first, second);
    assert!(second.ends_with("_clean (1).png"));
    assert_eq!(fs::read(first).unwrap(), content);
    assert_eq!(
        fs::read(&source.path).unwrap(),
        raster(image::ImageFormat::Png)
    );
}

#[test]
fn cancellation_before_and_during_strip_leaves_no_output() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let source = write_source(dir.path(), "cancel.png", &comfy_png());
    let original = fs::read(&source.path).unwrap();
    let cancel = AtomicBool::new(true);
    assert!(
        engine::export_one(&tool(), &source, "1", dest.path(), &cancel, |_| {})
            .unwrap_err()
            .contains("Cancelled")
    );
    cancel.store(false, Ordering::SeqCst);
    assert!(
        engine::export_one(&tool(), &source, "1", dest.path(), &cancel, |phase| {
            if phase == "stripping" {
                cancel.store(true, Ordering::SeqCst);
            }
        })
        .is_err()
    );
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
    assert_eq!(fs::read(&source.path).unwrap(), original);
}

#[test]
fn cancelled_batch_reports_every_unfinished_image_and_retains_completed_copy() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let engine = Engine::new(tool().path);
    for index in 0..3 {
        fs::write(
            dir.path().join(format!("{index}.png")),
            raster(image::ImageFormat::Png),
        )
        .unwrap();
    }
    let imported = engine
        .import(vec![dir.path().into()], |_, _, _| {})
        .unwrap();
    assert_eq!(imported.images.len(), 3);
    let ids: Vec<_> = imported.images.into_iter().map(|v| v.id).collect();
    let results = engine
        .export(&ids, dest.path(), |_, phase, _, result| {
            if phase == "complete" && result.is_some() {
                engine.cancel.store(true, Ordering::SeqCst);
            }
        })
        .unwrap();
    assert_eq!(results[0].status, "verified");
    assert_eq!(results[1].status, "cancelled");
    assert_eq!(results[2].status, "cancelled");
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 1);
}

#[test]
fn source_changed_missing_destination_and_publish_failure_are_safe() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let tool = tool();
    let source = write_source(dir.path(), "change.png", &raster(image::ImageFormat::Png));
    fs::write(&source.path, comfy_png()).unwrap();
    assert!(export(&tool, &source, dest.path())
        .unwrap_err()
        .contains("changed"));
    let source = write_source(dir.path(), "safe.png", &raster(image::ImageFormat::Png));
    assert!(export(&tool, &source, &dest.path().join("does-not-exist")).is_err());
    let temporary = tempfile::NamedTempFile::new_in(dest.path()).unwrap();
    let temporary_path = temporary.path().to_owned();
    let fake_directory = dest.path().join("not-a-directory");
    fs::write(&fake_directory, b"sentinel").unwrap();
    assert!(engine::publish(
        temporary.into_temp_path(),
        &fake_directory,
        "name",
        "png",
        &AtomicBool::new(false)
    )
    .is_err());
    assert!(!temporary_path.exists());
    assert_eq!(fs::read(fake_directory).unwrap(), b"sentinel");
    assert_eq!(
        fs::read(&source.path).unwrap(),
        raster(image::ImageFormat::Png)
    );
}

#[test]
fn rejects_malformed_crc_truncation_unsupported_animation_and_wrong_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(tool().path);
    let mut damaged = raster(image::ImageFormat::Png);
    damaged[29] ^= 1;
    fs::write(dir.path().join("damaged.png"), damaged).unwrap();
    fs::write(dir.path().join("fake.jpg"), b"not a JPEG").unwrap();
    fs::write(
        dir.path().join("truncated.jpg"),
        [0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff],
    )
    .unwrap();
    let mut animated = raster(image::ImageFormat::Png);
    add_png(&mut animated, b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
    fs::write(dir.path().join("animated.png"), animated).unwrap();
    fs::write(dir.path().join("ignored.txt"), b"private").unwrap();
    let imported = engine
        .import(vec![dir.path().into()], |_, _, _| {})
        .unwrap();
    assert!(imported.images.is_empty());
    assert_eq!(imported.issues.len(), 4);
}

#[test]
fn png_rgba_and_sixteen_bit_pixels_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let tool = tool();
    let variants = [
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(9, 7, |x, y| {
            image::Rgba([x as u8, y as u8, 240, ((x + y) * 16) as u8])
        })),
        DynamicImage::ImageRgb16(ImageBuffer::from_fn(9, 7, |x, y| {
            Rgb([(x * 5000) as u16, (y * 7000) as u16, 65535])
        })),
    ];
    for (index, variant) in variants.into_iter().enumerate() {
        let mut buffer = Cursor::new(vec![]);
        variant
            .write_to(&mut buffer, image::ImageFormat::Png)
            .unwrap();
        let source = write_source(
            dir.path(),
            &format!("variant-{index}.png"),
            &buffer.into_inner(),
        );
        let result = export(&tool, &source, dest.path()).unwrap();
        assert!(result.encoded_verified && result.pixels_verified);
    }
}

#[test]
fn exiftool_missing_binary_never_publishes() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let source = write_source(dir.path(), "safe.png", &comfy_png());
    let fake = ExifTool {
        path: dir.path().join("missing.exe"),
    };
    assert!(export(&fake, &source, dest.path())
        .unwrap_err()
        .contains("Cannot start"));
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
}

#[test]
fn rejects_compressed_metadata_bombs_and_invalid_compression() {
    let mut bomb = raster(image::ImageFormat::Png);
    let mut payload = b"workflow\0\0".to_vec();
    payload.extend(compressed(&vec![b'x'; container::MAX_METADATA + 1]));
    add_png(&mut bomb, b"zTXt", &payload);
    assert!(container::parse(&bomb)
        .unwrap_err()
        .contains("expansion limit"));
    let mut invalid = raster(image::ImageFormat::Png);
    add_png(&mut invalid, b"iTXt", b"prompt\0\x02\0\0\0invalid");
    assert!(container::parse(&invalid)
        .unwrap_err()
        .contains("compression flag"));
}

#[test]
fn png_exif_is_removed_and_rotated_png_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let tool = tool();
    for orientation in [1, 6] {
        let mut source = write_source(
            dir.path(),
            &format!("exif-{orientation}.png"),
            &raster(image::ImageFormat::Png),
        );
        let arg = format!("-IFD0:Orientation#={orientation}");
        tool.run(
            &["-overwrite_original", &arg, "-EXIF:Artist=private"],
            Some(&source.path),
            &AtomicBool::new(false),
        )
        .unwrap();
        source.hash = Sha256::digest(fs::read(&source.path).unwrap()).into();
        let result = export(&tool, &source, dest.path());
        if orientation == 1 {
            assert!(result
                .unwrap()
                .after
                .iter()
                .all(|f| f.key.starts_with("PNG")));
        } else {
            assert!(result.unwrap_err().contains("EXIF rotation"));
        }
    }
}

#[test]
fn progressive_and_cmyk_jpeg_carriers_preserve_encoded_data_and_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let tool = tool();
    for (name, data) in [
        (
            "progressive.jpg",
            include_bytes!("fixtures/progressive.jpg").as_slice(),
        ),
        ("cmyk.jpg", include_bytes!("fixtures/cmyk.jpg").as_slice()),
    ] {
        let source = write_source(dir.path(), name, data);
        let result = export(&tool, &source, dest.path()).unwrap();
        assert!(result.encoded_verified && result.pixels_verified);
    }
}

#[test]
fn concurrent_exports_to_one_filename_never_clobber_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let source = write_source(dir.path(), "shared 花.png", &comfy_png());
    let original = fs::read(&source.path).unwrap();
    let tool = tool();
    let outputs = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    export(&tool, &source, destination.path())
                        .unwrap()
                        .output
                        .unwrap()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<std::collections::HashSet<_>>()
    });
    assert_eq!(outputs.len(), 4);
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 4);
    for output in outputs {
        assert_eq!(
            container::parse(&fs::read(output).unwrap())
                .unwrap()
                .fingerprint,
            container::parse(&original).unwrap().fingerprint
        );
    }
    assert_eq!(fs::read(source.path).unwrap(), original);
}

#[test]
fn cancellation_kills_an_inflight_metadata_process_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_source(dir.path(), "processing.png", &comfy_png());
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| tool().inspect(&source.path, &cancel));
        std::thread::sleep(std::time::Duration::from_millis(5));
        cancel.store(true, Ordering::SeqCst);
        assert!(worker.join().unwrap().unwrap_err().contains("Cancelled"));
    });
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
fn read_only_original_can_be_exported_and_remains_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let source = write_source(dir.path(), "readonly.png", &raster(image::ImageFormat::Png));
    let original_permissions = fs::metadata(&source.path).unwrap().permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&source.path, read_only).unwrap();
    let result = export(&tool(), &source, dest.path());
    assert!(fs::metadata(&source.path).unwrap().permissions().readonly());
    fs::set_permissions(&source.path, original_permissions).unwrap();
    assert_eq!(result.unwrap().status, "verified");
}

#[cfg(windows)]
#[test]
fn windows_sharing_violation_cannot_damage_original_or_publish_output() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let original = raster(image::ImageFormat::Png);
    let source = write_source(dir.path(), "locked.png", &original);
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&source.path)
        .unwrap();
    assert!(export(&tool(), &source, dest.path())
        .unwrap_err()
        .contains("Cannot read"));
    drop(lock);
    assert_eq!(fs::read(&source.path).unwrap(), original);
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
}

fn riff_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut raw = kind.to_vec();
    raw.extend((payload.len() as u32).to_le_bytes());
    raw.extend(payload);
    if !payload.len().is_multiple_of(2) {
        raw.push(0);
    }
    raw
}
fn append_webp(data: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    data.extend(riff_chunk(kind, payload));
    let len = (data.len() - 8) as u32;
    data[4..8].copy_from_slice(&len.to_le_bytes());
}
fn webp_carriers() -> Vec<Vec<u8>> {
    let mut alpha = Cursor::new(vec![]);
    DynamicImage::ImageRgba8(ImageBuffer::from_fn(18, 12, |x, y| {
        image::Rgba([(x * 11) as u8, (y * 17) as u8, 203, ((x + y) * 8) as u8])
    }))
    .write_to(&mut alpha, image::ImageFormat::WebP)
    .unwrap();
    vec![
        raster(image::ImageFormat::WebP),
        alpha.into_inner(),
        include_bytes!("fixtures/lossy.webp").to_vec(),
        include_bytes!("fixtures/lossy-alpha.webp").to_vec(),
    ]
}

#[test]
fn webp_lossy_lossless_alpha_icc_and_generation_metadata_are_verified() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let tool = tool();
    let icc_path = root.path().join("colour.icc");
    fs::write(&icc_path, profile()).unwrap();
    let icc = format!("-ICC_Profile<={}", icc_path.to_string_lossy());
    for (index, bytes) in webp_carriers().iter().enumerate() {
        let source = write_source(root.path(), &format!("Comfy 花-{index}.webp"), bytes);
        tool.run(
            &[
                "-overwrite_original",
                "-IFD0:Orientation#=1",
                "-EXIF:Artist=PRIVATE_WEBP_MARKER",
                "-EXIF:UserComment={\"1\":{\"class_type\":\"KSampler\",\"inputs\":{\"seed\":42}}}",
                "-GPSLatitude=37.8",
                "-GPSLongitude=144.9",
                "-XMP-dc:Description=PRIVATE_WEBP_MARKER",
                &icc,
            ],
            Some(&source.path),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut original = fs::read(&source.path).unwrap();
        append_webp(&mut original, b"prIv", b"PRIVATE_WEBP_MARKER");
        fs::write(&source.path, &original).unwrap();
        let source = Source {
            hash: Sha256::digest(&original).into(),
            ..source
        };
        let result = export(&tool, &source, dest.path()).unwrap();
        assert_eq!(result.status, "verified");
        assert!(result.encoded_verified && result.pixels_verified);
        assert!(result.removed.iter().any(|f| f.category == "Generation"));
        assert!(result.removed.iter().any(|f| f.category == "Location"));
        assert!(result.removed.iter().any(|f| f.key.starts_with("XMP")));
        assert!(result.after.iter().any(|f| f.key.starts_with("ICC")));
        assert!(result
            .after
            .iter()
            .all(|f| f.key.starts_with("ICC") || f.key.starts_with("RIFF:")));
        assert!(result.removed_chunks.contains(&"EXIF".into()));
        assert!(result.removed_chunks.contains(&"XMP ".into()));
        assert!(result.removed_chunks.contains(&"prIv".into()));
        let clean = fs::read(result.output.unwrap()).unwrap();
        assert!(!clean.windows(19).any(|v| v == b"PRIVATE_WEBP_MARKER"));
        let parsed = container::parse(&original).unwrap();
        let after = container::parse(&clean).unwrap();
        assert_eq!(parsed.fingerprint, after.fingerprint);
        assert!(after.residual_regions.is_empty());
        assert_eq!(fs::read(&source.path).unwrap(), original);
        assert_eq!(
            engine::decode(&original).unwrap().as_bytes(),
            engine::decode(&clean).unwrap().as_bytes()
        );
    }
}

#[test]
fn webp_import_batch_collision_and_cancel_preserve_sources() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let carriers = webp_carriers();
    for (i, data) in carriers.iter().enumerate() {
        fs::write(root.path().join(format!("-all= 花-{i}.WEBP")), data).unwrap();
    }
    let engine = Engine::new(tool().path);
    let imported = engine
        .import(vec![root.path().into()], |_, _, _| {})
        .unwrap();
    assert!(
        imported.issues.is_empty(),
        "{} import issues",
        imported.issues.len()
    );
    assert_eq!(imported.images.len(), carriers.len());
    assert!(imported
        .images
        .iter()
        .all(|v| v.format == "WebP" && v.preview.starts_with("data:image/png;base64,")));
    let ids: Vec<_> = imported.images.iter().map(|v| v.id.clone()).collect();
    let first = engine.export(&ids, dest.path(), |_, _, _, _| {}).unwrap();
    assert!(first.iter().all(|v| v.status == "verified"), "{first:?}");
    let second = engine.export(&ids, dest.path(), |_, _, _, _| {}).unwrap();
    assert!(second
        .iter()
        .all(|v| v.output.as_ref().unwrap().ends_with("_clean (1).webp")));
    for result in &first {
        assert!(Path::new(result.output.as_ref().unwrap()).is_file());
    }
    let cancelled = engine
        .export(&ids, dest.path(), |_, phase, _, _| {
            if phase == "verifying" {
                engine.cancel.store(true, Ordering::SeqCst);
            }
        })
        .unwrap();
    assert!(cancelled.iter().all(|v| v.status == "cancelled"));
    assert_eq!(
        fs::read_dir(dest.path()).unwrap().count(),
        carriers.len() * 2
    );
    for (i, data) in carriers.iter().enumerate() {
        assert_eq!(
            fs::read(root.path().join(format!("-all= 花-{i}.WEBP"))).unwrap(),
            *data
        );
    }
}

#[test]
fn webp_rotation_and_ambiguous_colour_fail_without_publication() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let tool = tool();
    for args in [
        vec!["-IFD0:Orientation#=6"],
        vec!["-XMP-tiff:Orientation#=8"],
        vec!["-ExifIFD:ColorSpace#=65535"],
    ] {
        let source = write_source(root.path(), "rotated.webp", &webp_carriers()[0]);
        let mut options = vec!["-overwrite_original"];
        options.extend(args);
        tool.run(&options, Some(&source.path), &AtomicBool::new(false))
            .unwrap();
        let original = fs::read(&source.path).unwrap();
        let source = Source {
            hash: Sha256::digest(&original).into(),
            ..source
        };
        let err = export(&tool, &source, dest.path()).unwrap_err();
        assert!(
            err.contains("rotation") || err.contains("colour interpretation"),
            "{err}"
        );
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
        assert_eq!(fs::read(&source.path).unwrap(), original);
        let imported = Engine::new(tool.path.clone())
            .import(vec![source.path], |_, _, _| {})
            .unwrap();
        assert!(imported.images.is_empty());
        assert_eq!(imported.issues.len(), 1);
    }
}

#[test]
fn webp_source_change_and_filesystem_failures_never_publish() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let bytes = webp_carriers()[3].clone();
    let source = write_source(root.path(), "safe.webp", &bytes);
    assert!(export(&tool(), &source, &dest.path().join("missing"))
        .unwrap_err()
        .contains("staging"));
    let invalid = dest.path().join("file");
    fs::write(&invalid, b"sentinel").unwrap();
    assert!(export(&tool(), &source, &invalid).is_err());
    assert_eq!(fs::read(&invalid).unwrap(), b"sentinel");
    let mut changed = bytes.clone();
    append_webp(&mut changed, b"prIv", b"changed");
    fs::write(&source.path, &changed).unwrap();
    assert!(export(&tool(), &source, dest.path())
        .unwrap_err()
        .contains("changed after inspection"));
    assert_eq!(fs::read(&source.path).unwrap(), changed);
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 1);
}

fn queued_engine(folder: &Path, count: usize) -> Engine {
    let engine = Engine::new(tool().path);
    let bytes = raster(image::ImageFormat::Png);
    for index in 0..count {
        let mut source = write_source(folder, &format!("queued-{index}.png"), &bytes);
        source.path = fs::canonicalize(&source.path).unwrap();
        engine
            .sources
            .lock()
            .unwrap()
            .insert(format!("queued-{index}"), source);
    }
    engine
}

#[test]
fn reimporting_queued_folder_does_not_consume_capacity_for_new_files() {
    let root = tempfile::tempdir().unwrap();
    let queued = root.path().join("A");
    let fresh = root.path().join("B");
    fs::create_dir_all(&queued).unwrap();
    fs::create_dir_all(&fresh).unwrap();
    let engine = queued_engine(&queued, 300);
    let bytes = raster(image::ImageFormat::Png);
    fs::write(fresh.join("new-1.png"), &bytes).unwrap();
    fs::write(fresh.join("new-2.png"), &bytes).unwrap();
    let result = engine
        .import(vec![queued, fresh.clone(), fresh], |_, _, _| {})
        .unwrap();
    assert_eq!(result.images.len(), 2);
    assert!(result.images.iter().all(|v| v.name.starts_with("new-")));
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(engine.sources.lock().unwrap().len(), 302);
}

#[test]
fn nested_capacity_stop_has_one_notice_and_full_queue_can_ignore_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let queued = root.path().join("queued");
    let fresh = root.path().join("new/a/b/c");
    fs::create_dir_all(&queued).unwrap();
    fs::create_dir_all(&fresh).unwrap();
    let engine = queued_engine(&queued, 498);
    let bytes = raster(image::ImageFormat::Png);
    for index in 0..4 {
        fs::write(fresh.join(format!("new-{index}.png")), &bytes).unwrap();
    }
    let result = engine
        .import(
            vec![root.path().join("new"), root.path().join("new")],
            |_, _, _| {},
        )
        .unwrap();
    assert_eq!(result.images.len(), 2);
    assert_eq!(result.issues.len(), 1, "{:?}", result.issues);
    assert!(result.issues[0].message.contains("Queue limit"));
    assert!(result.issues[0].path.ends_with(".png"));
    assert_eq!(engine.sources.lock().unwrap().len(), 500);
    let duplicates = engine.import(vec![queued], |_, _, _| {}).unwrap();
    assert!(duplicates.images.is_empty());
    assert!(duplicates.issues.is_empty());
}
