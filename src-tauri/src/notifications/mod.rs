use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;

#[cfg(target_os = "macos")]
mod macos;

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
}

impl From<Permission> for NotificationStatus {
    fn from(permission: Permission) -> Self {
        Self {
            permission,
            can_request: permission == Permission::NotDetermined,
            can_send_test: permission.allows_submission(),
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

pub(super) trait Backend: Send + Sync {
    fn permission(&self) -> Result<Permission, NotificationError>;
    fn request_permission(&self) -> Result<(), NotificationError>;
    fn submit_test(&self) -> Result<(), NotificationError>;
}

struct NativeBackend<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> Backend for NativeBackend<R> {
    fn permission(&self) -> Result<Permission, NotificationError> {
        #[cfg(target_os = "macos")]
        {
            macos::permission(&self.app.config().identifier)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Permission::Unavailable)
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

    fn submit_test(&self) -> Result<(), NotificationError> {
        #[cfg(target_os = "macos")]
        {
            macos::submit_test()
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.app
                .notification()
                .builder()
                .title("Mini System Monitor")
                .body("This is a silent test notification from Mini System Monitor.")
                .show()
                .map_err(|error| NotificationError::new("submissionFailed", error.to_string()))
        }
    }
}

pub struct NotificationState {
    backend: Box<dyn Backend>,
    operation: Mutex<()>,
}

impl NotificationState {
    pub fn native<R: Runtime>(app: AppHandle<R>) -> Self {
        Self {
            backend: Box::new(NativeBackend { app }),
            operation: Mutex::new(()),
        }
    }

    fn execute(&self, action: Action) -> Result<NotificationStatus, NotificationError> {
        if matches!(action, Action::Status) {
            return self.backend.permission().map(Into::into);
        }
        let _guard = self.operation.try_lock().map_err(|_| {
            NotificationError::new(
                "busy",
                "A notification operation is already in progress. Try again when it finishes.",
            )
        })?;
        let permission = self.backend.permission()?;
        match action {
            Action::Status => Ok(permission.into()),
            Action::Request => {
                if permission == Permission::NotDetermined {
                    self.backend.request_permission()?;
                    return self.backend.permission().map(Into::into);
                }
                Ok(permission.into())
            }
            Action::Test => {
                if !permission.allows_submission() {
                    return Err(NotificationError::new(
                        "permissionRequired",
                        "Notifications are not authorized. Refresh permission status and review macOS System Settings → Notifications.",
                    ));
                }
                self.backend.submit_test()?;
                Ok(permission.into())
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
