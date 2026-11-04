use crate::{
    container::{self, Format},
    exiftool::{self, ExifTool},
    model::*,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::ImageReader;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};

const MAX_QUEUE: usize = 500;
const MAX_SCAN_PATHS: usize = 10_000;
const MAX_FOLDER_ENTRIES: usize = 10_000;

#[derive(Clone)]
pub struct Source {
    pub path: PathBuf,
    pub hash: [u8; 32],
    pub presentation_bytes: usize,
}

pub struct Engine {
    pub tool: ExifTool,
    pub sources: Mutex<HashMap<String, Source>>,
    pub busy: AtomicBool,
    pub cancel: AtomicBool,
    next_id: AtomicU64,
}

pub struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Engine {
    pub fn new(tool: PathBuf) -> Self {
        Self {
            tool: ExifTool { path: tool },
            sources: Mutex::new(HashMap::new()),
            busy: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn begin(&self) -> AppResult<BusyGuard<'_>> {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Another operation is in progress.")?;
        self.cancel.store(false, Ordering::SeqCst);
        Ok(BusyGuard(&self.busy))
    }

    pub fn import(
        &self,
        paths: Vec<PathBuf>,
        mut progress: impl FnMut(usize, usize, &str),
    ) -> AppResult<ImportResult> {
        let _guard = self.begin()?;
        self.tool.ensure_version(&self.cancel)?;
        let mut images = vec![];
        let mut issues = vec![];
        let mut files = vec![];
        let existing: HashSet<_> = self
            .sources
            .lock()
            .map_err(|_| "Queue unavailable.")?
            .values()
            .map(|v| v.path.clone())
            .collect();
        // Queued sources must be skipped before they can consume collection capacity.
        let mut seen = existing.clone();
        let capacity = MAX_QUEUE.saturating_sub(existing.len());
        let mut presentation_bytes: usize = self
            .sources
            .lock()
            .map_err(|_| "Queue unavailable.")?
            .values()
            .map(|v| v.presentation_bytes)
            .sum();
        for path in paths {
            if collect::<MAX_FOLDER_ENTRIES>(
                &path,
                0,
                capacity,
                &mut files,
                &mut seen,
                &mut issues,
                &self.cancel,
            ) {
                break;
            }
        }
        let total = files.len();
        for (index, path) in files.into_iter().enumerate() {
            if self.cancel.load(Ordering::SeqCst) {
                issues.push(ImportIssue {
                    path: "Import".into(),
                    message: "Import cancelled; already inspected files remain in the queue."
                        .into(),
                });
                break;
            }
            progress(
                index,
                total,
                &path.file_name().unwrap_or_default().to_string_lossy(),
            );
            match self.inspect_source(&path) {
                Ok((image, source)) => {
                    if presentation_bytes.saturating_add(source.presentation_bytes)
                        > 128 * 1024 * 1024
                    {
                        issues.push(ImportIssue { path: path.to_string_lossy().into(), message: "Queue metadata/preview memory budget reached (128 MiB). Remove some images and retry.".into() });
                        break;
                    }
                    presentation_bytes += source.presentation_bytes;
                    self.sources
                        .lock()
                        .map_err(|_| "Queue unavailable.")?
                        .insert(image.id.clone(), source);
                    images.push(image);
                }
                Err(message) => issues.push(ImportIssue {
                    path: path.to_string_lossy().into(),
                    message,
                }),
            }
        }
        Ok(ImportResult { images, issues })
    }

    fn inspect_source(&self, path: &Path) -> AppResult<(Inspection, Source)> {
        let data = read_bounded(path)?;
        let structure = container::parse(&data)?;
        let mut fields = self.tool.inspect(path, &self.cancel)?;
        validate_appearance(&structure.format, &fields)?;
        source_retention(&structure.format, &mut fields);
        let decoded = decode(&data)?;
        let mut thumb = decoded.thumbnail(560, 480);
        if let Some(value) = orientation(&fields) {
            if let Some(value) = image::metadata::Orientation::from_exif(value) {
                thumb.apply_orientation(value);
            }
        }
        let mut preview = Cursor::new(Vec::new());
        thumb
            .write_to(&mut preview, image::ImageFormat::Png)
            .map_err(|e| format!("Cannot create preview: {e}"))?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst).to_string();
        let image = Inspection {
            id,
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            path: path.to_string_lossy().into(),
            format: structure.format.label().into(),
            bytes: data.len() as u64,
            width: structure.width,
            height: structure.height,
            preview: format!(
                "data:image/png;base64,{}",
                STANDARD.encode(preview.into_inner())
            ),
            fields,
            chunks: structure.chunks,
        };
        let presentation_bytes = image.preview.len()
            + image
                .fields
                .iter()
                .map(|f| f.key.len() + f.value.len())
                .sum::<usize>();
        Ok((
            image,
            Source {
                path: path.to_path_buf(),
                hash: Sha256::digest(&data).into(),
                presentation_bytes,
            },
        ))
    }

    pub fn export(
        &self,
        ids: &[String],
        directory: &Path,
        mut progress: impl FnMut(&str, &str, usize, Option<ExportResult>),
    ) -> AppResult<Vec<ExportResult>> {
        self.export_prepared(ids, || Ok(directory.to_path_buf()), &mut progress)
    }

    pub fn export_prepared(
        &self,
        ids: &[String],
        prepare: impl FnOnce() -> AppResult<PathBuf>,
        mut progress: impl FnMut(&str, &str, usize, Option<ExportResult>),
    ) -> AppResult<Vec<ExportResult>> {
        let _guard = self.begin()?;
        if ids.is_empty() || ids.len() > MAX_QUEUE {
            return Err("Select between 1 and 500 images.".into());
        }
        let directory = prepare()?;
        if !directory.is_absolute()
            || !directory.is_dir()
            || is_link(&fs::symlink_metadata(&directory).map_err(|e| e.to_string())?)
        {
            return Err(
                "Export destination must be an existing local directory, not a symbolic link."
                    .into(),
            );
        }
        let mut results = vec![];
        let mut seen = HashSet::new();
        for (index, id) in ids.iter().enumerate() {
            if !seen.insert(id) {
                continue;
            }
            let source = self
                .sources
                .lock()
                .map_err(|_| "Queue unavailable.")?
                .get(id)
                .cloned();
            let result = if self.cancel.load(Ordering::SeqCst) {
                ExportResult::failed(id, "Cancelled before export.".into(), true)
            } else if let Some(source) = source {
                let phase = |phase: &str| progress(id, phase, index, None);
                match export_one(&self.tool, &source, id, &directory, &self.cancel, phase) {
                    Ok(result) => result,
                    Err(e) => ExportResult::failed(id, e, self.cancel.load(Ordering::SeqCst)),
                }
            } else {
                ExportResult::failed(id, "Image is no longer in this session.".into(), false)
            };
            progress(id, "complete", index + 1, Some(result.clone()));
            results.push(result);
        }
        Ok(results)
    }
}

pub fn read_bounded(path: &Path) -> AppResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("Cannot access image: {e}"))?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(
            "Only regular files are accepted; links and reparse points are skipped.".into(),
        );
    }
    if metadata.len() > container::MAX_FILE as u64 {
        return Err("Image exceeds the 128 MiB file limit.".into());
    }
    let mut data = vec![];
    File::open(path)
        .map_err(|e| format!("Cannot read image: {e}"))?
        .take(container::MAX_FILE as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > container::MAX_FILE {
        return Err("Image exceeded the file-size limit while reading.".into());
    }
    Ok(data)
}

pub fn decode(data: &[u8]) -> AppResult<image::DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| format!("Image decoding validation failed: {e}"))
}

fn orientation(fields: &[Field]) -> Option<u8> {
    fields
        .iter()
        .find(|f| f.key == "IFD0:Orientation")
        .and_then(|f| f.value.parse().ok())
}

fn validate_appearance(format: &Format, fields: &[Field]) -> AppResult<()> {
    let mut observed_orientation = None;
    for field in fields
        .iter()
        .filter(|f| f.key.split(':').next() == Some("IFD0") && f.key.ends_with(":Orientation"))
    {
        let value = field
            .value
            .parse::<u8>()
            .map_err(|_| "Invalid EXIF orientation.")?;
        if !(1..=8).contains(&value) {
            return Err("Invalid EXIF orientation.".into());
        }
        if observed_orientation.is_some_and(|old| old != value) {
            return Err("Conflicting EXIF orientation copies are not supported.".into());
        }
        observed_orientation = Some(value);
    }
    if observed_orientation.is_some() && orientation(fields).is_none() {
        return Err("Ambiguous EXIF orientation copies are not supported.".into());
    }
    if *format == Format::Webp {
        // Viewers disagree on WebP EXIF/XMP transforms. Reject ambiguous appearance.
        for field in fields {
            let tag = field.key.rsplit(':').next().unwrap_or("");
            if tag == "Orientation" && field.value != "1" {
                return Err("WebP with EXIF/XMP rotation is not supported; removing it could change appearance.".into());
            }
            if matches!(tag, "Rotation" | "RollAngle") && field.value != "0" {
                return Err("WebP with rotation metadata is not supported.".into());
            }
            if tag == "ColorSpace"
                && field.value != "1"
                && !fields.iter().any(|v| v.key.starts_with("ICC"))
            {
                return Err("WebP with non-sRGB metadata colour interpretation requires an ICC profile; this combination is not supported.".into());
            }
        }
    }
    if let Some(value) = orientation(fields) {
        if !(1..=8).contains(&value) {
            return Err("Invalid EXIF orientation.".into());
        }
        if *format == Format::Png && value != 1 {
            return Err(
                "PNG with EXIF rotation is not supported; removing it could change appearance."
                    .into(),
            );
        }
    }
    if *format == Format::Png
        && !fields.iter().any(|f| f.key.starts_with("ICC"))
        && fields
            .iter()
            .any(|f| f.key == "ExifIFD:ColorSpace" && f.value != "1")
    {
        return Err("PNG with non-sRGB EXIF colour interpretation requires an ICC profile; this combination is not supported.".into());
    }
    Ok(())
}

fn source_retention(format: &Format, fields: &mut [Field]) {
    // PNG orientation 1 is implicit. Other PNG EXIF fields are removed; the PNG rendering chunks are authoritative.
    if *format == Format::Png {
        for field in fields.iter_mut() {
            field.retained &= field.key.starts_with("PNG:")
                || field.key.starts_with("PNG-")
                || field.key.starts_with("ICC");
        }
    }
    if *format == Format::Webp {
        for field in fields.iter_mut() {
            field.retained &= field.key.starts_with("ICC")
                || (field.key.starts_with("RIFF:") && !field.key.ends_with(":WebP_Flags"));
        }
    }
}

pub fn export_one(
    tool: &ExifTool,
    source: &Source,
    id: &str,
    directory: &Path,
    cancel: &AtomicBool,
    mut phase: impl FnMut(&str),
) -> AppResult<ExportResult> {
    let checkpoint = || {
        if cancel.load(Ordering::SeqCst) {
            Err("Cancelled.".to_string())
        } else {
            Ok(())
        }
    };
    checkpoint()?;
    phase("staging");
    tool.ensure_version(cancel)?;
    let data = read_bounded(&source.path)?;
    if <[u8; 32]>::from(Sha256::digest(&data)) != source.hash {
        return Err(
            "Original changed after inspection. Remove and import it again before exporting."
                .into(),
        );
    }
    let before_structure = container::parse(&data)?;
    // Same-volume private staging enables an atomic no-replace publish on Windows.
    let staging = tempfile::Builder::new()
        .prefix(".fauxmetadata-")
        .tempdir_in(directory)
        .map_err(|e| format!("Cannot create staging directory: {e}"))?;
    let mut working = tempfile::Builder::new()
        .prefix("image-")
        .suffix(&format!(".{}", before_structure.format.extension()))
        .tempfile_in(staging.path())
        .map_err(|e| e.to_string())?;
    working
        .write_all(&data)
        .map_err(|e| format!("Cannot stage image: {e}"))?;
    working.flush().map_err(|e| e.to_string())?;
    // Close the Windows handle before ExifTool replaces the staged pathname.
    let working = working.into_temp_path();
    let mut before = tool
        .inspect(&working, cancel)
        .map_err(|e| format!("Staging inspection: {e}"))?;
    validate_appearance(&before_structure.format, &before)?;
    source_retention(&before_structure.format, &mut before);
    checkpoint()?;
    phase("stripping");
    tool.strip(&working, &before, cancel)
        .map_err(|e| format!("Metadata stripping: {e}"))?;
    if let Some(filtered) = &before_structure.sanitized_container {
        // Container allowlists preserve exact PNG/WebP rendering chunks that ExifTool may delete.
        // It also removes private/unknown text chunks and trailers that tag inventories cannot enumerate.
        let mut file = File::options()
            .write(true)
            .truncate(true)
            .open(&working)
            .map_err(|e| e.to_string())?;
        file.write_all(filtered)
            .map_err(|e| format!("Cannot finalise image container: {e}"))?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    checkpoint()?;
    phase("verifying");
    let output_data = read_bounded(&working)?;
    let after_structure = container::parse(&output_data)?;
    if before_structure.format != after_structure.format
        || before_structure.fingerprint != after_structure.fingerprint
    {
        return Err(
            "Verification blocked export: encoded image or colour rendering data changed.".into(),
        );
    }
    let bad_regions: Vec<_> = after_structure
        .residual_regions
        .iter()
        .filter(|region| !(after_structure.format == Format::Jpeg && region.as_str() == "EXIF"))
        .collect();
    if !bad_regions.is_empty() {
        return Err(format!(
            "Verification blocked export: unapproved container regions: {bad_regions:?}"
        ));
    }
    let pixel_signature = |bytes: &[u8]| -> AppResult<_> {
        let pixels = decode(bytes)?;
        Ok((
            pixels.color(),
            <[u8; 32]>::from(Sha256::digest(pixels.as_bytes())),
        ))
    };
    if pixel_signature(&data)? != pixel_signature(&output_data)? {
        return Err("Verification blocked export: decoded pixels changed.".into());
    }
    let after = tool
        .inspect(&working, cancel)
        .map_err(|e| format!("Output reinspection: {e}"))?;
    let (removed, retained) = exiftool::diff(&before, &after)?;
    let removed_chunks = before_structure
        .chunks
        .into_iter()
        .filter(|c| !after_structure.chunks.contains(c))
        .collect();
    File::options()
        .read(true)
        .write(true)
        .open(&working)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("Cannot sync output: {e}"))?;
    checkpoint()?;
    phase("publishing");
    let name = source
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let output = publish(
        working,
        directory,
        &name,
        before_structure.format.extension(),
        cancel,
    )?;
    Ok(ExportResult {
        id: id.into(),
        status: "verified".into(),
        output: Some(output.to_string_lossy().into()),
        error: None,
        removed,
        retained,
        after,
        removed_chunks,
        bytes: output_data.len() as u64,
        encoded_verified: true,
        pixels_verified: true,
    })
}

pub fn publish(
    mut temporary: tempfile::TempPath,
    directory: &Path,
    stem: &str,
    extension: &str,
    cancel: &AtomicBool,
) -> AppResult<PathBuf> {
    // Conservative component length keeps collision suffixes within Windows limits (including UTF-16).
    let stem: String = stem
        .chars()
        .scan(0usize, |units, ch| {
            *units += ch.len_utf16();
            (*units <= 100).then_some(ch)
        })
        .collect();
    let stem = stem.trim_end_matches(['.', ' ']);
    for suffix in 0..10000 {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled.".into());
        }
        let filename = if suffix == 0 {
            format!("{stem}_clean.{extension}")
        } else {
            format!("{stem}_clean ({suffix}).{extension}")
        };
        let destination = directory.join(filename);
        match temporary.persist_noclobber(&destination) {
            Ok(()) => return Ok(destination),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => temporary = e.path,
            Err(e) => {
                return Err(format!(
                    "Cannot publish sanitised copy (original untouched): {}",
                    e.error
                ))
            }
        }
    }
    Err("No available output filename after 10,000 collisions.".into())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

// A smaller entry limit lets regressions exercise real folders without 10,000 files.
fn collect<const ENTRY_LIMIT: usize>(
    path: &Path,
    depth: usize,
    capacity: usize,
    files: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    issues: &mut Vec<ImportIssue>,
    cancel: &AtomicBool,
) -> bool {
    if cancel.load(Ordering::SeqCst) || issues.len() >= 500 {
        return true;
    }
    let mut stopped = false;
    let mut visit = || -> AppResult<()> {
        if depth > 16 {
            return Err("Folder nesting exceeds 16 levels.".into());
        }
        if seen.len() >= MAX_SCAN_PATHS {
            stopped = true;
            return Err("Folder scan reached the 10,000-path traversal limit.".into());
        }
        let metadata = fs::symlink_metadata(path).map_err(|e| format!("Cannot read path: {e}"))?;
        if is_link(&metadata) {
            return Err("Symbolic links and reparse points are skipped.".into());
        }
        let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !seen.insert(path.clone()) {
            return Ok(());
        }
        if metadata.is_dir() {
            for (count, entry) in fs::read_dir(&path).map_err(|e| e.to_string())?.enumerate() {
                if count >= ENTRY_LIMIT {
                    // This folder's limit must not stop unrelated dropped paths.
                    return Err(format!(
                        "Folder scan stopped at its {ENTRY_LIMIT}-entry limit."
                    ));
                }
                let entry = entry.map_err(|e| e.to_string())?;
                if collect::<ENTRY_LIMIT>(
                    &entry.path(),
                    depth + 1,
                    capacity,
                    files,
                    seen,
                    issues,
                    cancel,
                ) {
                    stopped = true;
                    break;
                }
            }
        } else if metadata.is_file() {
            let extension = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            if !["png", "jpg", "jpeg", "webp"].contains(&extension.as_str()) {
                if depth == 0 {
                    return Err("Unsupported file type. Use PNG, JPEG or still WebP.".into());
                }
                return Ok(());
            }
            if files.len() >= capacity {
                stopped = true;
                return Err("Queue limit reached (500 images).".into());
            }
            files.push(path);
        } else {
            return Err("Not a regular image file or directory.".into());
        }
        Ok(())
    };
    if let Err(message) = visit() {
        issues.push(ImportIssue {
            path: path.to_string_lossy().into(),
            message,
        });
    }
    stopped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::PreferenceStore;

    #[test]
    fn folder_entry_limit_preserves_later_paths_and_reports_once() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        let folder = parent.join("large");
        fs::create_dir_all(&folder).unwrap();
        for index in 0..3 {
            fs::write(folder.join(format!("{index}.txt")), b"not an image").unwrap();
        }
        let later = root.path().join("later.png");
        fs::write(&later, b"collection does not decode files").unwrap();
        let mut files = vec![];
        let mut seen = HashSet::new();
        let mut issues = vec![];
        let cancel = AtomicBool::new(false);
        for path in [&parent, &later] {
            if collect::<2>(
                path,
                0,
                MAX_QUEUE,
                &mut files,
                &mut seen,
                &mut issues,
                &cancel,
            ) {
                break;
            }
        }
        assert_eq!(files, vec![fs::canonicalize(later).unwrap()]);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].path,
            fs::canonicalize(folder).unwrap().to_string_lossy()
        );
        assert!(issues[0].message.contains("2-entry limit"));
    }

    #[test]
    fn global_path_limit_stops_remaining_paths_with_one_notice() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("first.txt"), b"not an image").unwrap();
        let later = root.path().join("later.png");
        fs::write(&later, b"not inspected after global stop").unwrap();
        let mut files = vec![];
        let mut seen: HashSet<_> = (0..MAX_SCAN_PATHS - 1)
            .map(|index| root.path().join(format!("previous-{index}")))
            .collect();
        let mut issues = vec![];
        let cancel = AtomicBool::new(false);
        for path in [&folder, &later] {
            if collect::<MAX_FOLDER_ENTRIES>(
                path,
                0,
                MAX_QUEUE,
                &mut files,
                &mut seen,
                &mut issues,
                &cancel,
            ) {
                break;
            }
        }
        assert!(files.is_empty());
        assert_eq!(seen.len(), MAX_SCAN_PATHS);
        assert!(!seen.contains(&fs::canonicalize(later).unwrap()));
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("10,000-path traversal limit"));
    }

    #[test]
    fn busy_and_invalid_exports_do_not_prepare_destination() {
        let root = tempfile::tempdir().unwrap();
        let pictures = root.path().join("Pictures");
        let settings =
            PreferenceStore::load(root.path().join("preferences.json"), Ok(pictures.clone()));
        let engine = Engine::new(root.path().join("unused.exe"));
        let guard = engine.begin().unwrap();
        engine.cancel.store(true, Ordering::SeqCst);
        let error = engine
            .export_prepared(&["1".into()], || settings.prepare_export(), |_, _, _, _| {})
            .unwrap_err();
        assert!(error.contains("Another operation"));
        assert!(!pictures.exists());
        assert!(engine.cancel.load(Ordering::SeqCst));
        drop(guard);
        assert!(engine
            .export_prepared(&[], || settings.prepare_export(), |_, _, _, _| {})
            .is_err());
        assert!(!pictures.exists());
        assert!(engine
            .export_prepared(
                &["1".into()],
                || Err("Unavailable drive".into()),
                |_, _, _, _| {}
            )
            .is_err());
        assert!(
            engine.begin().is_ok(),
            "Failed preparation must release the operation guard"
        );
    }

    #[test]
    fn duplicate_orientation_cannot_override_primary_or_pass_verification() {
        let field = |key: &str, value: &str| Field {
            key: key.into(),
            value: value.into(),
            category: "Appearance".into(),
            retained: exiftool::permitted(key),
        };
        let primary = field("IFD0:Orientation", "6");
        let conflicting = vec![field("IFD0:Copy1:Orientation", "8"), primary.clone()];
        assert_eq!(orientation(&conflicting), Some(6));
        assert!(validate_appearance(&Format::Jpeg, &conflicting)
            .unwrap_err()
            .contains("Conflicting"));
        assert!(exiftool::diff(&conflicting, std::slice::from_ref(&primary)).is_err());
        let repeated = vec![field("IFD0:Copy1:Orientation", "6"), primary.clone()];
        assert!(validate_appearance(&Format::Jpeg, &repeated).is_ok());
        assert!(
            exiftool::diff(&repeated, &[primary]).is_err(),
            "Missing retained copies must still block publication"
        );
        assert!(
            validate_appearance(&Format::Jpeg, &[field("IFD0:Copy1:Orientation", "6")]).is_err()
        );
        assert!(
            validate_appearance(&Format::Jpeg, &[field("IFD0:Orientation", "invalid")]).is_err()
        );
    }
}
