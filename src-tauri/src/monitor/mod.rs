use serde::Serialize;
use std::sync::Mutex;
use sysinfo::System;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetrics {
    pub cpu_usage: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub battery: Option<BatteryMetrics>,
}

pub struct MonitorState {
    system: Mutex<System>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryMetrics {
    pub percentage: f32,
    pub status: String,
}

impl MonitorState {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self {
            system: Mutex::new(sys),
        }
    }
    pub fn get_metrics(&self) -> Result<SystemMetrics, String> {
        let (cpu_usage, memory_used, memory_total) = {
            let mut sys = self
                .system
                .lock()
                .map_err(|_| "Failed to acquire system monitor lock".to_string())?; // sys is MutexGuard<System>
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let cpu_usage = sys.global_cpu_usage();
            let memory_used = sys.used_memory();
            let memory_total = sys.total_memory();
            (cpu_usage, memory_used, memory_total)
        };
        let battery = self.read_battery().unwrap_or(None);
        Ok(SystemMetrics {
            cpu_usage,
            memory_used,
            memory_total,
            battery,
        })
    }
    pub fn read_battery(&self) -> Result<Option<BatteryMetrics>, String> {
        let manager = battery::Manager::new().map_err(|error| error.to_string())?;
        let mut batteries = manager.batteries().map_err(|error| error.to_string())?;
        let first_battery = batteries.next();

        match first_battery {
            Some(Ok(battery)) => {
                let charge_ratio = battery.state_of_charge().value;
                let percentage = charge_ratio * 100.0;
                let status = format!("{:?}", battery.state());
                let battery_metrics = BatteryMetrics { percentage, status };
                Ok(Some(battery_metrics))
            }
            Some(Err(error)) => Err(error.to_string()),
            None => Ok(None),
        }
    }
}

#[tauri::command]
pub fn get_system_metrics(state: tauri::State<'_, MonitorState>) -> Result<SystemMetrics, String> {
    state.get_metrics()
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_get_metrics() {
        let monitor = MonitorState::new();
        monitor.get_metrics().expect("Failed to read CPU usage");
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let metrics = monitor.get_metrics().expect("Failed to read metrics");
        let cpu_usage2 = metrics.cpu_usage;
        assert!(0.0 <= cpu_usage2 && cpu_usage2 <= 100.0); // assert!((0.0..=100.0).contains(&cpu_usage2));
        println!("CPU usage: {} %", cpu_usage2);
        let memory_used = metrics.memory_used;
        let memory_total = metrics.memory_total;
        assert!(memory_total > 0);
        assert!(memory_used <= memory_total);
        println!("Used RAM: {} bytes", memory_used);
        println!("Total RAM: {} bytes", memory_total);

        let battery = metrics.battery;
        if let Some(battery) = battery {
            assert!(0.0 <= battery.percentage && battery.percentage <= 100.0);
            println!("Battery: {}%", battery.percentage);
            println!("Battery status: {}", battery.status);
        }
    }
    #[test]
    fn test_read_battery() {
        let monitor = MonitorState::new();
        let battery_info = monitor.read_battery().expect("Failed to read battery");
        if let Some(battery) = battery_info {
            let valid_statuses = ["Charging", "Discharging", "Full", "Empty", "Unknown"];
            assert!(valid_statuses.contains(&battery.status.as_str()));
            assert!(0.0 <= battery.percentage && battery.percentage <= 100.0);
            println!("Battery percentage: {}%", battery.percentage);
            println!("Battery status: {}", battery.status);
        } else {
            println!("No battery detected");
        }
    }
}
