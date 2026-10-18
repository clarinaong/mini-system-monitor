use super::{NotificationError, Permission};
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
};
use std::{ptr::NonNull, sync::mpsc, time::Duration};

pub fn submit(message: &super::Message) -> Result<(), NotificationError> {
    let mut notification = notify_rust::Notification::new();
    notification.summary(&message.title).body(&message.body);
    if message.sound {
        notification.sound_name("Basso");
    }
    notification
        .show()
        .map(drop)
        .map_err(|error| NotificationError::new("submissionFailed", error.to_string()))
}

pub fn available(identifier: &str) -> bool {
    let bundle = NSBundle::mainBundle();
    !tauri::is_dev()
        && bundle.bundlePath().to_string().ends_with(".app")
        && bundle
            .bundleIdentifier()
            .is_some_and(|value| value.to_string() == identifier)
}

pub fn permission(identifier: &str) -> Result<Permission, NotificationError> {
    if !available(identifier) {
        return Ok(Permission::Unavailable);
    }
    let (sender, receiver) = mpsc::channel();
    let callback = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let permission = match status {
            UNAuthorizationStatus::NotDetermined => Permission::NotDetermined,
            UNAuthorizationStatus::Denied => Permission::Denied,
            UNAuthorizationStatus::Authorized => Permission::Authorized,
            UNAuthorizationStatus::Provisional => Permission::Provisional,
            _ => Permission::Unknown,
        };
        let _ = sender.send(permission);
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .getNotificationSettingsWithCompletionHandler(&callback);
    receiver.recv_timeout(Duration::from_secs(10)).map_err(|_| {
        NotificationError::new(
            "statusUnavailable",
            "macOS notification status did not respond. Try Refresh status.",
        )
    })
}

pub fn request_permission() -> Result<(), NotificationError> {
    let (sender, receiver) = mpsc::channel();
    let callback = RcBlock::new(move |_: Bool, error: *mut NSError| {
        let result = if error.is_null() {
            Ok(())
        } else {
            Err(NotificationError::new(
                "requestFailed",
                unsafe { &*error }.localizedDescription().to_string(),
            ))
        };
        let _ = sender.send(result);
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &callback,
        );
    receiver.recv_timeout(Duration::from_secs(120)).map_err(|_| {
        NotificationError::new("requestPending", "The macOS permission request has not completed. Respond to the system dialog, then refresh status.")
    })?
}
