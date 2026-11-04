pub mod container;
pub mod engine;
pub mod exiftool;
pub mod model;
pub mod preferences;

use engine::Engine;
use model::*;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::Ordering, Arc, Mutex},
};
use tauri::{Emitter, Manager, State};

type Shared = Arc<Engine>;
struct UserSettings(Mutex<preferences::PreferenceStore>);
#[derive(Default)]
struct Published(Mutex<HashMap<String, PathBuf>>);

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    version: String,
    exiftool: String,
    offline: bool,
}

#[tauri::command]
async fn health(state: State<'_, Shared>) -> AppResult<Health> {
    let engine = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let version = engine
            .tool
            .ensure_version(&std::sync::atomic::AtomicBool::new(false))?;
        Ok(Health {
            version: env!("CARGO_PKG_VERSION").into(),
            exiftool: version,
            offline: true,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn choose_inputs(folders: bool, window: tauri::WebviewWindow) -> AppResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title(if folders {
                "Add image folders"
            } else {
                "Add PNG, JPEG or WebP images"
            });
        let files = if folders {
            dialog.pick_folders()
        } else {
            dialog
                .add_filter("PNG, JPEG and WebP", &["png", "jpg", "jpeg", "webp"])
                .pick_files()
        };
        files
            .unwrap_or_default()
            .into_iter()
            .map(|v| v.to_string_lossy().into())
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn choose_destination(
    state: State<'_, Shared>,
    settings: State<'_, UserSettings>,
    window: tauri::WebviewWindow,
) -> AppResult<Option<preferences::PreferenceView>> {
    let _guard = state.begin()?;
    let initial = settings
        .0
        .lock()
        .map_err(|_| "Preferences unavailable.")?
        .view()
        .destination;
    let chosen = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("Choose where cleaned images are saved");
        if let Some(path) = initial {
            if std::path::Path::new(&path).is_dir() {
                dialog = dialog.set_directory(path);
            }
        }
        dialog.pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    chosen
        .map(|path| {
            settings
                .0
                .lock()
                .map_err(|_| "Preferences unavailable.")?
                .choose(&path)
        })
        .transpose()
}

#[tauri::command]
fn get_preferences(settings: State<'_, UserSettings>) -> AppResult<preferences::PreferenceView> {
    Ok(settings
        .0
        .lock()
        .map_err(|_| "Preferences unavailable.")?
        .view())
}

#[tauri::command]
fn reset_destination(
    state: State<'_, Shared>,
    settings: State<'_, UserSettings>,
) -> AppResult<preferences::PreferenceView> {
    let _guard = state.begin()?;
    settings
        .0
        .lock()
        .map_err(|_| "Preferences unavailable.")?
        .reset()
}

#[tauri::command]
fn set_auto_clean(
    enabled: bool,
    state: State<'_, Shared>,
    settings: State<'_, UserSettings>,
) -> AppResult<preferences::PreferenceView> {
    let _guard = state.begin()?;
    settings
        .0
        .lock()
        .map_err(|_| "Preferences unavailable.")?
        .auto_clean(enabled)
}

#[tauri::command]
async fn open_cleaned_images(id: String, published: State<'_, Published>) -> AppResult<()> {
    let output = published
        .0
        .lock()
        .map_err(|_| "Outputs unavailable.")?
        .get(&id)
        .cloned()
        .ok_or("No verified output for this image.")?;
    tauri::async_runtime::spawn_blocking(move || {
        preferences::open_folder(output.parent().ok_or("Output folder unavailable.")?)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn import_images(
    paths: Vec<String>,
    app: tauri::AppHandle,
    state: State<'_, Shared>,
) -> AppResult<ImportResult> {
    if paths.len() > 500 {
        return Err("Drop at most 500 paths at a time.".into());
    }
    let engine = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine.import(
            paths.into_iter().map(PathBuf::from).collect(),
            |completed, total, name| {
                let _ = app.emit(
                    "import-progress",
                    serde_json::json!({ "completed": completed, "total": total, "name": name }),
                );
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn export_images(
    ids: Vec<String>,
    job: String,
    app: tauri::AppHandle,
    state: State<'_, Shared>,
    published: State<'_, Published>,
) -> AppResult<Vec<ExportResult>> {
    if job.len() > 80 {
        return Err("Invalid job ID.".into());
    }
    let engine = state.inner().clone();
    let results = tauri::async_runtime::spawn_blocking(move || {
        let total = ids.len();
        // The worker reserves the operation before destination creation/probing.
        engine.export_prepared(
            &ids,
            || {
                app.state::<UserSettings>()
                    .0
                    .lock()
                    .map_err(|_| "Preferences unavailable.")?
                    .prepare_export()
            },
            |id, phase, completed, result| {
                let _ = app.emit(
                    "export-progress",
                    Progress {
                        job: job.clone(),
                        id: id.into(),
                        phase: phase.into(),
                        completed,
                        total,
                        result,
                    },
                );
            },
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut outputs = published.0.lock().map_err(|_| "Outputs unavailable.")?;
    for result in &results {
        if result.status == "verified" {
            if let Some(path) = &result.output {
                outputs.insert(result.id.clone(), PathBuf::from(path));
            }
        }
    }
    Ok(results)
}

#[tauri::command]
fn cancel_operation(state: State<'_, Shared>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn remove_images(
    ids: Vec<String>,
    state: State<'_, Shared>,
    published: State<'_, Published>,
) -> AppResult<()> {
    let _guard = state.begin()?;
    let mut sources = state.sources.lock().map_err(|_| "Queue unavailable.")?;
    for id in ids {
        sources.remove(&id);
        published
            .0
            .lock()
            .map_err(|_| "Outputs unavailable.")?
            .remove(&id);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let bundled = app
                .path()
                .resource_dir()?
                .join("resources/exiftool/exiftool.exe");
            #[cfg(debug_assertions)]
            let bundled = if bundled.exists() {
                bundled
            } else {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/exiftool/exiftool.exe")
            };
            app.manage(Arc::new(Engine::new(bundled)));
            app.manage(UserSettings(Mutex::new(
                preferences::PreferenceStore::load(
                    app.path().app_config_dir()?.join("preferences.json"),
                    preferences::pictures_folder(),
                ),
            )));
            app.manage(Published::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            health,
            choose_inputs,
            choose_destination,
            get_preferences,
            reset_destination,
            set_auto_clean,
            open_cleaned_images,
            import_images,
            export_images,
            cancel_operation,
            remove_images
        ])
        .run(tauri::generate_context!())
        .expect("FauxMetadata could not start");
}
