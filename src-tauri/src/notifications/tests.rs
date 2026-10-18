use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tauri::test::{mock_builder, mock_context, noop_assets};

struct FakeBackend {
    permission: Mutex<Permission>,
    requests: AtomicUsize,
    submissions: AtomicUsize,
    fail_status: bool,
    fail_request: bool,
    fail_submission: bool,
    requested_permission: Permission,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            permission: Mutex::new(Permission::NotDetermined),
            requests: AtomicUsize::new(0),
            submissions: AtomicUsize::new(0),
            fail_status: false,
            fail_request: false,
            fail_submission: false,
            requested_permission: Permission::Authorized,
        }
    }
}

impl Backend for Arc<FakeBackend> {
    fn permission(&self) -> Result<Permission, NotificationError> {
        if self.fail_status {
            return Err(NotificationError::new(
                "statusUnavailable",
                "Injected failure",
            ));
        }
        Ok(*self.permission.lock().unwrap())
    }
    fn request_permission(&self) -> Result<(), NotificationError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if self.fail_request {
            return Err(NotificationError::new("requestFailed", "Injected failure"));
        }
        *self.permission.lock().unwrap() = self.requested_permission;
        Ok(())
    }
    fn submit_test(&self) -> Result<(), NotificationError> {
        self.submissions.fetch_add(1, Ordering::SeqCst);
        if self.fail_submission {
            return Err(NotificationError::new(
                "submissionFailed",
                "Injected failure",
            ));
        }
        Ok(())
    }
}

fn state(backend: &Arc<FakeBackend>) -> NotificationState {
    NotificationState {
        backend: Box::new(Arc::clone(backend)),
        operation: Mutex::new(()),
    }
}

#[test]
fn status_checks_never_request_or_submit_and_serialize_capabilities() {
    for permission in [
        Permission::NotDetermined,
        Permission::Denied,
        Permission::Authorized,
        Permission::Provisional,
        Permission::Unavailable,
        Permission::Unknown,
    ] {
        let backend = Arc::new(FakeBackend::default());
        *backend.permission.lock().unwrap() = permission;
        let status = state(&backend).execute(Action::Status).unwrap();
        assert_eq!(status.permission, permission);
        assert_eq!(status.can_request, permission == Permission::NotDetermined);
        assert_eq!(status.can_send_test, permission.allows_submission());
        let json = serde_json::to_value(status).unwrap();
        assert!(json.get("canRequest").is_some());
        assert!(json.get("canSendTest").is_some());
        assert_eq!(backend.requests.load(Ordering::SeqCst), 0);
        assert_eq!(backend.submissions.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn explicit_permission_request_returns_authoritative_status_without_sending() {
    for permission in [Permission::Authorized, Permission::Denied] {
        let backend = Arc::new(FakeBackend {
            requested_permission: permission,
            ..FakeBackend::default()
        });
        let status = state(&backend).execute(Action::Request).unwrap();
        assert_eq!(status.permission, permission);
        assert_eq!(backend.requests.load(Ordering::SeqCst), 1);
        assert_eq!(backend.submissions.load(Ordering::SeqCst), 0);
        state(&backend).execute(Action::Request).unwrap();
        assert_eq!(backend.requests.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn denied_unknown_and_unavailable_do_not_request_or_submit_implicitly() {
    for permission in [
        Permission::Denied,
        Permission::Unavailable,
        Permission::Unknown,
        Permission::NotDetermined,
    ] {
        let backend = Arc::new(FakeBackend::default());
        *backend.permission.lock().unwrap() = permission;
        assert_eq!(
            state(&backend).execute(Action::Test).unwrap_err().code,
            "permissionRequired"
        );
        assert_eq!(backend.requests.load(Ordering::SeqCst), 0);
        assert_eq!(backend.submissions.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn test_submission_rechecks_permission_and_reports_submission_errors() {
    for fail_submission in [false, true] {
        let backend = Arc::new(FakeBackend {
            permission: Mutex::new(Permission::Authorized),
            fail_submission,
            ..FakeBackend::default()
        });
        let state = state(&backend);
        assert_eq!(state.execute(Action::Test).is_err(), fail_submission);
        assert_eq!(backend.submissions.load(Ordering::SeqCst), 1);
        *backend.permission.lock().unwrap() = Permission::Denied;
        assert!(state.execute(Action::Test).is_err());
        assert_eq!(backend.submissions.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn status_request_errors_and_busy_state_are_nonfatal_without_sending() {
    for (fail_status, fail_request) in [(true, false), (false, true)] {
        let backend = Arc::new(FakeBackend {
            fail_status,
            fail_request,
            ..FakeBackend::default()
        });
        assert!(state(&backend).execute(Action::Request).is_err());
        assert_eq!(backend.submissions.load(Ordering::SeqCst), 0);
    }
    let backend = Arc::new(FakeBackend::default());
    let state = state(&backend);
    let guard = state.operation.lock().unwrap();
    assert!(state.execute(Action::Status).is_ok());
    assert_eq!(state.execute(Action::Request).unwrap_err().code, "busy");
    drop(guard);
    assert!(state.execute(Action::Request).is_ok());
}

#[test]
fn widget_callers_are_rejected_for_all_notification_commands() {
    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    for action in [Action::Status, Action::Request, Action::Test] {
        assert_eq!(
            tauri::async_runtime::block_on(run(window.clone(), action))
                .unwrap_err()
                .code,
            "forbidden"
        );
    }
}

#[test]
fn settings_commands_use_managed_mock_adapter_without_touching_the_os() {
    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let backend = Arc::new(FakeBackend::default());
    app.manage(state(&backend));
    let window = tauri::WebviewWindowBuilder::new(&app, "settings", Default::default())
        .build()
        .unwrap();
    assert_eq!(
        tauri::async_runtime::block_on(get_notification_status(window.clone()))
            .unwrap()
            .permission,
        Permission::NotDetermined
    );
    assert_eq!(backend.requests.load(Ordering::SeqCst), 0);
    assert!(
        tauri::async_runtime::block_on(request_notification_permission(window.clone()))
            .unwrap()
            .can_send_test
    );
    tauri::async_runtime::block_on(send_test_notification(window)).unwrap();
    assert_eq!(backend.submissions.load(Ordering::SeqCst), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn unbundled_test_process_does_not_claim_native_authorization() {
    assert_eq!(
        macos::permission("com.minisystemmonitor").unwrap(),
        Permission::Unavailable
    );
}
