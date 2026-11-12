use crate::{
    alerts::{AlertRule, EligibleAlert},
    settings::Settings,
};
use serde::Serialize;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;

mod delivery;
#[cfg(target_os = "macos")]
mod macos;
use delivery::Message;

pub(crate) fn available<R: Runtime>(app: &AppHandle<R>) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::available(&app.config().identifier)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    NotDetermined,
    Denied,
    Authorized,
    Provisional,
    Unavailable,
    Unknown,
}

impl Permission {
    fn allows_submission(self) -> bool {
        matches!(self, Self::Authorized | Self::Provisional)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationStatus {
    permission: Permission,
    can_request: bool,
    can_send_test: bool,
    sound_enabled: Option<bool>,
}

impl From<Permission> for NotificationStatus {
    fn from(permission: Permission) -> Self {
        Self {
            permission,
            can_request: permission == Permission::NotDetermined,
            can_send_test: permission.allows_submission(),
            sound_enabled: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationError {
    code: &'static str,
    message: String,
}

impl NotificationError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

trait Backend: Send + Sync {
    fn permission(&self) -> Result<Permission, NotificationError>;
    fn status(&self) -> Result<NotificationStatus, NotificationError> {
        self.permission().map(Into::into)
    }
    fn request_permission(&self) -> Result<(), NotificationError>;
    fn submit(&self, message: &Message) -> Result<(), NotificationError>;
    fn submit_test(&self) -> Result<(), NotificationError> {
        self.submit(&Message::test())
    }
}

struct NativeBackend<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> Backend for NativeBackend<R> {
    fn permission(&self) -> Result<Permission, NotificationError> {
        #[cfg(target_os = "macos")]
        {
            macos::status(&self.app.config().identifier).map(|status| status.permission)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Permission::Unavailable)
        }
    }

    fn status(&self) -> Result<NotificationStatus, NotificationError> {
        #[cfg(target_os = "macos")]
        {
            macos::status(&self.app.config().identifier)
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.permission().map(Into::into)
        }
    }

    fn request_permission(&self) -> Result<(), NotificationError> {
        #[cfg(target_os = "macos")]
        {
            macos::request_permission()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(NotificationError::new(
                "unavailable",
                "Native authorization is supported on macOS only.",
            ))
        }
    }

    fn submit(&self, message: &Message) -> Result<(), NotificationError> {
        #[cfg(target_os = "macos")]
        {
            macos::submit(message)
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.app
                .notification()
                .builder()
                .title(&message.title)
                .body(&message.body)
                .show()
                .map_err(|error| NotificationError::new("submissionFailed", error.to_string()))
        }
    }
}

pub struct NotificationState {
    backend: Arc<dyn Backend>,
    operation: Arc<Mutex<()>>,
    delivery_busy: Arc<AtomicBool>,
}

impl NotificationState {
    pub fn native<R: Runtime>(app: AppHandle<R>) -> Self {
        Self {
            backend: Arc::new(NativeBackend { app }),
            operation: Arc::new(Mutex::new(())),
            delivery_busy: Arc::new(AtomicBool::new(false)),
        }
    }

    fn execute(&self, action: Action) -> Result<NotificationStatus, NotificationError> {
        if matches!(action, Action::Status) {
            return self.backend.status();
        }
        let _guard = self.operation.try_lock().map_err(|_| {
            NotificationError::new(
                "busy",
                "A notification operation is already in progress. Try again when it finishes.",
            )
        })?;
        let status = self.backend.status()?;
        let permission = status.permission;
        match action {
            Action::Status => Ok(status),
            Action::Request => {
                if status.can_request {
                    self.backend.request_permission()?;
                    return self.backend.status();
                }
                Ok(status)
            }
            Action::Test => {
                if !permission.allows_submission() {
                    return Err(NotificationError::new(
                        "permissionRequired",
                        "Notifications are not authorized. Refresh permission status and review macOS System Settings → Notifications.",
                    ));
                }
                self.backend.submit_test()?;
                Ok(status)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Status,
    Request,
    Test,
}

async fn run<R: Runtime>(
    window: WebviewWindow<R>,
    action: Action,
) -> Result<NotificationStatus, NotificationError> {
    if window.label() != "settings" {
        return Err(NotificationError::new(
            "forbidden",
            "Notification controls are available only in Settings.",
        ));
    }
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || app.state::<NotificationState>().execute(action))
        .await
        .map_err(|_| {
            NotificationError::new(
                "unavailable",
                "The notification operation could not complete. Try again.",
            )
        })?
}

#[tauri::command]
pub async fn get_notification_status<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<NotificationStatus, NotificationError> {
    run(window, Action::Status).await
}

#[tauri::command]
pub async fn request_notification_permission<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<NotificationStatus, NotificationError> {
    run(window, Action::Request).await
}

#[tauri::command]
pub async fn send_test_notification<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<NotificationStatus, NotificationError> {
    run(window, Action::Test).await
}

#[cfg(test)]
mod tests;
