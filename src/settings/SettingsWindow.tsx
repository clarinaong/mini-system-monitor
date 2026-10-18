import { useEffect, useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useSettings } from "../hooks/useSettings";
import type { Settings, SettingsSnapshot } from "../types/settings";
import { settingsErrorMessage, validateSettings } from "./validation";
import "./settings.css";
import NotificationPanel from "./NotificationPanel";

type CheckProps = { label: string; checked: boolean; onChange: (checked: boolean) => void };

function Check({ label, checked, onChange }: CheckProps) {
  return <label className="settings-check"><input type="checkbox" checked={checked}
    onChange={event => onChange(event.target.checked)} /><span>{label}</span></label>;
}

type NumberProps = {
  name: string; label: string; value: number; min: number; max: number;
  error?: string; onChange: (value: number) => void;
};

function NumberField({ name, label, value, min, max, error, onChange }: NumberProps) {
  return <div className="settings-number">
    <label htmlFor={name}>{label}</label>
    <input id={name} type="number" min={min} max={max} step="1"
      value={Number.isNaN(value) ? "" : value}
      aria-invalid={Boolean(error)} aria-describedby={error ? `${name}-error` : undefined}
      onChange={event => onChange(event.target.valueAsNumber)} />
    {error && <span className="field-error" id={`${name}-error`}>{error}</span>}
  </div>;
}

type FormProps = {
  snapshot: SettingsSnapshot; loading: boolean;
  reload: () => void; acceptSaved: (settings: Settings) => void;
};

function SettingsForm({ snapshot, loading, reload, acceptSaved }: FormProps) {
  const [draft, setDraft] = useState(() => structuredClone(snapshot.settings));
  const [baseline, setBaseline] = useState(snapshot.settings);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [tried, setTried] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const dirty = JSON.stringify(draft) !== JSON.stringify(baseline);
  const errors = validateSettings(draft);
  const changedElsewhere = JSON.stringify(snapshot.settings) !== JSON.stringify(baseline);

  useEffect(() => {
    if (!dirty) { setDraft(structuredClone(snapshot.settings)); setBaseline(snapshot.settings); }
  }, [snapshot.settings, dirty]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    void getCurrentWindow().onCloseRequested(event => {
      if (savingRef.current) event.preventDefault();
    }).then(cleanup => {
      if (!active) cleanup(); else unlisten = cleanup;
    }).catch(failure => {
      if (active) setActionError(settingsErrorMessage(failure, "Unable to listen for window closing."));
    });
    return () => { active = false; unlisten?.(); };
  }, []);

  function change(update: (settings: Settings) => void) {
    setDraft(previous => {
      const next = structuredClone(previous);
      update(next);
      return next;
    });
    setSaved(false);
    setActionError(null);
  }

  async function apply(event: FormEvent) {
    event.preventDefault();
    setTried(true);
    if (Object.keys(errors).length || savingRef.current || snapshot.loadError || !dirty) return;
    savingRef.current = true;
    setSaving(true);
    setActionError(null);
    setSaved(false);
    try {
      const result = await invoke<Settings>("save_settings", { settings: draft });
      setDraft(structuredClone(result));
      setBaseline(result);
      acceptSaved(result);
      setSaved(true);
      setTried(false);
    } catch (failure) {
      setActionError(settingsErrorMessage(failure, "Preferences could not be saved. Your edits are still here."));
    } finally { savingRef.current = false; setSaving(false); }
  }

  async function cancel() {
    if (savingRef.current) return;
    try { await getCurrentWindow().close(); }
    catch (failure) { setActionError(settingsErrorMessage(failure, "Unable to close Settings.")); }
  }

  const errorFor = (name: string) => tried ? errors[name] : undefined;
  const field = (name: string, label: string, value: number, min: number, max: number,
    update: (settings: Settings, value: number) => void) => (
    <NumberField name={name} label={label} value={value} min={min} max={max} error={errorFor(name)}
      onChange={value => change(next => update(next, value))} />
  );

  return <form onSubmit={apply} noValidate>
    <div className="settings-scroll">
      {snapshot.loadError && <div className="settings-error" role="alert">
        <strong>Saved preferences could not be loaded.</strong>
        <p>{snapshot.loadError.message}</p>
        <button type="button" onClick={reload} disabled={loading || saving}>Retry loading</button>
      </div>}
      {changedElsewhere && dirty && <p className="settings-note" role="status">
        Preferences changed elsewhere. Your unsaved edits have been kept.
      </p>}
      <fieldset disabled={saving}>
        <legend>Monitoring</legend>
        <label className="settings-row" htmlFor="interval">
          <span>Refresh interval</span>
          <select id="interval" value={draft.monitoringIntervalSeconds}
            onChange={event => change(next => { next.monitoringIntervalSeconds = Number(event.target.value) as Settings["monitoringIntervalSeconds"]; })}>
            {[1, 2, 5, 10].map(seconds => <option key={seconds} value={seconds}>{seconds} second{seconds === 1 ? "" : "s"}</option>)}
          </select>
        </label>
        <p className="settings-help">Monitoring continues while the widget is hidden.</p>
      </fieldset>

      <fieldset disabled={saving}>
        <legend>Widget</legend>
        <div className="settings-options">
          <Check label="Show CPU" checked={draft.visibleCards.cpu} onChange={value => change(next => { next.visibleCards.cpu = value; })} />
          <Check label="Show memory" checked={draft.visibleCards.memory} onChange={value => change(next => { next.visibleCards.memory = value; })} />
          <Check label="Show battery" checked={draft.visibleCards.battery} onChange={value => change(next => { next.visibleCards.battery = value; })} />
        </div>
        <div className="settings-options">
          <Check label="Show CPU graph" checked={draft.graphs.cpu} onChange={value => change(next => { next.graphs.cpu = value; })} />
          <Check label="Show memory graph" checked={draft.graphs.memory} onChange={value => change(next => { next.graphs.memory = value; })} />
        </div>
        <label className="settings-row" htmlFor="layout"><span>Layout</span>
          <select id="layout" value={draft.layout} onChange={event => change(next => { next.layout = event.target.value as Settings["layout"]; })}>
            <option value="expanded">Expanded</option><option value="compact">Compact</option>
          </select>
        </label>
        <p className="settings-help">Card visibility affects the widget only. Graph preferences are kept when a card is hidden.</p>
      </fieldset>

      <section className="settings-note" aria-label="Alert availability">
        <strong>Automatic alert delivery is not available yet</strong>
        <p>You can save alert preferences now. Automatic notifications and sounds are not available yet.</p>
      </section>
      <NotificationPanel disabled={saving} />
      <fieldset disabled={saving}>
        <legend>Alert preferences</legend>
        <Check label="Enable alerts when available" checked={draft.alerts.enabled} onChange={value => change(next => { next.alerts.enabled = value; })} />
        <div className="settings-rule">
          <Check label="CPU usage alert" checked={draft.alerts.cpu.enabled} onChange={value => change(next => { next.alerts.cpu.enabled = value; })} />
          <div className="settings-numbers">
            {field("cpuThreshold", "CPU threshold (%)", draft.alerts.cpu.thresholdPercent, 1, 100, (next, value) => { next.alerts.cpu.thresholdPercent = value; })}
            {field("cpuDuration", "CPU sustained seconds", draft.alerts.cpu.sustainedSeconds, 1, 3600, (next, value) => { next.alerts.cpu.sustainedSeconds = value; })}
          </div>
        </div>
        <div className="settings-rule">
          <Check label="Memory usage alert" checked={draft.alerts.memory.enabled} onChange={value => change(next => { next.alerts.memory.enabled = value; })} />
          <div className="settings-numbers">
            {field("memoryThreshold", "Memory threshold (%)", draft.alerts.memory.thresholdPercent, 1, 100, (next, value) => { next.alerts.memory.thresholdPercent = value; })}
            {field("memoryDuration", "Memory sustained seconds", draft.alerts.memory.sustainedSeconds, 1, 3600, (next, value) => { next.alerts.memory.sustainedSeconds = value; })}
          </div>
        </div>
        <div className="settings-rule">
          <Check label="Low battery reminder" checked={draft.alerts.batteryLow.enabled} onChange={value => change(next => { next.alerts.batteryLow.enabled = value; })} />
          {field("batteryLow", "Low battery threshold (%)", draft.alerts.batteryLow.thresholdPercent, 1, 100, (next, value) => { next.alerts.batteryLow.thresholdPercent = value; })}
        </div>
        <div className="settings-rule">
          <Check label="Charging reminder" checked={draft.alerts.batteryCharging.enabled} onChange={value => change(next => { next.alerts.batteryCharging.enabled = value; })} />
          {field("batteryCharging", "Charging reminder threshold (%)", draft.alerts.batteryCharging.thresholdPercent, 1, 100, (next, value) => { next.alerts.batteryCharging.thresholdPercent = value; })}
        </div>
        {field("cooldown", "Cooldown seconds per rule", draft.alerts.cooldownSeconds, 1, 86400, (next, value) => { next.alerts.cooldownSeconds = value; })}
        <Check label="Notification sound when available" checked={draft.alerts.notificationSound} onChange={value => change(next => { next.alerts.notificationSound = value; })} />
      </fieldset>
    </div>
    <footer className="settings-footer">
      <div className="settings-feedback" aria-live="polite">
        {actionError && <p className="field-error" role="alert">{actionError}</p>}
        {tried && Object.keys(errors).length > 0 && <p className="field-error" role="alert">Correct the highlighted values before applying.</p>}
        {saved && <p role="status">Preferences saved.</p>}
        {saving && <p role="status">Saving preferences…</p>}
      </div>
      <div className="settings-actions">
        <button type="button" onClick={() => { void cancel(); }} disabled={saving}>Cancel</button>
        <button className="primary" type="submit" disabled={!dirty || saving || loading || Boolean(snapshot.loadError)}>
          {saving ? "Saving…" : "Apply"}
        </button>
      </div>
    </footer>
  </form>;
}

export default function SettingsWindow() {
  const { snapshot, error, loading, reload, acceptSaved } = useSettings();
  return <main className="settings-page">
    <header className="settings-header"><h1>Settings</h1><p>Make the widget work for you.</p></header>
    {error && <div className="settings-error" role="alert"><p>{error}</p>
      <button type="button" onClick={reload}>Retry</button></div>}
    {snapshot ? <SettingsForm snapshot={snapshot} loading={loading} reload={reload} acceptSaved={acceptSaved} />
      : loading ? <p className="settings-loading" role="status">Loading preferences…</p> : null}
  </main>;
}
