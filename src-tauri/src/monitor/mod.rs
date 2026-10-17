use serde::Serialize;
use std::sync::Mutex;
use sysinfo::System;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]

pub struct SystemMetrics {
    pub cpu_usage: f32,
    pub memory_used: u64,
    pub memory_total: u64,
}

pub struct MonitorState {
    system: Mutex<System>,
}

impl MonitorState {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self {
            system: Mutex::new(sys),
        }
    }

    pub fn read_memory(&self) -> Result<(u64, u64), String> {
        let mut sys = self
            .system
            .lock()
            .map_err(|_| "Failed to acquire system monitor lock".to_string())?; // sys is MutexGuard<System>
        sys.refresh_memory();
        let used = sys.used_memory();
        let total = sys.total_memory();
        Ok((used, total))
    }

    pub fn read_cpu(&self) -> Result<f32, String> {
        let mut sys = self
            .system
            .lock()
            .map_err(|_| "Failed to acquire system monitor lock".to_string())?; // sys is MutexGuard<System>
        sys.refresh_cpu_usage();
        let cpu_usage = sys.global_cpu_usage();
        Ok(cpu_usage)
    }
    pub fn get_metrics(&self) -> Result<SystemMetrics, String> {
        let mut sys = self
            .system
            .lock()
            .map_err(|_| "Failed to acquire system monitor lock".to_string())?; // sys is MutexGuard<System>
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let cpu_usage = sys.global_cpu_usage();
        let used = sys.used_memory();
        let total = sys.total_memory();
        Ok(SystemMetrics {
            cpu_usage: cpu_usage,
            memory_used: used,
            memory_total: total,
        })
    }
}

#[tauri::command]
pub fn get_system_metrics(state: tauri::State<'_, MonitorState>) -> Result<SystemMetrics, String> {
    state.get_metrics()
}

// fn read_memory() -> (u64, u64) {
//     let mut sys = System::new();
//     sys.refresh_memory();
//     (sys.used_memory(), sys.total_memory())
// }

// Tests
#[cfg(test)]
mod tests {
    use super::*;

    // #[test]
    // fn test_read_memory() {
    //     let (used, total) = read_memory();
    //     assert!(total > 0);
    //     assert!(used <= total);
    //     println!("Used RAM: {} bytes", used);
    //     println!("Total RAM: {} bytes", total);
    // }
    #[test]
    fn test_read_memory() {
        let monitor = MonitorState::new();
        let (used, total) = monitor.read_memory().expect("Failed to read memory");
        assert!(total > 0);
        assert!(used <= total);
        println!("Used RAM: {} bytes", used);
        println!("Total RAM: {} bytes", total);
    }
    #[test]
    fn test_read_cpu() {
        let monitor = MonitorState::new();
        // let cpu_usage1 = monitor.read_cpu().expect("Failed to read CPU usage");
        monitor.read_cpu().expect("Failed to read CPU usage");
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let cpu_usage2 = monitor.read_cpu().expect("Failed to read CPU usage");
        assert!(0.0 <= cpu_usage2 && cpu_usage2 <= 100.0); // assert!((0.0..=100.0).contains(&cpu_usage2));
        println!("CPU usage: {} %", cpu_usage2);
    }
    #[test]
    fn test_get_metrics() {
        let monitor = MonitorState::new();
        monitor.get_metrics().expect("Failed to read CPU usage");
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let metrics = monitor.get_metrics().expect("Failed to read metrics");
        let cpu_usage2 = metrics.cpu_usage;
        assert!(0.0 <= cpu_usage2 && cpu_usage2 <= 100.0); // assert!((0.0..=100.0).contains(&cpu_usage2));
        println!("CPU usage: {} %", cpu_usage2);
        let used = metrics.memory_used;
        let total = metrics.memory_total;
        assert!(total > 0);
        assert!(used <= total);
        println!("Used RAM: {} bytes", used);
        println!("Total RAM: {} bytes", total);
    }
}
