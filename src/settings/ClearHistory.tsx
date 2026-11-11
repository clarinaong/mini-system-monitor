import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { settingsErrorMessage } from "./validation";

export default function ClearHistory({ recordingEnabled }: { recordingEnabled: boolean }) {
  const [confirming, setConfirming] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [cleared, setCleared] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);

  async function clear() {
    if (pending.current) return;
    pending.current = true;
    setClearing(true);
    setError(null);
    try {
      await invoke<void>("clear_history");
      setCleared(true);
      setConfirming(false);
    } catch (failure) {
      setError(settingsErrorMessage(failure, "Could not clear performance history."));
    } finally {
      pending.current = false;
      setClearing(false);
    }
  }

  return <div className="settings-rule">
    {confirming ? <>
      <p>Delete all saved performance readings now? This cannot be undone.
        {recordingEnabled && " Recording will continue with new readings."}</p>
      <div className="notification-actions">
        <button type="button" onClick={() => { void clear(); }} disabled={clearing}>
          {clearing ? "Clearing…" : "Delete saved readings"}
        </button>
        <button type="button" disabled={clearing} onClick={() => {
          setConfirming(false);
          setError(null);
        }}>Keep history</button>
      </div>
    </> : <button type="button" onClick={() => {
      setConfirming(true);
      setCleared(false);
    }}>Clear history…</button>}
    {cleared && <p role="status">Performance history cleared.</p>}
    {error && <p className="field-error" role="alert">{error}</p>}
  </div>;
}
