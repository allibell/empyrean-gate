// Live: a user-configurable touch control plane. Widgets are arranged in named,
// locally saved responsive decks; performance mode locks the layout while Edit
// mode enables touch dragging, resizing, adding, and removing controls.

import { useEffect, useMemo, useRef, useState } from "react";
import {
  Responsive,
  type ResponsiveLayouts,
  useContainerWidth,
  verticalCompactor,
} from "react-grid-layout";
import {
  addWidgetToDeck,
  cloneControlDeck,
  DECK_BREAKPOINTS,
  DECK_COLUMNS,
  defaultControlDeck,
  loadControlDecks,
  removeWidgetFromDeck,
  saveControlDecks,
  WIDGET_CATALOG,
  widgetLabel,
  type ControlDeck,
  type ControlWidget,
  type ControlWidgetKind,
  type DeckBreakpoint,
} from "./controlDecks";
import { EFFECTS } from "./effects";
import GateCanvas from "./GateCanvas";
import { QuickSettingsEditor, QuickSettingsPanel } from "./DeckQuickSettings";
import Sparkbars from "./Sparkbars";
import { useGate, useThrottled } from "./state";
import ToolIcon, { type ToolKind } from "./ToolIcon";
import type { RenderConfig } from "./types";

const TOOLS: { kind: ToolKind; label: string }[] = [
  { kind: "tap", label: "Tap" },
  { kind: "glow", label: "Glow" },
  { kind: "ripple", label: "Ripple" },
  { kind: "sparkle", label: "Sparkle" },
  { kind: "comet", label: "Comet" },
  { kind: "ring", label: "Ring" },
  { kind: "beam", label: "Beam" },
  { kind: "ember", label: "Ember" },
];

const SWATCHES: { hue: number; label: string }[] = [
  { hue: -1, label: "White" },
  { hue: 0.0, label: "Red" },
  { hue: 0.09, label: "Orange" },
  { hue: 0.16, label: "Gold" },
  { hue: 0.35, label: "Green" },
  { hue: 0.5, label: "Cyan" },
  { hue: 0.62, label: "Blue" },
  { hue: 0.78, label: "Purple" },
  { hue: 0.9, label: "Pink" },
];

function swatchColor(hue: number): string {
  return hue < 0 ? "#ffffff" : `hsl(${hue * 360}deg 90% 60%)`;
}

export default function Live() {
  const { client, config, status, beatAt } = useGate();
  const [tool, setTool] = useState<ToolKind>("tap");
  const [hue, setHue] = useState(0.5);
  const [size, setSize] = useState(0.12);
  const [queuePos, setQueuePos] = useState(0);
  const [decks, setDecks] = useState<ControlDeck[]>(loadControlDecks);
  const [activeDeckId, setActiveDeckId] = useState(
    () => localStorage.getItem("empyrean-active-control-deck") ?? "default",
  );
  const [editing, setEditing] = useState(false);
  const [addKind, setAddKind] = useState<ControlWidgetKind>("status");
  const [brightness, setBrightnessLocal] = useState(1);
  const [masterSpeed, setMasterSpeedLocal] = useState(1);
  const [shortcutEditorId, setShortcutEditorId] = useState<string | null>(null);
  const beatDotRef = useRef<HTMLDivElement>(null);
  const { width: deckWidth, containerRef: deckContainerRef, mounted: deckMounted } =
    useContainerWidth({ initialWidth: window.innerWidth });
  const breakpoint: DeckBreakpoint = deckWidth >= DECK_BREAKPOINTS.desktop
    ? "desktop"
    : deckWidth >= DECK_BREAKPOINTS.tablet
      ? "tablet"
      : "phone";
  const setBrightness = useThrottled((value: number) =>
    client.setMaster({ brightness: value }),
  );
  const setMasterSpeed = useThrottled((value: number) => client.setMaster({ speed: value }));

  const activeDeck = useMemo(
    () => decks.find((deck) => deck.id === activeDeckId) ?? decks[0] ?? defaultControlDeck(),
    [activeDeckId, decks],
  );

  useEffect(() => {
    saveControlDecks(decks);
  }, [decks]);

  useEffect(() => {
    localStorage.setItem("empyrean-active-control-deck", activeDeck.id);
    if (activeDeck.id !== activeDeckId) setActiveDeckId(activeDeck.id);
  }, [activeDeck.id, activeDeckId]);

  useEffect(() => {
    if (!config) return;
    setBrightnessLocal(config.render.master_brightness);
    setMasterSpeedLocal(config.render.master_speed);
  }, [config?.render.master_brightness, config?.render.master_speed]);

  const updateActiveDeck = (update: (deck: ControlDeck) => ControlDeck) => {
    setDecks((current) => current.map((deck) => (deck.id === activeDeck.id ? update(deck) : deck)));
  };

  // Viewer-slot queue: >0 means the preview is rationed and we're waiting.
  useEffect(() => {
    return client.onMessage((m) => {
      if (m.type === "preview_queue") setQueuePos(m.position);
    });
  }, [client]);

  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const dot = beatDotRef.current;
      if (dot) {
        const age = performance.now() - Math.max(...beatAt.current);
        const a = Math.max(0, 1 - age / 300);
        dot.style.opacity = String(0.15 + a * 0.85);
        dot.style.transform = `scale(${1 + a * 0.6})`;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [beatAt]);

  const multiplier =
    config?.render.beat_time === "half" ? 0.5 : config?.render.beat_time === "double" ? 2 : 1;
  const inferredBpm = (status?.audio.find((a) => a.active)?.bpm ?? 0) / multiplier;
  const manualBpm = config?.render.manual_bpm ?? null;
  const bpm = manualBpm !== null ? manualBpm * multiplier : inferredBpm * multiplier;

  const setTempo = (patch: Partial<Pick<RenderConfig, "beat_time" | "manual_bpm">>) => {
    if (!config) return;
    client.setConfig({ ...config, render: { ...config.render, ...patch } });
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (
        e.metaKey ||
        e.ctrlKey ||
        e.altKey ||
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement ||
        e.target instanceof HTMLSelectElement ||
        (e.target instanceof HTMLElement && e.target.isContentEditable)
      ) {
        return;
      }
      const beatTime =
        e.key === "-" || e.code === "NumpadSubtract"
          ? "half"
          : e.key === "+" || e.code === "NumpadAdd"
            ? "double"
            : e.key === "="
              ? "normal"
              : null;
      if (!beatTime) return;
      e.preventDefault();
      setTempo({ beat_time: beatTime });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [config, client]);

  const pens = (
    <div className="cluster pens">
      {TOOLS.map((t) => (
        <button
          key={t.kind}
          className={`pen-btn ${tool === t.kind ? "active" : ""}`}
          onClick={() => setTool(t.kind)}
        >
          <ToolIcon kind={t.kind} />
          {t.label}
        </button>
      ))}
    </div>
  );

  const effects = (
    <div className="cluster effects">
      {EFFECTS.map((e) => (
        <button
          key={e.kind}
          className="effect-btn"
          onClick={() => client.triggerEffect({ kind: e.kind, angle: Math.random() * Math.PI * 2 })}
        >
          {e.label}
          <span className="key-hint">{e.key}</span>
        </button>
      ))}
    </div>
  );

  const colors = (
    <div className="cluster swatches">
      {SWATCHES.map((s) => (
        <button
          key={s.label}
          className={`swatch ${hue === s.hue ? "active" : ""}`}
          style={{ background: swatchColor(s.hue) }}
          onClick={() => setHue(s.hue)}
          aria-label={s.label}
        />
      ))}
    </div>
  );

  const sizeCtl = (
    <div className="cluster size-ctl">
      <label className="slider-row">
        <span>Size</span>
        <input
          type="range"
          min={0.03}
          max={0.4}
          step={0.01}
          value={size}
          onChange={(e) => setSize(Number(e.target.value))}
        />
      </label>
    </div>
  );

  const tempoCtl = config ? (
    <div className="cluster tempo-menu" aria-label="Lighting tempo">
      <div className="tempo-time-grid">
        {([
          { time: "half", label: "Half", key: "−" },
          { time: "normal", label: "1×", key: "=" },
          { time: "double", label: "Double", key: "+" },
        ] as const).map(({ time, label, key }) => (
          <button
            key={time}
            className={`effect-btn ${config.render.beat_time === time ? "active" : ""}`}
            onClick={() => setTempo({ beat_time: time })}
            aria-label={`${label} time`}
          >
            {label}
            <span className="key-hint">{key}</span>
          </button>
        ))}
      </div>
      <div className="tempo-mode-row">
        <button
          className={manualBpm === null ? "active" : ""}
          onClick={() => setTempo({ manual_bpm: null })}
        >
          Auto
        </button>
        <button
          className={manualBpm !== null ? "active" : ""}
          onClick={() =>
            setTempo({ manual_bpm: Math.round(Math.min(240, Math.max(40, inferredBpm || 120))) })
          }
        >
          Manual
        </button>
      </div>
      {manualBpm !== null && (
        <label className="tempo-slider">
          <input
            type="range"
            min={40}
            max={240}
            step={1}
            value={manualBpm}
            onChange={(e) => setTempo({ manual_bpm: Number(e.target.value) })}
          />
          <span>{manualBpm.toFixed(0)}</span>
        </label>
      )}
    </div>
  ) : null;

  const preview = (
    <div className="live-canvas-wrap deck-preview-wrap">
      <GateCanvas
        drawPen={tool === "tap" ? undefined : { pen: tool, hue, size, intensity: 1 }}
        onTap={
          tool === "tap"
            ? (angle, radius) =>
                client.triggerEffect({ kind: "burst", angle, radius, hue, size: size / 0.12 })
            : undefined
        }
      />
      {queuePos > 0 && (
        <div className="queue-banner">
          Live view is full — you're #{queuePos} in line. Taps, drawing, and effects
          still reach the lights!
        </div>
      )}
      <div className="ring-center">
        <div className="ring-title">Empyrean Gate</div>
        <div className="ring-status">
          <div ref={beatDotRef} className="beat-dot" />
          <span>{bpm > 0 ? `${bpm.toFixed(0)} BPM${manualBpm !== null ? " manual" : ""}` : "no beat"}</span>
        </div>
        {status && (
          <Sparkbars
            data={status.fps_history}
            color="#38d1c2"
            label="fps"
            value={String(status.fps_history.at(-1) ?? 0)}
          />
        )}
        {status?.sacn_enabled && (
          <Sparkbars
            data={status.pps_history}
            color="#7c5cff"
            label="pkt/s"
            value={String(status.sacn_pps)}
            warn={status.sacn_pps === 0}
          />
        )}
        {status?.sacn_enabled && <span className="live-pill">sACN LIVE</span>}
      </div>
    </div>
  );

  const master = config ? (
    <div className="deck-master-controls">
      <label className="slider-row">
        <span>Brightness</span>
        <input
          type="range"
          min={0}
          max={1}
          step={0.01}
          value={brightness}
          onChange={(event) => {
            const value = Number(event.target.value);
            setBrightnessLocal(value);
            setBrightness(value);
          }}
        />
        <span className="slider-val">{brightness.toFixed(2)}</span>
      </label>
      <label className="slider-row">
        <span>Speed</span>
        <input
          type="range"
          min={0}
          max={3}
          step={0.01}
          value={masterSpeed}
          onChange={(event) => {
            const value = Number(event.target.value);
            setMasterSpeedLocal(value);
            setMasterSpeed(value);
          }}
        />
        <span className="slider-val">{masterSpeed.toFixed(2)}</span>
      </label>
    </div>
  ) : null;

  const layers = config ? (
    <div className="deck-layer-list">
      {config.layers.map((layer, index) => (
        <button
          key={`${layer.name}-${index}`}
          className={layer.enabled ? "active" : ""}
          onClick={() => client.updateLayer(index, { ...layer, enabled: !layer.enabled })}
        >
          <span className="deck-layer-dot" />
          <span>{layer.name || `Layer ${index + 1}`}</span>
        </button>
      ))}
    </div>
  ) : null;

  const showStatus = (
    <div className="deck-status-grid">
      <div><strong>{bpm > 0 ? bpm.toFixed(0) : "—"}</strong><span>BPM</span></div>
      <div><strong>{status?.engine_fps.toFixed(0) ?? "—"}</strong><span>FPS</span></div>
      <div><strong>{status?.sacn_enabled ? status.sacn_pps : "off"}</strong><span>sACN pkt/s</span></div>
      <div><strong>{status?.clients ?? "—"}</strong><span>clients</span></div>
    </div>
  );

  const widgetContent = (widget: ControlWidget) => {
    switch (widget.kind) {
      case "preview": return preview;
      case "tools": return pens;
      case "effects": return effects;
      case "tempo": return tempoCtl;
      case "colors": return colors;
      case "size": return sizeCtl;
      case "master": return master;
      case "quick_settings": return (
        <QuickSettingsPanel
          shortcuts={widget.shortcuts ?? []}
          onEdit={(id) => setShortcutEditorId(id)}
        />
      );
      case "layers": return layers;
      case "status": return showStatus;
    }
  };

  const availableWidgets = WIDGET_CATALOG.filter(
    (entry) => !activeDeck.widgets.some((widget) => widget.kind === entry.kind),
  );

  useEffect(() => {
    if (availableWidgets.length > 0 && !availableWidgets.some((entry) => entry.kind === addKind)) {
      setAddKind(availableWidgets[0].kind);
    }
  }, [activeDeck.id, activeDeck.widgets.length, addKind, availableWidgets]);

  const duplicateDeck = () => {
    const duplicate = cloneControlDeck(activeDeck);
    setDecks((current) => [...current, duplicate]);
    setActiveDeckId(duplicate.id);
    setEditing(true);
  };

  const deleteDeck = () => {
    if (decks.length <= 1 || !window.confirm(`Delete “${activeDeck.name}”?`)) return;
    const remaining = decks.filter((deck) => deck.id !== activeDeck.id);
    setDecks(remaining);
    setActiveDeckId(remaining[0].id);
  };

  return (
    <div className={`live-page control-deck-page ${editing ? "deck-editing" : ""}`}>
      <div className="control-deck-toolbar">
        <label>
          <span className="visually-hidden">Control deck</span>
          <select value={activeDeck.id} onChange={(event) => setActiveDeckId(event.target.value)}>
            {decks.map((deck) => <option key={deck.id} value={deck.id}>{deck.name}</option>)}
          </select>
        </label>
        <span className="deck-device-chip">{breakpoint}</span>
        <span className="spacer" />
        <button
          className={editing ? "primary" : ""}
          onClick={() => setEditing((value) => !value)}
          aria-pressed={editing}
        >
          {editing ? "✓ Done" : "✦ Edit deck"}
        </button>
      </div>

      {editing && (
        <div className="control-deck-editorbar">
          <input
            aria-label="Deck name"
            value={activeDeck.name}
            onChange={(event) =>
              updateActiveDeck((deck) => ({ ...deck, name: event.target.value }))
            }
          />
          {availableWidgets.length > 0 && (
            <div className="deck-add-control">
              <select value={addKind} onChange={(event) => setAddKind(event.target.value as ControlWidgetKind)}>
                {availableWidgets.map((entry) => (
                  <option key={entry.kind} value={entry.kind}>{entry.label}</option>
                ))}
              </select>
              <button
                onClick={() => {
                  const kind = availableWidgets.some((entry) => entry.kind === addKind)
                    ? addKind
                    : availableWidgets[0].kind;
                  updateActiveDeck((deck) => addWidgetToDeck(deck, kind));
                }}
              >
                + Add
              </button>
            </div>
          )}
          <button onClick={duplicateDeck}>Duplicate</button>
          <button
            onClick={() => updateActiveDeck((deck) => ({
              ...defaultControlDeck(),
              id: deck.id,
              name: deck.name,
            }))}
          >
            Reset
          </button>
          {decks.length > 1 && <button className="danger" onClick={deleteDeck}>Delete</button>}
          <span className="deck-edit-hint">Drag the dotted handle · resize from a corner</span>
        </div>
      )}

      <div className="control-deck-shell" ref={deckContainerRef}>
        {deckMounted && (
          <Responsive<DeckBreakpoint>
            width={deckWidth}
            layouts={activeDeck.layouts}
            breakpoints={DECK_BREAKPOINTS}
            cols={DECK_COLUMNS}
            rowHeight={48}
            margin={{ phone: [8, 8], tablet: [10, 10], desktop: [10, 10] }}
            containerPadding={{ phone: [8, 8], tablet: [10, 10], desktop: [10, 10] }}
            compactor={verticalCompactor}
            dragConfig={{ enabled: editing, bounded: true, handle: ".deck-drag-handle", cancel: "button,input,select" }}
            resizeConfig={{ enabled: editing, handles: ["se", "sw"] }}
            onLayoutChange={(_layout, layouts: ResponsiveLayouts<DeckBreakpoint>) => {
              if (!editing) return;
              updateActiveDeck((deck) =>
                JSON.stringify(deck.layouts) === JSON.stringify(layouts)
                  ? deck
                  : { ...deck, layouts },
              );
            }}
          >
            {activeDeck.widgets.map((widget) => (
              <section key={widget.id} className={`control-widget control-widget-${widget.kind}`}>
                {editing && (
                  <div className="deck-drag-handle">
                    <span aria-hidden="true">⠿</span>
                    <strong>{widgetLabel(widget.kind)}</strong>
                    {widget.kind === "quick_settings" && (
                      <button
                        aria-label="Customize quick settings"
                        onClick={() => setShortcutEditorId("")}
                      >
                        ⚙
                      </button>
                    )}
                    <button
                      aria-label={`Remove ${widgetLabel(widget.kind)}`}
                      onClick={() => updateActiveDeck((deck) => removeWidgetFromDeck(deck, widget.id))}
                    >
                      ×
                    </button>
                  </div>
                )}
                <div className="control-widget-body">{widgetContent(widget)}</div>
              </section>
            ))}
          </Responsive>
        )}
      </div>
      {shortcutEditorId !== null && (() => {
        const widget = activeDeck.widgets.find((entry) => entry.kind === "quick_settings");
        if (!widget) return null;
        return (
          <QuickSettingsEditor
            shortcuts={widget.shortcuts ?? []}
            initialId={shortcutEditorId}
            onClose={() => setShortcutEditorId(null)}
            onChange={(shortcuts) => updateActiveDeck((deck) => ({
              ...deck,
              widgets: deck.widgets.map((entry) => entry.id === widget.id
                ? { ...entry, shortcuts }
                : entry),
            }))}
          />
        );
      })()}
    </div>
  );
}
