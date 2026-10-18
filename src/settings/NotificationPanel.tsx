import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { settingsErrorMessage } from "./validation";

export type NotificationStatus = {
  permission: "notDetermined" | "denied" | "authorized" | "provisional" | "unavailable" | "unknown";
  canRequest: boolean;
  canSendTest: boolean;
};

const descriptions: Record<NotificationStatus["permission"], string> = {
  notDetermined: "Permission has not been requested. Only the button below asks macOS for permission.",
  denied: "Notifications are denied. Review System Settings → Notifications → Mini System Monitor, then refresh status.",
  authorized: "macOS has authorized notifications. Focus and notification settings may still suppress banners.",
  provisional: "macOS allows quiet notifications. Banners may not appear.",
  unavailable: "Permission checks and test notifications require a packaged macOS release app. They are unavailable in development runs.",
  unknown: "macOS returned an unrecognized permission state. Notification testing is disabled.",
};

export default function NotificationPanel({ disabled }: { disabled: boolean }) {
  const [status, setStatus] = useState<NotificationStatus | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const generation = useRef(0);
  const locked = useRef(false);

  async function perform(command: string, initial = false) {
    if (locked.current && !initial) return;
    locked.current = true;
    const request = ++generation.current;
    setBusy(true);
    setError(null);
    setFeedback(null);
    try {
      const next = await invoke<NotificationStatus>(command);
      if (request !== generation.current) return;
      setStatus(next);
      if (command === "send_test_notification") {
        setFeedback("Test notification submitted. Check Notification Center; submission does not confirm display.");
      }
    } catch (failure) {
      if (request === generation.current) {
        setError(settingsErrorMessage(failure, "The notification operation could not complete. Try refreshing status."));
      }
    } finally {
      if (request === generation.current) { locked.current = false; setBusy(false); }
    }
  }

  useEffect(() => {
    void perform("get_notification_status", true);
    return () => { generation.current += 1; };
  }, []);

  return <section className="settings-note" aria-label="Notification permission">
    <strong>macOS notification permission</strong>
    <p>{busy ? "Checking or updating notification permission…" : status ? descriptions[status.permission] : "Notification status is unavailable."}</p>
    <p>Permission is separate from saved alert preferences. Enable and apply alert rules to receive automatic notifications in the packaged app. The test notification is always silent.</p>
    <p>These controls take effect immediately. Apply and Cancel affect preferences only. For alert sounds, also enable sounds in macOS notification settings.</p>
    {error && <p className="field-error" role="alert">{error}</p>}
    {feedback && <p role="status">{feedback}</p>}
    <div className="notification-actions">
      <button type="button" disabled={disabled || busy} onClick={() => void perform("get_notification_status")}>Refresh notification status</button>
      <button type="button" disabled={disabled || busy || !status?.canRequest} onClick={() => void perform("request_notification_permission")}>Request notification permission</button>
      <button type="button" disabled={disabled || busy || !status?.canSendTest} onClick={() => void perform("send_test_notification")}>Send test notification</button>
    </div>
  </section>;
}
