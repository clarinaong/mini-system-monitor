use tauri::{
    AppHandle, LogicalSize, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

pub fn open_settings<R: Runtime>(app: &AppHandle<R>) -> Result<WebviewWindow<R>, String> {
    let window = match app.get_webview_window("settings") {
        Some(window) => window,
        None => WebviewWindowBuilder::new(
            app,
            "settings",
            WebviewUrl::App("index.html?view=settings".into()),
        )
        .title("Mini System Monitor Settings")
        .inner_size(540.0, 700.0)
        .min_inner_size(480.0, 460.0)
        .decorations(true)
        .transparent(false)
        .resizable(true)
        .always_on_bottom(false)
        .visible(false)
        .center()
        .build()
        .map_err(|error| error.to_string())?,
    };
    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    Ok(window)
}

pub fn open_history<R: Runtime>(app: &AppHandle<R>) -> Result<WebviewWindow<R>, String> {
    let window = match app.get_webview_window("history") {
        Some(window) => window,
        None => WebviewWindowBuilder::new(
            app,
            "history",
            WebviewUrl::App("index.html?view=history".into()),
        )
        .title("Mini System Monitor History")
        .inner_size(740.0, 660.0)
        .min_inner_size(480.0, 420.0)
        .decorations(true)
        .transparent(false)
        .resizable(true)
        .always_on_bottom(false)
        .visible(false)
        .center()
        .build()
        .map_err(|error| error.to_string())?,
    };
    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    Ok(window)
}

fn widget_height(requested: f64, available: f64) -> Result<f64, String> {
    if !requested.is_finite() || requested <= 0.0 {
        return Err("Invalid widget height".into());
    }
    Ok(requested
        .ceil()
        .clamp(140.0, available.clamp(140.0, 1000.0)))
}

#[tauri::command]
pub fn resize_widget<R: Runtime>(window: WebviewWindow<R>, height: f64) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the widget may resize itself".into());
    }
    let available = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .map(|monitor| f64::from(monitor.work_area().size.height) / monitor.scale_factor())
        .unwrap_or(1000.0);
    let height = widget_height(height, available)?;
    let position = window.outer_position().map_err(|error| error.to_string())?;
    window
        .set_size(LogicalSize::new(360.0, height))
        .map_err(|error| error.to_string())?;
    window
        .set_position(position)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    #[test]
    fn settings_window_is_reused() {
        let app = mock_builder().build(mock_context(noop_assets())).unwrap();
        let first = open_settings(app.handle()).unwrap();
        assert_eq!(first.label(), "settings");
        assert_eq!(app.webview_windows().len(), 1);
        let second = open_settings(app.handle()).unwrap();
        assert_eq!(first.label(), second.label());
        assert_eq!(app.webview_windows().len(), 1);
    }

    #[test]
    fn history_window_is_reused_separately_from_settings() {
        let app = mock_builder().build(mock_context(noop_assets())).unwrap();
        open_settings(app.handle()).unwrap();
        let first = open_history(app.handle()).unwrap();
        assert_eq!(first.label(), "history");
        let second = open_history(app.handle()).unwrap();
        assert_eq!(first.label(), second.label());
        assert_eq!(app.webview_windows().len(), 2);
        assert!(resize_widget(second, 400.0).is_err());
    }

    #[test]
    fn widget_height_is_finite_and_bounded_by_available_space() {
        assert_eq!(widget_height(80.0, 900.0).unwrap(), 140.0);
        assert_eq!(widget_height(400.2, 900.0).unwrap(), 401.0);
        assert_eq!(widget_height(1200.0, 720.0).unwrap(), 720.0);
        assert_eq!(widget_height(5000.0, 5000.0).unwrap(), 1000.0);
        for invalid in [f64::NAN, f64::INFINITY, 0.0, -10.0] {
            assert!(widget_height(invalid, 900.0).is_err());
        }
    }

    #[test]
    fn settings_window_cannot_resize_the_widget() {
        let app = mock_builder().build(mock_context(noop_assets())).unwrap();
        let settings = open_settings(app.handle()).unwrap();
        assert!(resize_widget(settings, 400.0).is_err());
    }
}
