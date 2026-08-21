import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  newQuickSetting,
  patchQuickSetting,
  QUICK_SETTING_TARGETS,
  readQuickSetting,
  targetDefinition,
  type QuickSettingShortcut,
  type QuickSettingTarget,
  type QuickSettingValue,
} from "./quickSettings";
import { useGate } from "./state";

const LONG_PRESS_MS = 650;

function valueLabel(shortcut: QuickSettingShortcut): string {
  const definition = targetDefinition(shortcut.target);
  if (definition.kind === "boolean") return shortcut.value ? "On" : "Off";
  if (definition.kind === "select") {
    return definition.options.find((option) => String(option.value) === String(shortcut.value))?.label
      ?? String(shortcut.value ?? "Auto");
  }
  return Number(shortcut.value).toFixed(definition.step < 0.1 ? 2 : 1);
}

function modeLabel(shortcut: QuickSettingShortcut): string {
  if (shortcut.mode === "hold") return "hold";
  if (shortcut.mode === "timed") return `${shortcut.durationMs / 1000}s`;
  return "set";
}

export function QuickSettingsPanel({
  shortcuts,
  onEdit,
}: {
  shortcuts: QuickSettingShortcut[];
  onEdit: (shortcutId: string) => void;
}) {
  const { client, config } = useGate();
  const configRef = useRef(config);
  const timers = useRef(new Map<string, number>());
  const active = useRef(new Map<string, { shortcut: QuickSettingShortcut; order: number }>());
  const baselines = useRef(new Map<QuickSettingTarget, QuickSettingValue>());
  const actionOrder = useRef(0);
  const longPressTimer = useRef<number | null>(null);
  const longPressed = useRef(false);

  useEffect(() => {
    configRef.current = config;
  }, [config]);

  const write = (target: QuickSettingTarget, value: QuickSettingValue) => {
    const current = configRef.current;
    if (!current) return;
    const next = patchQuickSetting(current, target, value);
    configRef.current = next;
    if (target === "master_brightness") client.setMaster({ brightness: Number(value) });
    else if (target === "master_speed") client.setMaster({ speed: Number(value) });
    else client.setConfig(next);
  };

  const start = (shortcut: QuickSettingShortcut) => {
    const current = configRef.current;
    if (!current) return;
    if (shortcut.mode === "set") {
      write(shortcut.target, shortcut.value);
      return;
    }
    const existingTimer = timers.current.get(shortcut.id);
    if (existingTimer !== undefined) window.clearTimeout(existingTimer);
    if (!baselines.current.has(shortcut.target)) {
      baselines.current.set(shortcut.target, readQuickSetting(current, shortcut.target));
    }
    active.current.set(shortcut.id, { shortcut, order: ++actionOrder.current });
    write(shortcut.target, shortcut.value);
    if (shortcut.mode === "timed") {
      const timer = window.setTimeout(() => restore(shortcut), shortcut.durationMs);
      timers.current.set(shortcut.id, timer);
    }
  };

  const restore = (shortcut: QuickSettingShortcut) => {
    if (!active.current.has(shortcut.id)) return;
    const timer = timers.current.get(shortcut.id);
    if (timer !== undefined) window.clearTimeout(timer);
    timers.current.delete(shortcut.id);
    active.current.delete(shortcut.id);
    const previous = Array.from(active.current.values())
      .filter((entry) => entry.shortcut.target === shortcut.target)
      .sort((a, b) => b.order - a.order)[0];
    if (previous) {
      write(shortcut.target, previous.shortcut.value);
      return;
    }
    const baseline = baselines.current.get(shortcut.target);
    baselines.current.delete(shortcut.target);
    if (baseline !== undefined) write(shortcut.target, baseline);
  };

  useEffect(() => () => {
    if (longPressTimer.current !== null) window.clearTimeout(longPressTimer.current);
    for (const timer of timers.current.values()) window.clearTimeout(timer);
    for (const [target, baseline] of baselines.current) write(target, baseline);
    timers.current.clear();
    active.current.clear();
    baselines.current.clear();
  }, []);

  const pointerDown = (event: ReactPointerEvent<HTMLButtonElement>, shortcut: QuickSettingShortcut) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    longPressed.current = false;
    longPressTimer.current = window.setTimeout(() => {
      longPressed.current = true;
      if (shortcut.mode === "hold") restore(shortcut);
      onEdit(shortcut.id);
    }, LONG_PRESS_MS);
    if (shortcut.mode === "hold") start(shortcut);
  };

  const pointerEnd = (shortcut: QuickSettingShortcut) => {
    if (longPressTimer.current !== null) window.clearTimeout(longPressTimer.current);
    longPressTimer.current = null;
    if (shortcut.mode === "hold") restore(shortcut);
  };

  if (shortcuts.length === 0) {
    return <button className="quick-setting-empty" onClick={() => onEdit("")}>+ Add a shortcut</button>;
  }

  return (
    <div className="quick-settings-grid">
      {shortcuts.map((shortcut) => (
        <button
          key={shortcut.id}
          className="quick-setting-button"
          onPointerDown={(event) => pointerDown(event, shortcut)}
          onPointerUp={() => pointerEnd(shortcut)}
          onPointerCancel={() => pointerEnd(shortcut)}
          onLostPointerCapture={() => pointerEnd(shortcut)}
          onContextMenu={(event) => event.preventDefault()}
          onClick={() => {
            if (longPressed.current) {
              longPressed.current = false;
              return;
            }
            if (shortcut.mode !== "hold") start(shortcut);
          }}
          aria-label={`${shortcut.label}: ${targetDefinition(shortcut.target).label} ${valueLabel(shortcut)}`}
        >
          <strong>{shortcut.label}</strong>
          <span>{valueLabel(shortcut)} · {modeLabel(shortcut)}</span>
        </button>
      ))}
    </div>
  );
}

export function QuickSettingsEditor({
  shortcuts,
  initialId,
  onChange,
  onClose,
}: {
  shortcuts: QuickSettingShortcut[];
  initialId: string;
  onChange: (shortcuts: QuickSettingShortcut[]) => void;
  onClose: () => void;
}) {
  const initial = shortcuts.find((shortcut) => shortcut.id === initialId) ?? newQuickSetting();
  const [draft, setDraft] = useState(initial);
  const exists = shortcuts.some((shortcut) => shortcut.id === draft.id);
  const definition = targetDefinition(draft.target);

  const updateTarget = (target: QuickSettingTarget) => {
    const nextDefinition = targetDefinition(target);
    setDraft({ ...draft, target, value: nextDefinition.defaultValue });
  };

  const save = () => {
    const value = definition.kind === "number"
      ? Math.min(definition.max, Math.max(definition.min, Number(draft.value)))
      : draft.value;
    const normalized = {
      ...draft,
      label: draft.label.trim() || definition.label,
      value,
      durationMs: Math.min(3_600_000, Math.max(100, draft.durationMs)),
    };
    onChange(exists
      ? shortcuts.map((shortcut) => shortcut.id === normalized.id ? normalized : shortcut)
      : [...shortcuts, normalized]);
    onClose();
  };

  return (
    <div className="modal-backdrop quick-settings-backdrop" onClick={onClose}>
      <div
        className="modal quick-settings-editor"
        role="dialog"
        aria-modal="true"
        aria-labelledby="quick-settings-editor-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="quick-settings-editor-title">
          <div>
            <h2 id="quick-settings-editor-title">{exists ? "Edit shortcut" : "New shortcut"}</h2>
            <p className="hint">Long-press any quick button to return here.</p>
          </div>
          <button aria-label="Close quick setting editor" onClick={onClose}>×</button>
        </div>
        <label className="field-row">
          <span>Button label</span>
          <input value={draft.label} onChange={(event) => setDraft({ ...draft, label: event.target.value })} autoFocus />
        </label>
        <label className="field-row">
          <span>Setting</span>
          <select value={draft.target} onChange={(event) => updateTarget(event.target.value as QuickSettingTarget)}>
            {Array.from(new Set(QUICK_SETTING_TARGETS.map((entry) => entry.group))).map((group) => (
              <optgroup key={group} label={group}>
                {QUICK_SETTING_TARGETS.filter((entry) => entry.group === group).map((entry) => (
                  <option key={entry.id} value={entry.id}>{entry.label}</option>
                ))}
              </optgroup>
            ))}
          </select>
        </label>
        <label className="field-row">
          <span>Value</span>
          {definition.kind === "boolean" ? (
            <select value={String(draft.value)} onChange={(event) => setDraft({ ...draft, value: event.target.value === "true" })}>
              <option value="true">On</option><option value="false">Off</option>
            </select>
          ) : definition.kind === "select" ? (
            <select value={String(draft.value)} onChange={(event) => {
              const option = definition.options.find((entry) => String(entry.value) === event.target.value);
              setDraft({ ...draft, value: option?.value ?? null });
            }}>
              {definition.options.map((option) => <option key={String(option.value)} value={String(option.value)}>{option.label}</option>)}
            </select>
          ) : (
            <input type="number" min={definition.min} max={definition.max} step={definition.step} value={Number(draft.value)}
              onChange={(event) => setDraft({ ...draft, value: Number(event.target.value) })} />
          )}
        </label>
        <label className="field-row">
          <span>Action</span>
          <select value={draft.mode} onChange={(event) => setDraft({ ...draft, mode: event.target.value as QuickSettingShortcut["mode"] })}>
            <option value="set">Set and leave</option>
            <option value="hold">While held, then restore</option>
            <option value="timed">Set for a duration, then restore</option>
          </select>
        </label>
        {draft.mode === "timed" && (
          <label className="field-row">
            <span>Duration (seconds)</span>
            <input type="number" min={0.1} max={3600} step={0.1} value={draft.durationMs / 1000}
              onChange={(event) => setDraft({ ...draft, durationMs: Math.max(100, Number(event.target.value) * 1000) })} />
          </label>
        )}
        <div className="quick-settings-editor-actions">
          {exists && (
            <button className="danger" onClick={() => { onChange(shortcuts.filter((shortcut) => shortcut.id !== draft.id)); onClose(); }}>
              Delete
            </button>
          )}
          <span className="spacer" />
          <button onClick={onClose}>Cancel</button>
          <button className="primary" onClick={save}>Save shortcut</button>
        </div>
      </div>
    </div>
  );
}
