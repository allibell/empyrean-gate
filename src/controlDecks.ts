import type { LayoutItem, ResponsiveLayouts } from "react-grid-layout";
import { defaultQuickSettings, type QuickSettingShortcut } from "./quickSettings";

export type DeckBreakpoint = "phone" | "tablet" | "desktop";

export type ControlWidgetKind =
  | "preview"
  | "tools"
  | "effects"
  | "tempo"
  | "colors"
  | "size"
  | "master"
  | "quick_settings"
  | "layers"
  | "status";

export interface ControlWidget {
  id: string;
  kind: ControlWidgetKind;
  shortcuts?: QuickSettingShortcut[];
}

export interface ControlDeck {
  id: string;
  name: string;
  schemaVersion?: number;
  widgets: ControlWidget[];
  layouts: ResponsiveLayouts<DeckBreakpoint>;
}

// Bump whenever the built-in layout or widget catalog changes so existing
// clients receive the newly merged quick-settings widget and sane dimensions.
const DECK_SCHEMA_VERSION = 9;

export const DECK_BREAKPOINTS: Record<DeckBreakpoint, number> = {
  desktop: 1180,
  tablet: 600,
  phone: 0,
};

export const DECK_COLUMNS: Record<DeckBreakpoint, number> = {
  desktop: 12,
  tablet: 8,
  phone: 4,
};

export const WIDGET_CATALOG: ReadonlyArray<{
  kind: ControlWidgetKind;
  label: string;
  description: string;
}> = [
  { kind: "preview", label: "Gate preview", description: "Interactive live gate view" },
  { kind: "tools", label: "Drawing tools", description: "Tap, glow, ripple, and paint tools" },
  { kind: "effects", label: "Effects", description: "Burst, strobe, swoosh, and collapse" },
  { kind: "tempo", label: "Tempo", description: "Half, normal, double, auto, and manual BPM" },
  { kind: "colors", label: "Colors", description: "Drawing and effect color palette" },
  { kind: "size", label: "Brush size", description: "Drawing and effect size" },
  { kind: "master", label: "Master", description: "Brightness and global speed" },
  { kind: "quick_settings", label: "Quick settings", description: "Custom set, hold, and timed setting buttons" },
  { kind: "layers", label: "Layers", description: "Quick layer enable controls" },
  { kind: "status", label: "Show status", description: "Beat, FPS, output, and connection health" },
];

export function widgetLabel(kind: ControlWidgetKind): string {
  return WIDGET_CATALOG.find((entry) => entry.kind === kind)?.label ?? kind;
}

const STORAGE_KEY = "empyrean-control-decks-v1";

const item = (
  i: ControlWidgetKind,
  x: number,
  y: number,
  w: number,
  h: number,
  minW = 1,
  minH = 1,
): LayoutItem => ({ i, x, y, w, h, minW, minH });

export function defaultControlDeck(): ControlDeck {
  const kinds: ControlWidgetKind[] = [
    "preview",
    "tools",
    "effects",
    "tempo",
    "colors",
    "size",
    "master",
    "quick_settings",
  ];
  return {
    id: "default",
    name: "Live · Balanced",
    schemaVersion: DECK_SCHEMA_VERSION,
    widgets: kinds.map((kind) => kind === "quick_settings"
      ? { id: kind, kind, shortcuts: defaultQuickSettings() }
      : { id: kind, kind }),
    layouts: {
      phone: [
        item("preview", 0, 0, 4, 7, 2, 4),
        item("tools", 0, 7, 4, 3, 2, 2),
        item("effects", 0, 10, 4, 2, 2, 1),
        item("tempo", 0, 12, 4, 3, 2, 2),
        item("colors", 0, 15, 4, 1, 2, 1),
        item("size", 0, 16, 4, 1, 2, 1),
        item("master", 0, 17, 4, 2, 2, 2),
        item("quick_settings", 0, 19, 4, 2, 2, 2),
      ],
      tablet: [
        item("preview", 0, 0, 6, 10, 3, 5),
        item("tools", 6, 0, 2, 5, 2, 2),
        item("effects", 6, 5, 2, 2, 2, 1),
        item("tempo", 6, 7, 2, 3, 2, 2),
        item("colors", 0, 10, 6, 1, 3, 1),
        item("size", 6, 10, 2, 1, 2, 1),
        item("master", 0, 11, 8, 2, 2, 2),
        item("quick_settings", 0, 13, 8, 2, 2, 2),
      ],
      desktop: [
        item("tools", 0, 4, 2, 5, 2, 2),
        item("effects", 0, 9, 2, 2, 2, 1),
        item("preview", 2, 0, 8, 16, 6, 8),
        item("colors", 10, 3, 2, 3, 2, 1),
        item("size", 10, 6, 2, 1, 2, 1),
        item("tempo", 10, 7, 2, 3, 2, 2),
        item("master", 10, 10, 2, 2, 2, 2),
        item("quick_settings", 0, 0, 2, 2, 2, 2),
      ],
    },
  };
}

function isDeck(value: unknown): value is ControlDeck {
  if (!value || typeof value !== "object") return false;
  const deck = value as Partial<ControlDeck>;
  return (
    typeof deck.id === "string" &&
    typeof deck.name === "string" &&
    Array.isArray(deck.widgets) &&
    !!deck.layouts &&
    typeof deck.layouts === "object"
  );
}

export function loadControlDecks(): ControlDeck[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as unknown;
    if (Array.isArray(parsed)) {
      const decks = parsed.filter(isDeck);
      if (decks.length > 0) {
        const freshDefault = defaultControlDeck();
        return decks.map((deck) => {
          if (deck.id !== "default" || deck.schemaVersion === DECK_SCHEMA_VERSION) return deck;
          const upgraded = {
            ...deck,
            schemaVersion: DECK_SCHEMA_VERSION,
            layouts: { ...deck.layouts, desktop: freshDefault.layouts.desktop },
          };
          return upgraded.widgets.some((widget) => widget.kind === "quick_settings")
            ? upgraded
            : addWidgetToDeck(upgraded, "quick_settings");
        });
      }
    }
  } catch {
    // A malformed local edit should never keep the performance surface from loading.
  }
  return [defaultControlDeck()];
}

export function saveControlDecks(decks: ControlDeck[]): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(decks));
}

export function cloneControlDeck(deck: ControlDeck): ControlDeck {
  const suffix = Math.random().toString(36).slice(2, 8);
  return {
    ...structuredClone(deck),
    id: `deck-${suffix}`,
    name: `${deck.name} copy`,
  };
}

const DEFAULT_SIZE: Record<ControlWidgetKind, Record<DeckBreakpoint, [number, number]>> = {
  preview: { phone: [4, 7], tablet: [6, 10], desktop: [8, 12] },
  tools: { phone: [4, 3], tablet: [4, 3], desktop: [3, 5] },
  effects: { phone: [4, 2], tablet: [4, 2], desktop: [3, 3] },
  tempo: { phone: [4, 3], tablet: [4, 3], desktop: [3, 4] },
  colors: { phone: [4, 1], tablet: [4, 2], desktop: [3, 3] },
  size: { phone: [4, 1], tablet: [4, 1], desktop: [3, 2] },
  master: { phone: [4, 2], tablet: [4, 2], desktop: [3, 3] },
  quick_settings: { phone: [4, 2], tablet: [4, 2], desktop: [3, 3] },
  layers: { phone: [4, 3], tablet: [4, 3], desktop: [3, 4] },
  status: { phone: [4, 2], tablet: [4, 2], desktop: [3, 3] },
};

function bottom(layout: readonly LayoutItem[]): number {
  return layout.reduce((max, entry) => Math.max(max, entry.y + entry.h), 0);
}

export function addWidgetToDeck(deck: ControlDeck, kind: ControlWidgetKind): ControlDeck {
  if (deck.widgets.some((widget) => widget.kind === kind)) return deck;
  const id = kind;
  const next = structuredClone(deck);
  next.widgets.push(kind === "quick_settings"
    ? { id, kind, shortcuts: defaultQuickSettings() }
    : { id, kind });
  for (const breakpoint of Object.keys(DECK_COLUMNS) as DeckBreakpoint[]) {
    const layout = [...(next.layouts[breakpoint] ?? [])];
    if (layout.some((entry) => entry.i === id)) continue;
    const [defaultW, defaultH] = DEFAULT_SIZE[kind][breakpoint];
    const w = Math.min(defaultW, DECK_COLUMNS[breakpoint]);
    layout.push(item(id as ControlWidgetKind, 0, bottom(layout), w, defaultH));
    next.layouts[breakpoint] = layout;
  }
  return next;
}

export function removeWidgetFromDeck(deck: ControlDeck, widgetId: string): ControlDeck {
  const next = structuredClone(deck);
  next.widgets = next.widgets.filter((widget) => widget.id !== widgetId);
  for (const breakpoint of Object.keys(DECK_COLUMNS) as DeckBreakpoint[]) {
    next.layouts[breakpoint] = (next.layouts[breakpoint] ?? []).filter(
      (entry) => entry.i !== widgetId,
    );
  }
  return next;
}
