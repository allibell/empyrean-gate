import type { AppConfig } from "./types";

export type QuickSettingTarget =
  | "master_brightness"
  | "master_speed"
  | "manual_bpm"
  | "beat_time"
  | "walk_enabled"
  | "walk_layers"
  | "walk_speed"
  | "walk_depth"
  | "output_enabled"
  | "beat_taps_enabled";

export type QuickSettingValue = number | boolean | string | null;
export type QuickSettingMode = "set" | "hold" | "timed";

export interface QuickSettingShortcut {
  id: string;
  label: string;
  target: QuickSettingTarget;
  value: QuickSettingValue;
  mode: QuickSettingMode;
  durationMs: number;
}

type NumberTarget = { kind: "number"; min: number; max: number; step: number; defaultValue: number };
type BooleanTarget = { kind: "boolean"; defaultValue: boolean };
type SelectTarget = { kind: "select"; options: ReadonlyArray<{ value: string | null; label: string }>; defaultValue: string | null };

export type QuickSettingTargetDefinition = {
  id: QuickSettingTarget;
  label: string;
  group: string;
} & (NumberTarget | BooleanTarget | SelectTarget);

export const QUICK_SETTING_TARGETS: readonly QuickSettingTargetDefinition[] = [
  { id: "master_brightness", label: "Master brightness", group: "Master", kind: "number", min: 0, max: 1, step: 0.01, defaultValue: 0 },
  { id: "master_speed", label: "Master speed", group: "Master", kind: "number", min: 0, max: 3, step: 0.05, defaultValue: 1 },
  { id: "manual_bpm", label: "Tempo source / BPM", group: "Tempo", kind: "select", defaultValue: null, options: [
    { value: null, label: "Auto" }, { value: "80", label: "80 BPM" }, { value: "100", label: "100 BPM" },
    { value: "120", label: "120 BPM" }, { value: "128", label: "128 BPM" }, { value: "140", label: "140 BPM" },
  ] },
  { id: "beat_time", label: "Beat time", group: "Tempo", kind: "select", defaultValue: "normal", options: [
    { value: "half", label: "Half" }, { value: "normal", label: "Normal" }, { value: "double", label: "Double" },
  ] },
  { id: "walk_enabled", label: "Autopilot", group: "Autopilot", kind: "boolean", defaultValue: true },
  { id: "walk_layers", label: "Walk active layers", group: "Autopilot", kind: "boolean", defaultValue: true },
  { id: "walk_speed", label: "Walk speed", group: "Autopilot", kind: "number", min: 0.1, max: 5, step: 0.1, defaultValue: 1 },
  { id: "walk_depth", label: "Walk depth", group: "Autopilot", kind: "number", min: 0, max: 3, step: 0.1, defaultValue: 1 },
  { id: "output_enabled", label: "sACN output", group: "Output", kind: "boolean", defaultValue: true },
  { id: "beat_taps_enabled", label: "Beat taps", group: "Effects", kind: "boolean", defaultValue: true },
];

export function defaultQuickSettings(): QuickSettingShortcut[] {
  return [{
    id: "blackout",
    label: "Blackout",
    target: "master_brightness",
    value: 0,
    mode: "hold",
    durationMs: 1000,
  }];
}

export function newQuickSetting(): QuickSettingShortcut {
  return {
    id: `shortcut-${Math.random().toString(36).slice(2, 8)}`,
    label: "New shortcut",
    target: "master_brightness",
    value: 0,
    mode: "set",
    durationMs: 1000,
  };
}

export function targetDefinition(target: QuickSettingTarget): QuickSettingTargetDefinition {
  return QUICK_SETTING_TARGETS.find((entry) => entry.id === target) ?? QUICK_SETTING_TARGETS[0];
}

export function readQuickSetting(config: AppConfig, target: QuickSettingTarget): QuickSettingValue {
  switch (target) {
    case "master_brightness": return config.render.master_brightness;
    case "master_speed": return config.render.master_speed;
    case "manual_bpm": return config.render.manual_bpm;
    case "beat_time": return config.render.beat_time;
    case "walk_enabled": return config.render.walk_enabled;
    case "walk_layers": return config.render.walk_layers;
    case "walk_speed": return config.render.walk_speed;
    case "walk_depth": return config.render.walk_depth;
    case "output_enabled": return config.output.enabled;
    case "beat_taps_enabled": return config.beat_taps.enabled;
  }
}

export function patchQuickSetting(
  config: AppConfig,
  target: QuickSettingTarget,
  value: QuickSettingValue,
): AppConfig {
  const next = structuredClone(config);
  switch (target) {
    case "master_brightness": next.render.master_brightness = Number(value); break;
    case "master_speed": next.render.master_speed = Number(value); break;
    case "manual_bpm": next.render.manual_bpm = value === null ? null : Number(value); break;
    case "beat_time": next.render.beat_time = value as AppConfig["render"]["beat_time"]; break;
    case "walk_enabled": next.render.walk_enabled = Boolean(value); break;
    case "walk_layers": next.render.walk_layers = Boolean(value); break;
    case "walk_speed": next.render.walk_speed = Number(value); break;
    case "walk_depth": next.render.walk_depth = Number(value); break;
    case "output_enabled": next.output.enabled = Boolean(value); break;
    case "beat_taps_enabled": next.beat_taps.enabled = Boolean(value); break;
  }
  return next;
}
