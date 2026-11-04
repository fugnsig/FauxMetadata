use crate::model::{AppResult, Field};
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_OUTPUT: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct ExifTool {
    pub path: PathBuf,
}

impl ExifTool {
    pub fn ensure_version(&self, cancel: &AtomicBool) -> AppResult<String> {
        let bytes = self.run(&["-ver"], None, cancel)?;
        let version = String::from_utf8_lossy(&bytes).trim().to_string();
        if version != "13.60" {
            return Err(
                "Bundled ExifTool version mismatch. Restore the verified application package."
                    .into(),
            );
        }
        Ok(version)
    }
    pub fn run(
        &self,
        args: &[&str],
        file: Option<&Path>,
        cancel: &AtomicBool,
    ) -> AppResult<Vec<u8>> {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled.".into());
        }
        let mut command = Command::new(&self.path);
        command
            .args([
                "-config",
                "",
                "-charset",
                "filename=UTF8",
                "-api",
                "WindowsLongPath=1",
            ])
            .args(args)
            .env("LC_ALL", "C")
            .env("LC_CTYPE", "C")
            .env("LANG", "C")
            .env_remove("PERL5OPT")
            .env_remove("PERL5LIB")
            .env_remove("PERLLIB")
            .env_remove("EXIFTOOL_HOME")
            .stdin(if file.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(parent) = self.path.parent() {
            command.current_dir(parent);
        }
        if let Some(file) = file {
            if !file.is_absolute() || file.to_string_lossy().contains(['\r', '\n']) {
                return Err(
                    "Unsafe image path; an absolute path without line breaks is required.".into(),
                );
            }
            // UTF-8 argument input avoids Windows ANSI command-line conversion and globbing problems.
            command.args(["-@", "-"]);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start bundled ExifTool: {e}"))?;
        if let Some(file) = file {
            let input = format!("--\n{}\n", file.to_string_lossy());
            let write = child
                .stdin
                .take()
                .ok_or("ExifTool input unavailable.")?
                .write_all(input.as_bytes());
            if let Err(e) = write {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Cannot send image path to ExifTool: {e}"));
            }
        }
        let out = child.stdout.take().ok_or("ExifTool stdout unavailable.")?;
        let err = child.stderr.take().ok_or("ExifTool stderr unavailable.")?;
        let overflow = Arc::new(AtomicBool::new(false));
        let read_pipe = |mut pipe: Box<dyn Read + Send>, overflow: Arc<AtomicBool>| {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let read = pipe.by_ref().take(MAX_OUTPUT + 1).read_to_end(&mut bytes);
                if bytes.len() as u64 > MAX_OUTPUT {
                    overflow.store(true, Ordering::SeqCst);
                }
                read.map(|_| bytes)
            })
        };
        let stdout = read_pipe(Box::new(out), overflow.clone());
        let stderr = read_pipe(Box::new(err), overflow.clone());
        let started = Instant::now();
        let result = loop {
            let failure = if cancel.load(Ordering::SeqCst) {
                Some("Cancelled.")
            } else if overflow.load(Ordering::SeqCst) {
                Some("ExifTool metadata exceeds the 16 MiB inspection limit.")
            } else if started.elapsed() > Duration::from_secs(45) {
                Some("ExifTool timed out after 45 seconds.")
            } else {
                None
            };
            if let Some(message) = failure {
                let _ = child.kill();
                let _ = child.wait();
                break Err(message.to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!("ExifTool process failed: {e}"));
                }
            }
        };
        let output = stdout
            .join()
            .map_err(|_| "Metadata reader failed.")?
            .map_err(|e| e.to_string())?;
        let errors = stderr
            .join()
            .map_err(|_| "Metadata error reader failed.")?
            .map_err(|e| e.to_string())?;
        let status = result?;
        if overflow.load(Ordering::SeqCst) {
            return Err("Metadata output limit exceeded.".into());
        }
        let benign_layout_warnings = args.contains(&"-overwrite_original")
            && String::from_utf8_lossy(&errors).lines().all(|line| {
                line.starts_with(
                    "Warning: [minor] Text/EXIF chunk(s) found after PNG IDAT (fixed) - ",
                )
            });
        if !status.success() || (!errors.is_empty() && !benign_layout_warnings) {
            return Err(format!(
                "ExifTool rejected this file: {}",
                String::from_utf8_lossy(&errors).trim()
            ));
        }
        Ok(output)
    }

    pub fn inspect(&self, file: &Path, cancel: &AtomicBool) -> AppResult<Vec<Field>> {
        // Family 4 retains duplicate tag instances; -a prevents hidden duplicate metadata.
        let output = self.run(
            &[
                "-j",
                "-G1:4",
                "-a",
                "-u",
                "-n",
                "-struct",
                "-api",
                "RequestAll=3",
            ],
            Some(file),
            cancel,
        )?;
        let values: Value = serde_json::from_slice(&output)
            .map_err(|e| format!("Invalid ExifTool response: {e}"))?;
        let object = values
            .as_array()
            .and_then(|v| v.first())
            .and_then(Value::as_object)
            .ok_or("Missing ExifTool metadata record.")?;
        let mut fields = Vec::new();
        for (key, value) in object {
            if key == "SourceFile" || key.starts_with("System:") || key.starts_with("ExifTool:") {
                if key.ends_with(":Error") || key.ends_with(":Warning") {
                    let warning = value.as_str().unwrap_or_default();
                    if warning == "[minor] Trailer data after PNG IEND chunk"
                        || warning.starts_with("[minor] Text/EXIF chunk(s) found after PNG IDAT (")
                    {
                        fields.push(Field {
                            key: key.clone(),
                            value: warning.into(),
                            category: "Other".into(),
                            retained: false,
                        });
                    } else {
                        return Err(format!("Metadata parser warning: {value}"));
                    }
                }
                continue;
            }
            if key.starts_with("Composite:") || key.starts_with("File:") {
                continue;
            }
            fields.push(Field {
                key: key.clone(),
                value: match value {
                    Value::String(v) => v.clone(),
                    _ => value.to_string(),
                },
                category: classify(key, value).into(),
                retained: permitted(key),
            });
        }
        fields.sort_by(|a, b| a.category.cmp(&b.category).then(a.key.cmp(&b.key)));
        Ok(fields)
    }

    pub fn strip(&self, file: &Path, before: &[Field], cancel: &AtomicBool) -> AppResult<()> {
        let mut args = vec![
            "-overwrite_original",
            "-all=",
            "-trailer:all=",
            "--ICC_Profile:All",
            "--JFIF:All",
            "--Adobe:All",
        ];
        let copies: Vec<_> = [
            ("IFD0:Orientation", "-IFD0:Orientation"),
            ("IFD0:XResolution", "-IFD0:XResolution"),
            ("IFD0:YResolution", "-IFD0:YResolution"),
            ("IFD0:ResolutionUnit", "-IFD0:ResolutionUnit"),
            ("IFD0:YCbCrPositioning", "-IFD0:YCbCrPositioning"),
            ("ExifIFD:ColorSpace", "-ExifIFD:ColorSpace"),
            ("ExifIFD:ExifVersion", "-ExifIFD:ExifVersion"),
            ("ExifIFD:FlashpixVersion", "-ExifIFD:FlashpixVersion"),
            (
                "ExifIFD:ComponentsConfiguration",
                "-ExifIFD:ComponentsConfiguration",
            ),
            ("InteropIFD:InteropIndex", "-InteropIFD:InteropIndex"),
            ("InteropIFD:InteropVersion", "-InteropIFD:InteropVersion"),
        ]
        .into_iter()
        .filter(|(key, _)| before.iter().any(|f| f.retained && f.key == *key))
        .map(|(_, argument)| argument)
        .collect();
        if !copies.is_empty() {
            args.extend(["-tagsfromfile", "@"]);
            args.extend(copies);
        }
        self.run(&args, Some(file), cancel)?;
        Ok(())
    }
}

pub fn category(key: &str) -> &'static str {
    let lower = key.to_ascii_lowercase();
    let tag = lower.rsplit(':').next().unwrap_or(&lower);
    let generation_context = lower.starts_with("png:") || lower.starts_with("xmp-");
    if generation_context
        && [
            "workflow",
            "prompt",
            "parameters",
            "comfy",
            "sampler",
            "seed",
            "cfg",
            "model",
            "checkpoint",
            "lora",
            "negativeprompt",
        ]
        .iter()
        .any(|word| tag.contains(word))
    {
        "Generation"
    } else if lower.contains("gps")
        || ["latitude", "longitude", "location"]
            .iter()
            .any(|v| tag.contains(v))
    {
        "Location"
    } else if key.starts_with("ICC")
        || (key.starts_with("RIFF:") && permitted(key))
        || [
            "Orientation",
            "ColorSpace",
            "InteropIndex",
            "Gamma",
            "SRGBRendering",
            "WhitePointX",
            "WhitePointY",
            "RedX",
            "RedY",
            "GreenX",
            "GreenY",
            "BlueX",
            "BlueY",
            "PixelsPerUnitX",
            "PixelsPerUnitY",
            "PixelUnits",
            "SignificantBits",
            "BackgroundColor",
            "Transparency",
        ]
        .contains(&key.rsplit(':').next().unwrap_or(key))
    {
        "Appearance"
    } else if lower.starts_with("xmp") {
        "XMP"
    } else if lower.starts_with("iptc") || lower.starts_with("photoshop") {
        "IPTC"
    } else if lower.starts_with("ifd")
        || lower.starts_with("exif")
        || lower.starts_with("interop")
        || lower.starts_with("makernotes")
    {
        "EXIF"
    } else {
        "Other"
    }
}

fn classify(key: &str, value: &Value) -> &'static str {
    let default = category(key);
    if permitted(key) || default == "Location" {
        return default;
    }
    if let Some(text) = value.as_str() {
        let text = text.to_ascii_lowercase();
        if text.contains("\"class_type\"")
            || (text.contains("\"nodes\"") && text.contains("\"links\""))
            || (text.contains("steps:") && text.contains("sampler:") && text.contains("seed:"))
        {
            return "Generation";
        }
    }
    default
}

pub fn permitted(key: &str) -> bool {
    let group = key.split(':').next().unwrap_or("");
    let tag = key.rsplit(':').next().unwrap_or("");
    group.starts_with("ICC")
        || group == "JFIF"
        || group == "Adobe"
        || (group == "RIFF"
            && [
                "ImageWidth",
                "ImageHeight",
                "VP8Version",
                "HorizontalScale",
                "VerticalScale",
                "AlphaIsUsed",
                "WebP_Flags",
                "AlphaPreprocessing",
                "AlphaFiltering",
                "AlphaCompression",
            ]
            .contains(&tag))
        || (group == "IFD0"
            && [
                "Orientation",
                "XResolution",
                "YResolution",
                "ResolutionUnit",
                "YCbCrPositioning",
            ]
            .contains(&tag))
        || (group == "ExifIFD"
            && [
                "ColorSpace",
                "ExifVersion",
                "ComponentsConfiguration",
                "FlashpixVersion",
            ]
            .contains(&tag))
        || (group == "InteropIFD" && ["InteropIndex", "InteropVersion"].contains(&tag))
        || (matches!(
            group,
            "PNG" | "PNG-pHYs" | "PNG-cHRM" | "PNG-sRGB" | "PNG-sBIT" | "PNG-bKGD"
        ) && [
            "ImageWidth",
            "ImageHeight",
            "BitDepth",
            "ColorType",
            "Compression",
            "Filter",
            "Interlace",
            "Palette",
            "Transparency",
            "Gamma",
            "SRGBRendering",
            "WhitePointX",
            "WhitePointY",
            "RedX",
            "RedY",
            "GreenX",
            "GreenY",
            "BlueX",
            "BlueY",
            "PixelsPerUnitX",
            "PixelsPerUnitY",
            "PixelUnits",
            "SignificantBits",
            "BackgroundColor",
            "ProfileName",
        ]
        .contains(&tag))
}

pub fn diff(before: &[Field], after: &[Field]) -> AppResult<(Vec<Field>, Vec<Field>)> {
    let residual: Vec<_> = after
        .iter()
        .filter(|f| !permitted(&f.key))
        .map(|f| f.key.as_str())
        .collect();
    if !residual.is_empty() {
        return Err(format!(
            "Verification blocked export: residual metadata: {}",
            residual.join(", ")
        ));
    }
    for old in before.iter().filter(|f| f.retained) {
        if !after
            .iter()
            .any(|f| f.key == old.key && f.value == old.value)
        {
            return Err(format!(
                "Verification blocked export: retained appearance field changed: {}",
                old.key
            ));
        }
    }
    let removed = before
        .iter()
        .filter(|old| {
            !after
                .iter()
                .any(|new| new.key == old.key && new.value == old.value)
        })
        .cloned()
        .collect();
    Ok((removed, after.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_residuals_and_changed_orientation() {
        let field = |key: &str, value: &str| Field {
            key: key.into(),
            value: value.into(),
            category: category(key).into(),
            retained: permitted(key),
        };
        assert!(diff(&[], &[field("PNG:Workflow", "{}")]).is_err());
        assert!(diff(
            &[field("IFD0:Orientation", "6")],
            &[field("IFD0:Orientation", "1")]
        )
        .is_err());
        assert!(diff(&[], &[field("XMP-dc:Creator", "private")]).is_err());
        assert!(diff(&[], &[field("IFD0:Copy1:Software", "private")]).is_err());
    }

    #[test]
    fn camera_model_is_exif_and_generation_comments_are_recognized() {
        assert_eq!(category("IFD0:Model"), "EXIF");
        assert_eq!(category("PNG:Model"), "Generation");
        assert_eq!(
            classify(
                "ExifIFD:UserComment",
                &Value::String("{\"1\":{\"class_type\":\"KSampler\"}}".into())
            ),
            "Generation"
        );
        assert_eq!(
            classify(
                "ExifIFD:UserComment",
                &Value::String("ordinary photo note".into())
            ),
            "EXIF"
        );
    }
}
