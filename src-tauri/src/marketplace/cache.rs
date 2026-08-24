use crate::app_paths::AppPaths;
use crate::models::StudioVersionCatalog;
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;

const CACHE_FILE_NAME: &str = "studio-version-catalog.json";

pub fn load_cached_catalog(app: &AppHandle) -> Result<StudioVersionCatalog, String> {
    let path = cache_path(app)?;
    if !path.is_file() {
        return Ok(StudioVersionCatalog::default());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| crate::tr!("error-version-cache-read", error = error))?;
    serde_json::from_str(&content)
        .map_err(|error| crate::tr!("error-version-cache-invalid", error = error))
}
fn cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(AppPaths::from_app(app)?
        .cache_directory()
        .join(CACHE_FILE_NAME))
}

pub(super) fn save_catalog(app: &AppHandle, catalog: &StudioVersionCatalog) -> Result<(), String> {
    let path = cache_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| crate::tr!("error-version-cache-directory-create", error = error))?;
    }
    let content = serde_json::to_string_pretty(catalog)
        .map_err(|error| crate::tr!("error-version-cache-create", error = error))?;
    let temporary_path = path.with_extension("json.tmp");
    fs::write(&temporary_path, content)
        .map_err(|error| crate::tr!("error-version-cache-save", error = error))?;
    fs::rename(&temporary_path, &path)
        .map_err(|error| crate::tr!("error-version-cache-finalize", error = error))
}
