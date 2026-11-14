use super::{BatteryMetrics, SystemMetrics};
use std::time::Duration;
use sysinfo::System;

pub(super) struct CollectedMetrics {
    pub metrics: SystemMetrics,
    pub battery_error: Option<String>,
}

pub(super) trait Collector {
    fn collect(&mut self, now: Duration) -> Result<CollectedMetrics, String>;
}

pub(super) struct HardwareCollector {
    system: System,
    battery_manager: Option<battery::Manager>,
    previous_cpu_refresh: Option<Duration>,
}

impl HardwareCollector {
    pub fn new() -> Self {
        Self {
            system: System::new(),
            battery_manager: None,
            previous_cpu_refresh: None,
        }
    }

    fn read_battery(&mut self) -> Result<Option<BatteryMetrics>, String> {
        if self.battery_manager.is_none() {
            self.battery_manager =
                Some(battery::Manager::new().map_err(|error| error.to_string())?);
        }
        let result = (|| {
            let manager = self
                .battery_manager
                .as_ref()
                .ok_or("Battery manager is unavailable")?;
            let mut batteries = manager.batteries().map_err(|error| error.to_string())?;
            batteries
                .next()
                .transpose()
                .map_err(|error| error.to_string())
                .map(|battery| {
                    battery.map(|battery| BatteryMetrics {
                        percentage: (battery.state_of_charge().value * 100.0).clamp(0.0, 100.0),
                        status: format!("{:?}", battery.state()),
                    })
                })
        })();
        if result.is_err() {
            self.battery_manager = None;
        }
        result
    }
}

impl Collector for HardwareCollector {
    fn collect(&mut self, now: Duration) -> Result<CollectedMetrics, String> {
        let cpu_ready = self.previous_cpu_refresh.is_some_and(|previous| {
            now.saturating_sub(previous) >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL
        });
        self.system.refresh_cpu_usage();
        self.previous_cpu_refresh = Some(now);
        self.system.refresh_memory();
        if self.system.total_memory() == 0 {
            return Err("System memory readings are unavailable".into());
        }
        let (battery, battery_error) = match self.read_battery() {
            Ok(battery) => (battery, None),
            Err(error) => (None, Some(error)),
        };
        Ok(CollectedMetrics {
            metrics: SystemMetrics {
                cpu_usage: cpu_ready.then(|| self.system.global_cpu_usage().clamp(0.0, 100.0)),
                memory_used: self.system.used_memory(),
                memory_total: self.system.total_memory(),
                swap_used: self.system.used_swap(),
                battery,
            },
            battery_error,
        })
    }
}
