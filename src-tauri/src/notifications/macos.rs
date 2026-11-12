use super::{NotificationError, NotificationStatus, Permission};
use block2::RcBlock;
use objc2::{
    AnyThread, define_class,
    rc::Retained,
    runtime::{Bool, ProtocolObject},
};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString, NSUUID};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationSetting,
    UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use std::{
    ptr::NonNull,
    sync::{OnceLock, mpsc},
    time::Duration,
};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MiniSystemMonitorNotificationDelegate"]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn present(
            &self,
            _center: &UNUserNotificationCenter,
            notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            let mut options = UNNotificationPresentationOptions::Banner;
            if notification.request().content().sound().is_some() {
                options |= UNNotificationPresentationOptions::Sound;
            }
            completion.call((options,));
        }
    }
);

pub fn submit(message: &super::Message) -> Result<(), NotificationError> {
    static DELEGATE: OnceLock<Retained<NotificationDelegate>> = OnceLock::new();
    let center = UNUserNotificationCenter::currentNotificationCenter();
    DELEGATE.get_or_init(|| {
        let delegate: Retained<NotificationDelegate> =
            unsafe { objc2::msg_send![super(NotificationDelegate::alloc().set_ivars(())), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        delegate
    });

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&message.title));
    content.setBody(&NSString::from_str(&message.body));
    if message.sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSUUID::UUID().UUIDString(),
        &content,
        None,
    );
    let (sender, receiver) = mpsc::channel();
    let callback = RcBlock::new(move |error: *mut NSError| {
        let result = if error.is_null() {
            Ok(())
        } else {
            Err(NotificationError::new(
                "submissionFailed",
                unsafe { &*error }.localizedDescription().to_string(),
            ))
        };
        let _ = sender.send(result);
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&callback));
    receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| {
            NotificationError::new(
                "submissionFailed",
                "macOS did not confirm notification submission.",
            )
        })?
}

pub fn available(identifier: &str) -> bool {
    let bundle = NSBundle::mainBundle();
    !tauri::is_dev()
        && bundle.bundlePath().to_string().ends_with(".app")
        && bundle
            .bundleIdentifier()
            .is_some_and(|value| value.to_string() == identifier)
}

pub fn status(identifier: &str) -> Result<NotificationStatus, NotificationError> {
    if !available(identifier) {
        return Ok(Permission::Unavailable.into());
    }
    let (sender, receiver) = mpsc::channel();
    let callback = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        let settings = unsafe { settings.as_ref() };
        let permission = match settings.authorizationStatus() {
            UNAuthorizationStatus::NotDetermined => Permission::NotDetermined,
            UNAuthorizationStatus::Denied => Permission::Denied,
            UNAuthorizationStatus::Authorized => Permission::Authorized,
            UNAuthorizationStatus::Provisional => Permission::Provisional,
            _ => Permission::Unknown,
        };
        let mut status = NotificationStatus::from(permission);
        status.sound_enabled = match settings.soundSetting() {
            UNNotificationSetting::Enabled => Some(true),
            UNNotificationSetting::Disabled => Some(false),
            _ => None,
        };
        let _ = sender.send(status);
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
