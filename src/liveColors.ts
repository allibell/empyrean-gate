export interface LiveColor {
  id: string;
  label: string;
  hex: string;
  hue: number;
  saturation: number;
  brightness: number;
}

export const BUILTIN_LIVE_COLORS: readonly LiveColor[] = [
  liveColor("white", "White", "#ffffff"),
  liveColor("red", "Red", "#ff2638"),
  liveColor("orange", "Orange", "#ff861f"),
  liveColor("gold", "Gold", "#ffd52a"),
  liveColor("green", "Green", "#2ee879"),
  liveColor("cyan", "Cyan", "#23e6e0"),
  liveColor("blue", "Blue", "#3979ff"),
  liveColor("purple", "Purple", "#9b58ff"),
  liveColor("pink", "Pink", "#ff4db8"),
];

const STORAGE_KEY = "empyrean-live-custom-colors-v1";
const SELECTED_STORAGE_KEY = "empyrean-live-selected-color-v1";

export function liveColor(id: string, label: string, hex: string): LiveColor {
  const normalized = /^#[0-9a-f]{6}$/i.test(hex) ? hex.toLowerCase() : "#ffffff";
  const r = parseInt(normalized.slice(1, 3), 16) / 255;
  const g = parseInt(normalized.slice(3, 5), 16) / 255;
  const b = parseInt(normalized.slice(5, 7), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  let hue = 0;
  if (delta > 0) {
    if (max === r) hue = ((g - b) / delta) % 6;
    else if (max === g) hue = (b - r) / delta + 2;
    else hue = (r - g) / delta + 4;
    hue = ((hue / 6) + 1) % 1;
  }
  return {
    id,
    label,
    hex: normalized,
    hue,
    saturation: max === 0 ? 0 : delta / max,
    brightness: max,
  };
}

export function loadCustomLiveColors(): LiveColor[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]") as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((entry): entry is { id: string; label: string; hex: string } => {
        if (!entry || typeof entry !== "object") return false;
        const value = entry as Record<string, unknown>;
        return typeof value.id === "string" && typeof value.label === "string"
          && typeof value.hex === "string" && /^#[0-9a-f]{6}$/i.test(value.hex);
      })
      .slice(0, 24)
      .map((entry) => liveColor(entry.id, entry.label, entry.hex));
  } catch {
    return [];
  }
}

export function saveCustomLiveColors(colors: readonly LiveColor[]): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(colors.map(({ id, label, hex }) => ({ id, label, hex }))));
}

export function loadSelectedLiveColor(): LiveColor {
  const hex = localStorage.getItem(SELECTED_STORAGE_KEY);
  if (hex && /^#[0-9a-f]{6}$/i.test(hex)) {
    const saved = [...BUILTIN_LIVE_COLORS, ...loadCustomLiveColors()]
      .find((color) => color.hex === hex.toLowerCase());
    return saved ?? liveColor(`selected-${hex.slice(1)}`, hex.toUpperCase(), hex);
  }
  return BUILTIN_LIVE_COLORS[5];
}

export function saveSelectedLiveColor(color: LiveColor): void {
  localStorage.setItem(SELECTED_STORAGE_KEY, color.hex);
}
