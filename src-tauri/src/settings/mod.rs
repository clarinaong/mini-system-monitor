mod model;
mod persistence;

pub use model::Settings;
use serde::Serialize;
use std::{fmt, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub const SETTINGS_CHANGED_EVENT: &str = "settings-changed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsErrorCode {
    InvalidData,
    UnsupportedVersion,
    Validation,
    SaveFailed,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsError {
    pub code: SettingsErrorCode,
    pub message: String,
}

impl SettingsError {
    fn new(code: SettingsErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn load(error: impl fmt::Display) -> Self {
        Self::new(
            SettingsErrorCode::InvalidData,
            format!(
                "Cannot read saved preferences: {error}. The file was preserved. Restore or move it aside, then retry."
            ),
        )
    }

    fn save(error: impl fmt::Display) -> Self {
        Self::new(
            SettingsErrorCode::SaveFailed,
            format!(
                "Preferences were not saved: {error}. Check available space and folder permissions, then retry."
            ),
        )
    }
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub settings: Settings,
    pub load_error: Option<SettingsError>,
    pub notification_delivery_available: bool,
}

pub struct SettingsState {
    path: Result<PathBuf, SettingsError>,
    snapshot: Mutex<SettingsSnapshot>,
}

impl SettingsState {
    pub fn current_settings(&self) -> Result<Settings, SettingsError> {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.settings.clone())
            .map_err(|_| state_unavailable())
    }

    pub fn load<R: Runtime>(app: &AppHandle<R>) -> Self {
        let path = app
            .path()
            .app_data_dir()
            .map(|dir| dir.join("settings.json"))
            .map_err(|error| SettingsError::new(SettingsErrorCode::Unavailable, error.to_string()));
        Self::at_path(app, path)
    }

    fn at_path<R: Runtime>(app: &AppHandle<R>, path: Result<PathBuf, SettingsError>) -> Self {
        let loaded = path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|path| persistence::read(app, path));
        let (settings, load_error) = match loaded {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(error)),
        };
        Self {
            path,
            snapshot: Mutex::new(SettingsSnapshot {
                settings,
                load_error,
                notification_delivery_available: crate::notifications::available(app),
            }),
        }
    }

    fn get<R: Runtime>(&self, app: &AppHandle<R>) -> Result<SettingsSnapshot, SettingsError> {
        let mut snapshot = self.snapshot.lock().map_err(|_| state_unavailable())?;
        if snapshot.load_error.is_some() {
            match self
                .path
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|path| persistence::read(app, path))
            {
                Ok(settings) => {
                    snapshot.settings = settings;
                    snapshot.load_error = None;
                    apply_monitor_settings(app, &snapshot.settings);
                }
                Err(error) => snapshot.load_error = Some(error),
            }
        }
        Ok(snapshot.clone())
    }

    fn save<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        settings: Settings,
        publish: impl FnOnce(&Settings),
    ) -> Result<Settings, SettingsError> {
        settings.validate()?;
        let mut snapshot = self.snapshot.lock().map_err(|_| state_unavailable())?;
        let path = self.path.as_ref().map_err(Clone::clone)?;
        if let Err(error) = persistence::read(app, path) {
            snapshot.load_error = Some(error.clone());
            return Err(error);
        }
        persistence::write(app, path, &settings)?;
        snapshot.settings = settings.clone();
        snapshot.load_error = None;
        apply_monitor_settings(app, &settings);
        publish(&settings);
        Ok(settings)
    }
}

fn apply_monitor_settings<R: Runtime>(app: &AppHandle<R>, settings: &Settings) {
    if let Some(history) = app.try_state::<crate::history::HistoryState>() {
        history.set_enabled(settings.history.recording_enabled);
    }
    if let Some(monitor) = app.try_state::<crate::monitor::MonitorState>()
        && let Err(error) = monitor.apply_settings(settings)
    {
        eprintln!("Saved monitoring preferences could not be applied: {error}");
    }
}

fn state_unavailable() -> SettingsError {
    SettingsError::new(
        SettingsErrorCode::Unavailable,
        "Preferences are unavailable. Restart the app and retry.",
    )
}

#[tauri::command]
pub async fn get_settings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<SettingsSnapshot, SettingsError> {
    tauri::async_runtime::spawn_blocking(move || app.state::<SettingsState>().get(&app))
        .await
        .map_err(|_| state_unavailable())?
}

#[tauri::command]
pub async fn save_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: Settings,
) -> Result<Settings, SettingsError> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<SettingsState>().save(&app, settings, |saved| {
            if let Err(error) = app.emit(SETTINGS_CHANGED_EVENT, saved) {
                eprintln!("Preferences saved, but settings-changed delivery failed: {error}");
            }
        })
    })
    .await
    .map_err(|_| state_unavailable())?
}

#[cfg(test)]
mod tests;
