//! Small, versioned preferences store. No source paths or image metadata are persisted.
use crate::model::AppResult;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    version: u8,
    pub custom_destination: Option<PathBuf>,
    pub auto_clean_on_drop: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            custom_destination: None,
            auto_clean_on_drop: false,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceView {
    pub destination: Option<String>,
    pub is_default: bool,
    pub auto_clean_on_drop: bool,
    pub error: Option<String>,
}

pub struct PreferenceStore {
    file: PathBuf,
    default: AppResult<PathBuf>,
    value: Preferences,
    load_error: Option<String>,
}
impl PreferenceStore {
    pub fn load(file: PathBuf, pictures: AppResult<PathBuf>) -> Self {
        let default = pictures.and_then(|path| default_destination(&path));
        let (value, load_error) = match read_preferences(&file) {
            Ok(value) => (value, None),
            Err(error) => (Preferences::default(), Some(format!("Cannot load saved preferences: {error} Choose a folder or Reset to Default to recover."))),
        };
        Self {
            file,
            default,
            value,
            load_error,
        }
    }
    fn active(&self) -> AppResult<PathBuf> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        self.value
            .custom_destination
            .clone()
            .map(Ok)
            .unwrap_or_else(|| self.default.clone())
    }
    pub fn view(&self) -> PreferenceView {
        let path = self.active();
        PreferenceView {
            destination: path.as_ref().ok().map(|p| display_path(p)),
            is_default: self.value.custom_destination.is_none(),
            auto_clean_on_drop: self.value.auto_clean_on_drop,
            error: path.err(),
        }
    }
    fn commit(&mut self, value: Preferences) -> AppResult<PreferenceView> {
        write_preferences(&self.file, &value)?;
        self.value = value;
        self.load_error = None;
        Ok(self.view())
    }
    pub fn choose(&mut self, path: &Path) -> AppResult<PreferenceView> {
        validate_path(path)?;
        validate_directory(path)?;
        let canonical = fs::canonicalize(path)
            .map_err(|e| format!("Cannot access selected output folder: {e}"))?;
        self.commit(Preferences {
            custom_destination: Some(canonical),
            ..self.value.clone()
        })
    }
    pub fn reset(&mut self) -> AppResult<PreferenceView> {
        self.default.as_ref().map_err(Clone::clone)?;
        self.commit(Preferences {
            custom_destination: None,
            ..self.value.clone()
        })
    }
    pub fn auto_clean(&mut self, enabled: bool) -> AppResult<PreferenceView> {
        // Corrupt preferences require explicit destination recovery before automation is enabled.
        self.active()?;
        self.commit(Preferences {
            auto_clean_on_drop: enabled,
            ..self.value.clone()
        })
    }
    pub fn prepare_export(&self) -> AppResult<PathBuf> {
        let path = self.active()?;
        validate_path(&path)?;
        if self.value.custom_destination.is_none() {
            fs::create_dir_all(&path).map_err(|e| format!("Cannot create Cleaned Images folder at {}: {e}. Choose another output folder or check permissions and drive availability.", display_path(&path)))?;
        }
        validate_directory(&path)?;
        // Check write access before starting the batch. The export engine still validates and
        // stages independently, so a later permissions/race failure cannot publish an unsafe copy.
        let probe = tempfile::Builder::new().prefix(".fauxmetadata-access-").tempfile_in(&path)
            .map_err(|e| format!("Cannot write to output folder {}: {e}. Check permissions, free space and drive availability, or choose another folder.", display_path(&path)))?;
        probe
            .close()
            .map_err(|e| format!("Cannot remove output access probe: {e}"))?;
        Ok(path)
    }
}

pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.trim_start_matches("\\\\?\\").into()
    }
}
pub fn default_destination(pictures: &Path) -> AppResult<PathBuf> {
    validate_path(pictures)?;
    Ok(pictures.join("Cleaned Images"))
}
fn validate_path(path: &Path) -> AppResult<()> {
    let text = path
        .to_str()
        .ok_or("Output folder must be valid Unicode.")?;
    if !path.is_absolute()
        || text.len() > 32000
        || text.chars().any(|c| c < ' ')
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(
            "Invalid output folder path. Choose an absolute folder path or Reset to Default."
                .into(),
        );
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        let valid = matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)));
        if !valid
            || text
                .trim_start_matches("\\\\?\\")
                .contains(['*', '?', '"', '<', '>', '|'])
        {
            return Err(
                "Invalid Windows output folder path. Choose a folder or Reset to Default.".into(),
            );
        }
        for component in path.components() {
            if let Component::Normal(part) = component {
                let part = part.to_string_lossy();
                let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
                if part.contains(':')
                    || part.ends_with(['.', ' '])
                    || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || (stem.len() == 4
                        && (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                {
                    return Err(
                        "Invalid Windows output folder name. Choose a folder or Reset to Default."
                            .into(),
                    );
                }
            }
        }
    }
    Ok(())
}
fn validate_directory(path: &Path) -> AppResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("Output folder {} is unavailable: {e}. Check the drive or choose another folder / Reset to Default.", display_path(path)))?;
    #[cfg(windows)]
    let link = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let link = metadata.file_type().is_symlink();
    if !metadata.is_dir() || link {
        return Err(
            "Output destination must be a regular directory, not a file, link or reparse point."
                .into(),
        );
    }
    Ok(())
}
fn read_preferences(file: &Path) -> AppResult<Preferences> {
    match fs::symlink_metadata(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
        Err(e) => return Err(e.to_string()),
        Ok(m) if !m.is_file() || is_reparse(&m) || m.len() > 65536 => {
            return Err("Invalid preferences file (regular JSON, at most 64 KiB required).".into())
        }
        Ok(_) => (),
    }
    let mut data = Vec::new();
    fs::File::open(file)
        .map_err(|e| e.to_string())?
        .take(65537)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > 65536 {
        return Err("Preferences file exceeds 64 KiB.".into());
    }
    let value: Preferences = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if value.version != 1 {
        return Err("Unsupported preferences version.".into());
    }
    if let Some(path) = &value.custom_destination {
        validate_path(path)?;
    }
    Ok(value)
}
fn is_reparse(metadata: &fs::Metadata) -> bool {
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
fn write_preferences(file: &Path, value: &Preferences) -> AppResult<()> {
    let parent = file.parent().ok_or("Preferences directory unavailable.")?;
    fs::create_dir_all(parent).map_err(|e| format!("Cannot create preferences directory: {e}"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".fauxmetadata-settings-")
        .tempfile_in(parent)
        .map_err(|e| format!("Cannot save preferences: {e}"))?;
    let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    temporary
        .write_all(&data)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| format!("Cannot save preferences: {e}"))?;
    temporary
        .persist(file)
        .map_err(|e| format!("Cannot save preferences; previous choices are unchanged: {e}"))?;
    Ok(())
}

#[cfg(windows)]
pub fn pictures_folder() -> AppResult<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr};
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY},
    };
    // Null token means current user; no DEFAULT_PATH flag, so redirection is respected.
    unsafe {
        let mut raw = ptr::null_mut();
        let result = SHGetKnownFolderPath(
            &FOLDERID_Pictures,
            KF_FLAG_DONT_VERIFY as u32,
            ptr::null_mut(),
            &mut raw,
        );
        if result < 0 || raw.is_null() {
            if !raw.is_null() {
                CoTaskMemFree(raw.cast());
            }
            return Err(format!("Windows could not resolve your Pictures known folder (HRESULT {result:#x}). Choose another output folder."));
        }
        let mut len = 0;
        while *raw.add(len) != 0 {
            len += 1;
        }
        let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(raw, len)));
        CoTaskMemFree(raw.cast());
        Ok(path)
    }
}
#[cfg(not(windows))]
pub fn pictures_folder() -> AppResult<PathBuf> {
    Err("Pictures known-folder resolution requires Windows.".into())
}

/// Only called with a folder derived from a verified, backend-recorded export.
#[cfg(windows)]
pub fn open_folder(path: &Path) -> AppResult<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    validate_directory(path)?;
    let visible = PathBuf::from(display_path(path));
    let path: Vec<u16> = visible.as_os_str().encode_wide().chain(Some(0)).collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        Err(format!(
            "Windows could not open the cleaned images folder (error {result})."
        ))
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
pub fn open_folder(_: &Path) -> AppResult<()> {
    Err("Open folder requires Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(root: &Path) -> PreferenceStore {
        PreferenceStore::load(
            root.join("config/preferences.json"),
            Ok(root.join("Redirected Pictures 花")),
        )
    }
    #[test]
    fn redirected_pictures_is_lazy_and_created_at_export_only() {
        let root = tempfile::tempdir().unwrap();
        let state = store(root.path());
        let expected = root.path().join("Redirected Pictures 花/Cleaned Images");
        assert_eq!(state.active().unwrap(), expected);
        assert!(!expected.exists());
        assert!(!state.view().auto_clean_on_drop);
        assert_eq!(state.prepare_export().unwrap(), expected);
        assert!(expected.is_dir());
        assert_eq!(fs::read_dir(expected).unwrap().count(), 0);
    }
    #[test]
    fn custom_and_auto_preferences_survive_launch_and_reset() {
        let root = tempfile::tempdir().unwrap();
        let custom = root.path().join("Custom 花");
        fs::create_dir(&custom).unwrap();
        let mut state = store(root.path());
        state.choose(&custom).unwrap();
        state.auto_clean(true).unwrap();
        let mut restarted = store(root.path());
        assert!(!restarted.view().is_default);
        assert!(restarted.view().auto_clean_on_drop);
        assert_eq!(
            restarted.prepare_export().unwrap(),
            fs::canonicalize(&custom).unwrap()
        );
        restarted.reset().unwrap();
        let restarted = store(root.path());
        assert!(restarted.view().is_default);
        assert!(restarted.view().auto_clean_on_drop);
        assert!(!restarted.active().unwrap().exists());
        let json = fs::read_to_string(root.path().join("config/preferences.json")).unwrap();
        assert!(!json.contains("Custom 花"));
    }
    #[test]
    fn malformed_relative_device_and_future_preferences_block_exports() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("config/preferences.json");
        fs::create_dir(file.parent().unwrap()).unwrap();
        for contents in [
            "{oops".to_string(),
            r#"{"version":2,"customDestination":null,"autoCleanOnDrop":true}"#.into(),
            r#"{"version":1,"customDestination":"relative","autoCleanOnDrop":true}"#.into(),
            "x".repeat(65537),
        ] {
            fs::write(&file, contents).unwrap();
            let mut state = store(root.path());
            assert!(state.view().error.is_some());
            assert!(state.view().destination.is_none());
            assert!(!state.view().auto_clean_on_drop);
            assert!(state.prepare_export().is_err());
            assert!(state.auto_clean(true).is_err());
            state.reset().unwrap();
            assert!(store(root.path()).view().error.is_none());
        }
        #[cfg(windows)]
        for path in [
            r"\\.\NUL",
            r"C:\foo:stream",
            r"C:\bad?path",
            r"C:\foo\..\bar",
            r"C:\NUL",
            r"C:\trailing.",
        ] {
            assert!(validate_path(Path::new(path)).is_err(), "{path}");
        }
    }
    #[test]
    fn missing_custom_directory_is_not_recreated_or_silently_replaced() {
        let root = tempfile::tempdir().unwrap();
        let custom = root.path().join("Removable");
        fs::create_dir(&custom).unwrap();
        let mut state = store(root.path());
        state.choose(&custom).unwrap();
        fs::remove_dir(&custom).unwrap();
        let restarted = store(root.path());
        assert!(!restarted.view().is_default);
        assert!(restarted
            .prepare_export()
            .unwrap_err()
            .contains("unavailable"));
        assert!(!custom.exists());
        assert!(!restarted.default.unwrap().exists());
    }
    #[test]
    fn missing_known_folder_can_recover_via_custom_choice() {
        let root = tempfile::tempdir().unwrap();
        let mut state = PreferenceStore::load(
            root.path().join("config/preferences.json"),
            Err("Known folder unavailable".into()),
        );
        assert!(state.view().error.is_some());
        assert!(state.prepare_export().is_err());
        state.choose(root.path()).unwrap();
        assert!(state.prepare_export().is_ok());
        assert!(state.reset().is_err());
        assert!(!state.view().is_default);
    }
    #[test]
    fn file_at_default_destination_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let state = store(root.path());
        let path = state.active().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"sentinel").unwrap();
        assert!(state
            .prepare_export()
            .unwrap_err()
            .contains("Cannot create"));
        assert_eq!(fs::read(&path).unwrap(), b"sentinel");
    }
    #[test]
    fn failed_settings_commit_keeps_previous_state_and_cleans_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let mut state = store(root.path());
        fs::create_dir_all(&state.file).unwrap();
        assert!(state.auto_clean(true).is_err());
        assert!(!state.view().auto_clean_on_drop);
        assert_eq!(
            fs::read_dir(state.file.parent().unwrap()).unwrap().count(),
            1
        );
    }
    #[cfg(windows)]
    #[test]
    fn locked_settings_file_cannot_replace_saved_preferences() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let mut state = store(root.path());
        state.auto_clean(false).unwrap();
        let before = fs::read(&state.file).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&state.file)
            .unwrap();
        assert!(state.auto_clean(true).is_err());
        assert!(!state.view().auto_clean_on_drop);
        drop(lock);
        assert_eq!(fs::read(&state.file).unwrap(), before);
        assert_eq!(
            fs::read_dir(state.file.parent().unwrap()).unwrap().count(),
            1
        );
    }
    #[cfg(windows)]
    #[test]
    fn missing_drive_keeps_custom_destination_and_reports_recovery() {
        let root = tempfile::tempdir().unwrap();
        let missing = (b'D'..=b'Z')
            .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
            .find(|path| !path.exists())
            .expect("A missing drive letter is required for this regression");
        let file = root.path().join("preferences.json");
        let custom = missing.join("FauxMetadata Outputs");
        fs::write(
            &file,
            serde_json::to_vec(&Preferences {
                custom_destination: Some(custom.clone()),
                ..Preferences::default()
            })
            .unwrap(),
        )
        .unwrap();
        let state = PreferenceStore::load(file, Ok(root.path().join("Pictures")));
        assert!(!state.view().is_default);
        assert_eq!(state.view().destination.unwrap(), display_path(&custom));
        assert!(state
            .prepare_export()
            .unwrap_err()
            .contains("Check the drive"));
        assert!(!root.path().join("Pictures").exists());
    }
    #[cfg(windows)]
    #[test]
    fn native_pictures_resolution_returns_absolute_current_folder() {
        let pictures = pictures_folder().unwrap();
        assert!(pictures.is_absolute());
        assert_eq!(
            default_destination(&pictures).unwrap().parent(),
            Some(pictures.as_path())
        );
        println!("Windows Pictures known folder: {}", display_path(&pictures));
    }

    #[cfg(windows)]
    #[test]
    fn denied_windows_destination_and_folder_creation_fail_clearly() {
        use std::{os::windows::process::CommandExt, process::Command};
        let root = tempfile::tempdir().unwrap();
        let state = store(root.path());
        let destination = state.prepare_export().unwrap();
        // This ACL is applied only to this test's disposable directory and always restored.
        let identity = Command::new("whoami.exe")
            .args(["/user", "/fo", "csv", "/nh"])
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(identity.status.success());
        let identity = String::from_utf8(identity.stdout).unwrap();
        let sid = identity.split(',').nth(1).unwrap().trim().trim_matches('"');
        assert!(sid.starts_with("S-1-"));
        struct Restore<'a>(&'a Path, String);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                let status = Command::new("icacls.exe")
                    .arg(self.0)
                    .args(["/remove:d", &self.1])
                    .creation_flags(0x08000000)
                    .output()
                    .unwrap();
                assert!(status.status.success(), "Cannot restore test directory ACL");
            }
        }
        for path in [&destination, destination.parent().unwrap()] {
            if path != destination {
                fs::remove_dir(&destination).unwrap();
            }
            let principal = format!("*{sid}");
            let denied = format!("{principal}:(WD,AD)");
            let status = Command::new("icacls.exe")
                .arg(path)
                .args(["/deny", &denied])
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "{}",
                String::from_utf8_lossy(&status.stderr)
            );
            let restore = Restore(path, principal);
            let error = state.prepare_export().unwrap_err();
            drop(restore);
            assert!(
                error.contains(if path == destination {
                    "Cannot write"
                } else {
                    "Cannot create"
                }),
                "{error}"
            );
        }
    }
}
