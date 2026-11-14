export type Appearance = "system" | "light" | "dark";

export type Settings = {
  version: 1;
  monitoringIntervalSeconds: 1 | 2 | 5 | 10;
  visibleCards: { cpu: boolean; memory: boolean; battery: boolean };
  appearance: Appearance;
  layout: "expanded" | "compact";
  graphs: { cpu: boolean; memory: boolean };
  history: { recordingEnabled: boolean };
  alerts: {
    enabled: boolean; // Master switch; false by default.
    cpu: UsageAlert;
    memory: UsageAlert;
    batteryLow: BatteryAlert;
    batteryCharging: BatteryAlert;
    cooldownSeconds: number;
    notificationSound: boolean;
  };
};

export type UsageAlert = {
  enabled: boolean;
  thresholdPercent: number; 
  sustainedSeconds: number; 
};

export type BatteryAlert = {
  enabled: boolean;
  thresholdPercent: number;
};

export type SettingsError = {
  code: "invalidData" | "unsupportedVersion" | "validation" | "saveFailed" | "unavailable";
  message: string;
};

export type SettingsSnapshot = {
  settings: Settings;
  loadError: SettingsError | null;
  notificationDeliveryAvailable: false;
};
