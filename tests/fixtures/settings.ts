import type { Settings, SettingsSnapshot } from "../../src/types/settings";

export function settingsFixture(): Settings {
  return {
    version: 1,
    monitoringIntervalSeconds: 1,
    visibleCards: { cpu: true, memory: true, battery: true },
    layout: "expanded",
    graphs: { cpu: true, memory: true },
    history: { recordingEnabled: false },
    alerts: {
      enabled: false,
      cpu: { enabled: true, thresholdPercent: 90, sustainedSeconds: 30 },
      memory: { enabled: true, thresholdPercent: 90, sustainedSeconds: 30 },
      batteryLow: { enabled: true, thresholdPercent: 20 },
      batteryCharging: { enabled: true, thresholdPercent: 80 },
      cooldownSeconds: 600,
      notificationSound: false,
    },
  };
}

export function settingsSnapshot(): SettingsSnapshot {
  return { settings: settingsFixture(), loadError: null, notificationDeliveryAvailable: false };
}
