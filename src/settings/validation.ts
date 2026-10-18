import type { Settings } from "../types/settings";

export function validateSettings(settings: Settings): Record<string, string> {
  const errors: Record<string, string> = {};
  function integer(name: string, value: number, min: number, max: number) {
    if (!Number.isInteger(value) || value < min || value > max) {
      errors[name] = `Enter a whole number from ${min} to ${max}.`;
    }
  }
  if (![1, 2, 5, 10].includes(settings.monitoringIntervalSeconds)) {
    errors.interval = "Choose 1, 2, 5, or 10 seconds.";
  }
  integer("cpuThreshold", settings.alerts.cpu.thresholdPercent, 1, 100);
  integer("cpuDuration", settings.alerts.cpu.sustainedSeconds, 1, 3600);
  integer("memoryThreshold", settings.alerts.memory.thresholdPercent, 1, 100);
  integer("memoryDuration", settings.alerts.memory.sustainedSeconds, 1, 3600);
  integer("batteryLow", settings.alerts.batteryLow.thresholdPercent, 1, 100);
  integer("batteryCharging", settings.alerts.batteryCharging.thresholdPercent, 1, 100);
  integer("cooldown", settings.alerts.cooldownSeconds, 1, 86400);
  return errors;
}

export function settingsErrorMessage(error: unknown, fallback: string): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error && typeof error.message === "string") {
    return error.message;
  }
  return fallback;
}
