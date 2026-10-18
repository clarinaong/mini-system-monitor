import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Settings, SettingsSnapshot } from "../types/settings";
import { settingsErrorMessage } from "../settings/validation";

export function useSettings() {
  const [snapshot, setSnapshot] = useState<SettingsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [attempt, setAttempt] = useState(0);
  const revision = useRef(0);

  const acceptSaved = useCallback((settings: Settings) => {
    revision.current += 1;
    setSnapshot({ settings, loadError: null, notificationDeliveryAvailable: false });
    setLoading(false);
    setError(null);
  }, []);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    let retry: ReturnType<typeof setTimeout> | null = null;
    setLoading(true);

    async function read() {
      const requestedAt = revision.current;
      try {
        const result = await invoke<SettingsSnapshot>("get_settings");
        if (active && requestedAt === revision.current) {
          setSnapshot(result);
          setError(null);
        }
      } catch (failure) {
        if (active && requestedAt === revision.current) {
          setError(settingsErrorMessage(failure, "Unable to load preferences."));
          retry = setTimeout(() => { void read(); }, 2000);
        }
      } finally { if (active) setLoading(false); }
    }

    void listen<Settings>("settings-changed", event => {
      if (active) acceptSaved(event.payload);
    }).then(cleanup => {
      if (!active) { cleanup(); return; }
      unlisten = cleanup;
      void read();
    }).catch(failure => {
      if (active) {
        setError(settingsErrorMessage(failure, "Unable to subscribe to preferences."));
        setLoading(false);
      }
    });

    return () => {
      active = false;
      unlisten?.();
      if (retry !== null) clearTimeout(retry);
    };
  }, [attempt, acceptSaved]);

  return { snapshot, error, loading, acceptSaved, reload: () => setAttempt(value => value + 1) };
}
