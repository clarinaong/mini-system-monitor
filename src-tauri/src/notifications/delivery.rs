use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Message {
    pub title: String,
    pub body: String,
    pub sound: bool,
}

impl Message {
    pub fn test() -> Self {
        Self {
            title: "Mini System Monitor".into(),
            body: "This is a silent test notification from Mini System Monitor.".into(),
            sound: false,
        }
    }

    fn alert(alert: &EligibleAlert, settings: &Settings) -> Option<Self> {
        let rules = &settings.alerts;
        let (enabled, title, body) = match alert.rule {
            AlertRule::Cpu => (
                rules.cpu.enabled,
                "Mini System Monitor — High CPU usage",
                format!(
                    "CPU usage is {:.1}% (threshold {}%).",
                    alert.value_percent, alert.threshold_percent
                ),
            ),
            AlertRule::Memory => (
                rules.memory.enabled,
                "Mini System Monitor — High memory usage",
                format!(
                    "Memory usage is {:.1}% (threshold {}%).",
                    alert.value_percent, alert.threshold_percent
                ),
            ),
            AlertRule::BatteryLow => (
                rules.battery_low.enabled,
                "Mini System Monitor — Low battery",
                format!(
                    "Battery is at {:.1}% (reminder at {}%).",
                    alert.value_percent, alert.threshold_percent
                ),
            ),
            AlertRule::BatteryCharging => (
                rules.battery_charging.enabled,
                "Charge target reached.",
                format!(
                    "Battery is at {:.1}% (target {}%). You can unplug the charger.",
                    alert.value_percent, alert.threshold_percent
                ),
            ),
        };
        (rules.enabled && enabled).then(|| Self {
            title: title.into(),
            body,
            sound: rules.notification_sound,
        })
    }
}

struct Reservation(Arc<AtomicBool>);

impl Reservation {
    fn acquire(busy: &Arc<AtomicBool>) -> Option<Self> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(Arc::clone(busy)))
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl NotificationState {
    pub(crate) fn dispatch<R: Runtime>(
        &self,
        app: AppHandle<R>,
        alerts: Vec<EligibleAlert>,
        revision: u64,
    ) -> Option<tauri::async_runtime::JoinHandle<()>> {
        let Some(reservation) = Reservation::acquire(&self.delivery_busy) else {
            eprintln!("Alert notification skipped: delivery is already busy");
            return None;
        };
        let backend = Arc::clone(&self.backend);
        let operation = Arc::clone(&self.operation);
        Some(tauri::async_runtime::spawn_blocking(move || {
            let _reservation = reservation;
            let Ok(_guard) = operation.try_lock() else {
                eprintln!("Alert notification skipped: a notification operation is in progress");
                return;
            };
            deliver_batch(&*backend, &alerts, |alert| {
                let monitor = app.try_state::<crate::monitor::MonitorState>()?;
                let settings = monitor.delivery_settings(revision)?;
                let now = monitor.snapshot().ok()?.monotonic_now_ms;
                fresh(alert, &settings, now).then_some(settings)
            });
        }))
    }
}

fn fresh(alert: &EligibleAlert, settings: &Settings, now: u64) -> bool {
    now >= alert.sample_monotonic_ms
        && now - alert.sample_monotonic_ms
            < u64::from(settings.monitoring_interval_seconds) * 2000 + 2000
}

fn deliver_batch(
    backend: &dyn Backend,
    alerts: &[EligibleAlert],
    mut settings: impl FnMut(&EligibleAlert) -> Option<Settings>,
) {
    for alert in alerts {
        let Some(current) = settings(alert) else {
            continue;
        };
        if Message::alert(alert, &current).is_none() {
            continue;
        }
        match backend.permission() {
            Ok(permission) if permission.allows_submission() => {}
            Ok(_) => continue,
            Err(error) => {
                eprintln!(
                    "Cannot check alert notification permission: {}",
                    error.message
                );
                continue;
            }
        }
        let Some(current) = settings(alert) else {
            continue;
        };
        let Some(message) = Message::alert(alert, &current) else {
            continue;
        };
        if let Err(error) = backend.submit(&message) {
            eprintln!("Cannot submit alert notification: {}", error.message);
        }
    }
}

#[cfg(test)]
mod tests;
