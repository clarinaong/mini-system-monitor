mod alerts;
mod history;
mod incidents;
mod monitor;
mod notifications;
mod settings;
mod windows;

use history::HistoryState;
use monitor::{MonitorState, get_system_metrics};
use notifications::{
    NotificationState, get_notification_status, request_notification_permission,
    send_test_notification,
};
use settings::{SettingsState, get_settings, save_settings};
use tauri::{
    Emitter, Manager,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_window_state::StateFlags;
use windows::resize_widget;

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::POSITION)
                .with_filter(|label| label == "main")
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_system_metrics,
            get_settings,
            save_settings,
            history::recorder::clear_history,
            history::recorder::query_history,
            resize_widget,
            get_notification_status,
            request_notification_permission,
            send_test_notification
        ])
        .setup(|app| {
            app.manage(NotificationState::native(app.handle().clone()));
            app.manage(SettingsState::load(app.handle()));
            let settings = app
                .state::<SettingsState>()
                .current_settings()
                .map_err(|error| error.to_string())?;
            match HistoryState::start(app.handle(), settings.history.recording_enabled) {
                Ok(history) => {
                    app.manage(history);
                }
                Err(error) => eprintln!("Could not start local history storage: {error}"),
            }
            app.manage(MonitorState::new());
            app.state::<MonitorState>().start(app.handle(), &settings)?;
            #[cfg(target_os = "macos")]
            {
                let window = app
                    .get_webview_window("main")
                    .ok_or("Failed to find main window")?;

                let ns_window_ptr = window.ns_window()? as *mut NSWindow;

                let ns_window = unsafe { &*ns_window_ptr };

                let mut behavior = ns_window.collectionBehavior();

                behavior.remove(
                    NSWindowCollectionBehavior::Managed | NSWindowCollectionBehavior::Transient,
                );

                behavior.insert(NSWindowCollectionBehavior::Stationary);

                ns_window.setCollectionBehavior(behavior);
                // Keep the widget below normal application windows.
                ns_window.setLevel(-1);
            }

            // Create menu items
            let show_item = MenuItem::with_id(app, "show", "Show Widget", true, None::<&str>)?;

            let hide_item = MenuItem::with_id(app, "hide", "Hide Widget", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;

            let quit_item =
                MenuItem::with_id(app, "quit", "Quit Mini System Monitor", true, None::<&str>)?;

            // Combine the items into one menu
            let menu =
                Menu::with_items(app, &[&show_item, &hide_item, &settings_item, &quit_item])?;

            // Create the menu bar icon
            let tray_icon =
                tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

            TrayIconBuilder::new()
                .icon(tray_icon)
                .icon_as_template(false)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main")
                            && window.show().is_ok()
                        {
                            let _ = app.emit_to("main", "widget-visibility", true);
                        }
                    }

                    "hide" => {
                        if let Some(window) = app.get_webview_window("main")
                            && window.hide().is_ok()
                        {
                            let _ = app.emit_to("main", "widget-visibility", false);
                        }
                    }

                    "settings" => {
                        if let Err(error) = windows::open_settings(app) {
                            eprintln!("Unable to open Settings: {error}");
                        }
                    }

                    "quit" => {
                        app.exit(0);
                    }

                    _ => {}
                })
                .build(app)?;

            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(monitor) = app.try_state::<MonitorState>() {
                    monitor.stop();
                }
                if let Some(history) = app.try_state::<HistoryState>() {
                    history.stop();
                }
            }
        });
}
