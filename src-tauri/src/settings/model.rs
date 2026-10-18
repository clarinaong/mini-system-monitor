use serde::{Deserialize, Serialize};

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub version: u32,
    pub monitoring_interval_seconds: u8,
    pub visible_cards: VisibleCards,
    pub layout: Layout,
    pub graphs: GraphSettings,
    pub alerts: AlertSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            monitoring_interval_seconds: 1,
            visible_cards: VisibleCards::default(),
            layout: Layout::Expanded,
            graphs: GraphSettings::default(),
            alerts: AlertSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct VisibleCards {
    pub cpu: bool,
    pub memory: bool,
    pub battery: bool,
}

impl Default for VisibleCards {
    fn default() -> Self {
        Self {
            cpu: true,
            memory: true,
            battery: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    Expanded,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphSettings {
    pub cpu: bool,
    pub memory: bool,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            cpu: true,
            memory: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AlertSettings {
    /// Master switch; individual rule switches take effect only when enabled.
    pub enabled: bool,
    pub cpu: UsageAlert,
    pub memory: UsageAlert,
    pub battery_low: BatteryLowAlert,
    pub battery_charging: BatteryChargingAlert,
    pub cooldown_seconds: u32,
    pub notification_sound: bool,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cpu: UsageAlert::default(),
            memory: UsageAlert::default(),
            battery_low: BatteryLowAlert::default(),
            battery_charging: BatteryChargingAlert::default(),
            cooldown_seconds: 600,
            notification_sound: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageAlert {
    pub enabled: bool,
    pub threshold_percent: u8,
    pub sustained_seconds: u32,
}

impl Default for UsageAlert {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_percent: 90,
            sustained_seconds: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BatteryLowAlert {
    pub enabled: bool,
    pub threshold_percent: u8,
}

impl Default for BatteryLowAlert {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_percent: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BatteryChargingAlert {
    pub enabled: bool,
    pub threshold_percent: u8,
}

impl Default for BatteryChargingAlert {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_percent: 80,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), super::SettingsError> {
        use super::{SettingsError, SettingsErrorCode};

        if self.version != SETTINGS_VERSION {
            return Err(SettingsError::new(
                SettingsErrorCode::UnsupportedVersion,
                format!(
                    "Settings version {} is unsupported; this app supports version {SETTINGS_VERSION}.",
                    self.version
                ),
            ));
        }
        let invalid = |message| Err(SettingsError::new(SettingsErrorCode::Validation, message));
        if ![1, 2, 5, 10].contains(&self.monitoring_interval_seconds) {
            return invalid("Monitoring interval must be 1, 2, 5, or 10 seconds.");
        }
        for rule in [&self.alerts.cpu, &self.alerts.memory] {
            if !(1..=100).contains(&rule.threshold_percent) {
                return invalid("CPU and memory thresholds must be between 1 and 100 percent.");
            }
            if !(1..=3600).contains(&rule.sustained_seconds) {
                return invalid("Sustained duration must be between 1 and 3600 seconds.");
            }
        }
        if !(1..=100).contains(&self.alerts.battery_low.threshold_percent)
            || !(1..=100).contains(&self.alerts.battery_charging.threshold_percent)
        {
            return invalid("Battery thresholds must be between 1 and 100 percent.");
        }
        if !(1..=86400).contains(&self.alerts.cooldown_seconds) {
            return invalid("Per-rule cooldown must be between 1 and 86400 seconds.");
        }
        Ok(())
    }
}
