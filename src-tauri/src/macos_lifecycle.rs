use crate::monitor::MonitorState;
use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSScreen, NSWindow, NSWorkspace,
    NSWorkspaceDidWakeNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSOperationQueue,
    NSRect, NSSize,
};
use std::ptr::NonNull;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Observer {
    fn register(
        center: Retained<NSNotificationCenter>,
        name: &NSNotificationName,
        callback: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let block = RcBlock::new(move |_: NonNull<NSNotification>| callback());
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        Self { center, token }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        unsafe { self.center.removeObserver((*self.token).as_ref()) };
    }
}

pub struct Observers {
    _wake: Observer,
    _display: Observer,
}

impl Observers {
    pub fn register<R: Runtime>(app: &AppHandle<R>) -> Self {
        let wake_app = app.clone();
        let wake = Observer::register(
            NSWorkspace::sharedWorkspace().notificationCenter(),
            unsafe { NSWorkspaceDidWakeNotification },
            move || {
                if let Some(monitor) = wake_app.try_state::<MonitorState>() {
                    monitor.recheck_deadline();
                }
                recover_widget(&wake_app);
            },
        );
        let display_app = app.clone();
        let display = Observer::register(
            NSNotificationCenter::defaultCenter(),
            unsafe { NSApplicationDidChangeScreenParametersNotification },
            move || recover_widget(&display_app),
        );
        Self {
            _wake: wake,
            _display: display,
        }
    }
}

pub fn recover_widget<R: Runtime>(app: &AppHandle<R>) {
    if let Some(widget) = app.get_webview_window("main")
        && let Err(error) = adjust_widget(&widget, None)
    {
        eprintln!("Could not recover widget position: {error}");
    }
}

pub fn adjust_widget<R: Runtime>(
    widget: &WebviewWindow<R>,
    requested_height: Option<f64>,
) -> Result<(), String> {
    let main_thread = MainThreadMarker::new().ok_or("Widget placement requires the main thread")?;
    let pointer = widget.ns_window().map_err(|error| error.to_string())?;
    let window = unsafe { &*pointer.cast::<NSWindow>() };
    let screens = NSScreen::screens(main_thread);
    let visible_frames: Vec<_> = screens.iter().map(|screen| screen.visibleFrame()).collect();
    let original = window.frame();
    let area = preferred_area(original, &visible_frames);
    let mut frame = original;
    if let Some(height) = requested_height {
        frame.size = NSSize::new(360.0, height);
        frame.origin.y = original.max().y - height;
    }
    if let Some(area) = area {
        frame = fit_frame(frame, area);
    }
    if frame != original {
        window.setFrame_display(frame, true);
    }
    Ok(())
}

fn preferred_area(frame: NSRect, visible_frames: &[NSRect]) -> Option<NSRect> {
    fn overlap(a: NSRect, b: NSRect) -> f64 {
        let width = (a.max().x.min(b.max().x) - a.origin.x.max(b.origin.x)).max(0.0);
        let height = (a.max().y.min(b.max().y) - a.origin.y.max(b.origin.y)).max(0.0);
        width * height
    }

    let mut areas = visible_frames
        .iter()
        .copied()
        .filter(|area| !area.is_empty());
    let mut best = areas.next()?;
    let mut largest_overlap = overlap(frame, best);
    for area in areas {
        let area_overlap = overlap(frame, area);
        if area_overlap > largest_overlap {
            best = area;
            largest_overlap = area_overlap;
        }
    }
    Some(best)
}

fn fit_frame(mut frame: NSRect, area: NSRect) -> NSRect {
    let top = frame.max().y;
    frame.size.width = frame.size.width.min(area.size.width);
    frame.size.height = frame.size.height.min(area.size.height);
    frame.origin.x = frame
        .origin
        .x
        .clamp(area.origin.x, area.max().x - frame.size.width);
    frame.origin.y =
        (top - frame.size.height).clamp(area.origin.y, area.max().y - frame.size.height);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSPoint;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn preserves_a_visible_widget_on_a_secondary_display_with_negative_coordinates() {
        let primary = rect(0.0, 0.0, 1280.0, 760.0);
        let secondary = rect(-1600.0, -200.0, 1600.0, 900.0);
        let widget = rect(-1000.0, -100.0, 360.0, 400.0);
        assert_eq!(
            preferred_area(widget, &[primary, secondary]),
            Some(secondary)
        );
        assert_eq!(fit_frame(widget, secondary), widget);
    }

    #[test]
    fn recovers_a_disconnected_display_to_the_primary_work_area() {
        let primary = rect(0.0, 50.0, 1280.0, 710.0);
        let secondary = rect(1280.0, 0.0, 1600.0, 900.0);
        let widget = rect(-1000.0, -100.0, 360.0, 400.0);
        assert_eq!(preferred_area(widget, &[]), None);
        assert_eq!(preferred_area(widget, &[primary, secondary]), Some(primary));
        assert_eq!(fit_frame(widget, primary), rect(0.0, 50.0, 360.0, 400.0));
    }

    #[test]
    fn keeps_the_larger_overlap_and_fits_a_widget_after_work_area_shrinks() {
        let primary = rect(0.0, 0.0, 1280.0, 760.0);
        let secondary = rect(1280.0, 20.0, 600.0, 480.0);
        let widget = rect(1250.0, 100.0, 360.0, 650.0);
        assert_eq!(
            preferred_area(widget, &[primary, secondary]),
            Some(secondary)
        );
        assert_eq!(
            fit_frame(widget, secondary),
            rect(1280.0, 20.0, 360.0, 480.0)
        );
    }
}
