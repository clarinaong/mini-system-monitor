use super::{Settings, SettingsError, SettingsErrorCode};
use serde_json::Value;
use std::{io::ErrorKind, path::Path};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreBuilder;

const SETTINGS_KEY: &str = "settings";

pub(super) fn read<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Result<Settings, SettingsError> {
    let snapshot = tempfile::NamedTempFile::new().map_err(SettingsError::load)?;
    match std::fs::copy(path, snapshot.path()) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(SettingsError::load(error)),
    }
    let store = StoreBuilder::new(app, snapshot.path())
        .create_new()
        .disable_auto_save()
        .build()
        .map_err(SettingsError::load)?;
    let result = match store.reload_ignore_defaults() {
        Err(error) => Err(SettingsError::load(error)),
        Ok(()) => {
            if store.keys().iter().any(|key| key != SETTINGS_KEY) {
                Err(SettingsError::load("Unrecognized settings file entries"))
            } else {
                decode(
                    store
                        .get(SETTINGS_KEY)
                        .unwrap_or_else(|| serde_json::json!({})),
                )
            }
        }
    };
    store.close_resource();
    result
}

fn decode(value: Value) -> Result<Settings, SettingsError> {
    if let Some(version) = value.get("version").and_then(Value::as_u64)
        && version != u64::from(super::model::SETTINGS_VERSION)
    {
        return Err(SettingsError::new(
            SettingsErrorCode::UnsupportedVersion,
            format!(
                "Settings version {version} is unsupported. Update the app or restore a supported backup, then retry; the file was preserved."
            ),
        ));
    }
    let settings: Settings = serde_json::from_value(value).map_err(SettingsError::load)?;
    settings.validate().map_err(SettingsError::load)?;
    Ok(settings)
}

pub(super) fn write<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
    settings: &Settings,
) -> Result<(), SettingsError> {
    let parent = path
        .parent()
        .ok_or_else(|| SettingsError::save("Invalid settings path"))?;
    std::fs::create_dir_all(parent).map_err(SettingsError::save)?;
    let pending = tempfile::NamedTempFile::new_in(parent).map_err(SettingsError::save)?;
    let store = StoreBuilder::new(app, pending.path())
        .create_new()
        .disable_auto_save()
        .default(
            SETTINGS_KEY,
            serde_json::to_value(settings).map_err(SettingsError::save)?,
        )
        .build()
        .map_err(SettingsError::save)?;
    let result = store.save().map_err(SettingsError::save);
    store.close_resource();
    result?;
    pending.as_file().sync_all().map_err(SettingsError::save)?;
    pending.persist(path).map_err(SettingsError::save)?;
    Ok(())
}
