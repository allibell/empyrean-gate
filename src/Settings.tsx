// Settings: layer stack editor, geometry, audio sources, sACN output, and
// this-device remote inputs (mic / IMU).

import { useEffect, useRef, useState } from "react";
import { useGate, useThrottled } from "./state";
import { startImu, startMic } from "./sensors";
import {
  BLEND_MODES,
  LAYER_KINDS,
  LAYER_LABELS,
  PARAM_LABELS,
  defaultLayer,
  type AppConfig,
  type AudioSourceConfig,
  type LayerCfg,
  type LayerKind,
} from "./types";

export default function Settings() {
  const { config } = useGate();
  if (!config) return <p className="hint">Waiting for backend…</p>;
  return (
    <div className="settings-page">
      <LayersPanel config={config} />
      <AudioPanel config={config} />
      <RhythmPanel config={config} />
      <OutputPanel config={config} />
      <GeometryPanel config={config} />
      <ClientsPanel />
      <DiagnosticsPanel />
      <UpdatesPanel config={config} />
      <ThisDevicePanel />
    </div>
  );
}

function DiagnosticsPanel() {
  const { client, config, status } = useGate();
  const [note, setNote] = useState("");
  const path = status?.diagnostics_path ?? "";
  return (
    <section className="panel">
      <h2>Diagnostics</h2>
      <p className="hint">
        Recent app logs are kept on the show machine in a bounded four-file set
        (about 4 MB total). Downloads are capped at 2 MB and redact join credentials.
      </p>
      <p className={status?.diagnostics_active ? "ok" : "warn"}>
        {status?.diagnostics_active ? "Persistent logging active" : "Persistent logging unavailable"}
        {status?.diagnostics_error ? ` — ${status.diagnostics_error}` : ""}
      </p>
      {path && <p className="diagnostics-path" title={path}>{path}</p>}
      <div className="add-row">
        <button
          disabled={!path}
          onClick={() => void navigator.clipboard.writeText(path)
            .then(() => setNote("Log path copied."))
            .catch(() => setNote("Could not copy the path; select it above."))}
        >Copy log path</button>
        <button
          disabled={!status?.diagnostics_active}
          onClick={() => void client.downloadDiagnostics(config?.server.join_token ?? "")
            .then(() => setNote("Diagnostics download started."))
            .catch((error: unknown) => setNote(error instanceof Error ? error.message : "Download failed."))}
        >Download recent diagnostics</button>
      </div>
      {note && <p className="hint" role="status">{note}</p>}
    </section>
  );
}

function RhythmPanel({ config }: { config: AppConfig }) {
  const { client, status } = useGate();
  const rhythm = config.rhythm;
  const commit = (patch: Partial<AppConfig["rhythm"]>) =>
    client.setConfig({ ...config, rhythm: { ...rhythm, ...patch } });
  const clock = status?.rhythm;

  return (
    <section className="panel">
      <h2>Lighting clock</h2>
      <p className="hint">
        Timing is independent from audio energy. An external DJ clock can lock every layer while
        each layer still gets level, bands, waveform, and spectrum from its selected audio source.
      </p>
      <label className="field-row">
        <span>Timing source</span>
        <select
          value={rhythm.source}
          onChange={(e) => commit({ source: e.target.value as AppConfig["rhythm"]["source"] })}
        >
          <option value="layer_audio">Each layer's audio detector</option>
          <option value="midi_clock">MIDI Clock (global)</option>
          <option value="pro_dj_link">Pioneer PRO DJ LINK (global)</option>
        </select>
      </label>
      {rhythm.source === "midi_clock" && (
        <>
          <label className="field-row">
            <span>MIDI input</span>
            <select
              value={rhythm.midi_port ?? ""}
              onChange={(e) => commit({ midi_port: e.target.value || null })}
            >
              <option value="">Select a MIDI input…</option>
              {rhythm.midi_port && !status?.midi_ports.includes(rhythm.midi_port) && (
                <option value={rhythm.midi_port}>{rhythm.midi_port} (missing)</option>
              )}
              {(status?.midi_ports ?? []).map((port) => (
                <option key={port} value={port}>{port}</option>
              ))}
            </select>
          </label>
        </>
      )}
      {rhythm.source === "pro_dj_link" && (
        <>
          <label className="field-row">
            <span>Deck</span>
            <select
              value={rhythm.pro_dj_link_player}
              onChange={(e) => commit({ pro_dj_link_player: Number(e.target.value) })}
            >
              <option value={0}>Auto — follow tempo master</option>
              {rhythm.pro_dj_link_player > 0 &&
                !status?.pro_dj_link_devices.some((d) => d.number === rhythm.pro_dj_link_player) && (
                  <option value={rhythm.pro_dj_link_player}>
                    Player {rhythm.pro_dj_link_player} (not detected)
                  </option>
                )}
              {(status?.pro_dj_link_devices ?? []).map((deck) => (
                <option key={deck.number} value={deck.number}>
                  Player {deck.number} — {deck.name}
                </option>
              ))}
            </select>
          </label>
          <p className="hint">
            Receive-only: Gate listens on UDP 50001/50002 and never claims a deck number or sends
            sync/master commands. If Auto cannot see full status, select the playing deck number.
          </p>
        </>
      )}
      {rhythm.source !== "layer_audio" && (
        <>
          <label className="slider-row">
            <span>Latency offset</span>
            <input
              type="range"
              min={-250}
              max={250}
              step={1}
              value={rhythm.latency_ms}
              onChange={(e) => commit({ latency_ms: Number(e.target.value) })}
            />
            <span className="slider-val">{rhythm.latency_ms.toFixed(0)} ms</span>
          </label>
          <p className="hint">Positive values delay the visual beat; negative values lead it.</p>
          <label className="toggle-row">
            <input
              type="checkbox"
              checked={rhythm.fallback_to_audio}
              onChange={(e) => commit({ fallback_to_audio: e.target.checked })}
            />
            Fall back to audio if the external clock disappears
            {rhythm.fallback_to_audio && (
              <select
                value={rhythm.fallback_audio_source}
                onChange={(e) => commit({ fallback_audio_source: Number(e.target.value) })}
              >
                {config.audio.sources.map((source, i) => (
                  <option key={i} value={i}>{source.id}</option>
                ))}
              </select>
            )}
          </label>
        </>
      )}
      {clock && (
        <div className="meters">
          <span className={clock.active ? "ok" : "warn"}>
            {clock.active ? (clock.using_fallback ? "AUDIO FALLBACK" : "LOCKED") : "WAITING"}
          </span>
          <span className="bpm">{clock.bpm > 0 ? `${clock.bpm.toFixed(1)} BPM` : "—"}</span>
          <span className="hint">{clock.detail}</span>
        </div>
      )}
    </section>
  );
}

function UpdatesPanel({ config }: { config: AppConfig }) {
  const { client, status } = useGate();
  const commit = (patch: Partial<AppConfig["update"]>) =>
    client.setConfig({ ...config, update: { ...config.update, ...patch } });
  return (
    <section className="panel">
      <h2>Updates</h2>
      <p className="hint">
        Running v{status?.version ?? "?"}
        {status?.update_available
          ? ` — v${status.update_available} is available.`
          : status?.update_state
            ? ` — ${status.update_state}.`
            : "."}{" "}
        Updating downloads the new binary beside this one and hot-swaps via the seamless
        takeover — the structure sees at most a frame.
      </p>
      <div className="add-row">
        <button onClick={() => client.send({ type: "check_update" })}>Check now</button>
        {status?.update_available && (
          <button onClick={() => client.send({ type: "install_update" })}>
            Update to v{status.update_available}
          </button>
        )}
      </div>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={config.update.auto_check}
          onChange={(e) => commit({ auto_check: e.target.checked })}
        />
        Check automatically (startup + every 6 h)
      </label>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={config.update.auto_install}
          onChange={(e) => commit({ auto_install: e.target.checked })}
        />
        Install automatically when found
      </label>
    </section>
  );
}

// ---------------------------------------------------------------------------

function ClientsPanel() {
  const { client, config, status } = useGate();
  const list = status?.client_list ?? [];
  return (
    <section className="panel">
      <h2>Clients</h2>
      <p className="hint">
        Devices that have connected. Revoking kicks a device immediately and blocks its id.
        With open join (below unchecked) a determined device could rejoin with a fresh
        identity — require the join token and rotate it for a real lockout.
      </p>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={config?.server.require_token ?? false}
          onChange={(e) => client.send({ type: "set_require_token", require: e.target.checked })}
        />
        Require join token (new devices must scan the Connect QR)
        <button onClick={() => client.send({ type: "rotate_join_token" })}>Rotate token</button>
      </label>
      <label className="field-row" style={{ maxWidth: 320 }}>
        <span>Max live viewers (WiFi guard)</span>
        <input
          type="number"
          min={1}
          max={64}
          value={config?.server.max_preview_clients ?? 10}
          onChange={(e) => {
            if (config) {
              client.setConfig({
                ...config,
                server: {
                  ...config.server,
                  max_preview_clients: Math.max(1, Number(e.target.value) || 1),
                },
              });
            }
          }}
        />
      </label>
      <p className="hint">
        Only this many clients stream the live view at once (a few Mbps per phone);
        extras queue for a slot but keep full control of taps, drawing, and effects.
      </p>
      {list.length === 0 && <p className="hint">No devices yet — use ⊕ Connect in the top bar.</p>}
      {list.map((c) => (
        <div className="client-row" key={c.id}>
          <span className={c.connected ? "conn-dot on" : "conn-dot"} />
          <div className="client-identity">
            <input
              defaultValue={c.name}
              onBlur={(e) => {
                if (e.target.value !== c.name) {
                  client.send({ type: "rename_client", id: c.id, name: e.target.value });
                }
              }}
            />
            <span className="hint">{c.id === client.clientId ? "this device" : c.id}</span>
          </div>
          <div className="client-actions">
            {c.revoked ? (
              <button onClick={() => client.send({ type: "unrevoke_client", id: c.id })}>
                Restore
              </button>
            ) : (
              <button
                className="danger"
                onClick={() => client.send({ type: "revoke_client", id: c.id })}
              >
                Revoke
              </button>
            )}
            {!c.connected && (
              <button className="danger" onClick={() => client.send({ type: "forget_client", id: c.id })}>
                Forget
              </button>
            )}
          </div>
        </div>
      ))}
    </section>
  );
}

// ---------------------------------------------------------------------------

function Slider({
  label,
  value,
  min = 0,
  max = 1,
  step = 0.01,
  onChange,
}: {
  label: string;
  value: number;
  min?: number;
  max?: number;
  step?: number;
  onChange: (v: number) => void;
}) {
  return (
    <label className="slider-row">
      <span>{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span className="slider-val">{value.toFixed(2)}</span>
    </label>
  );
}

function NumberField({
  label,
  value,
  onCommit,
  step = 1,
}: {
  label: string;
  value: number;
  onCommit: (v: number) => void;
  step?: number;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  return (
    <label className="field-row">
      <span>{label}</span>
      <input
        type="number"
        value={text}
        step={step}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const v = Number(text);
          if (!Number.isNaN(v) && v !== value) onCommit(v);
        }}
      />
    </label>
  );
}

// ---------------------------------------------------------------------------

function LayerEditor({ layer, index }: { layer: LayerCfg; index: number }) {
  const { client, config } = useGate();
  const throttledUpdate = useThrottled(
    (l: LayerCfg) => client.updateLayer(index, l),
    100,
  );
  // Local mirror so sliders feel instant while updates stream out.
  const [local, setLocal] = useState(layer);
  const dragging = useRef(false);
  useEffect(() => {
    if (!dragging.current) setLocal(layer);
  }, [layer]);

  const up = (patch: Partial<LayerCfg>) => {
    const next = { ...local, ...patch };
    setLocal(next);
    throttledUpdate(next);
  };

  const params = PARAM_LABELS[local.kind] ?? [];
  const sourceCount = config?.audio.sources.length ?? 1;

  return (
    <div
      className={`layer-card ${local.enabled ? "" : "disabled"}`}
      onPointerDown={() => (dragging.current = true)}
      onPointerUp={() => (dragging.current = false)}
    >
      <div className="layer-head">
        <input
          type="checkbox"
          checked={local.enabled}
          onChange={(e) => up({ enabled: e.target.checked })}
        />
        <select
          value={local.kind}
          onChange={(e) => up({ kind: e.target.value as LayerKind })}
        >
          {LAYER_KINDS.map((k) => (
            <option key={k} value={k}>
              {LAYER_LABELS[k]}
            </option>
          ))}
        </select>
        <select
          value={local.blend}
          onChange={(e) => up({ blend: e.target.value as LayerCfg["blend"] })}
        >
          {BLEND_MODES.map((b) => (
            <option key={b} value={b}>
              {b}
            </option>
          ))}
        </select>
        <select
          value={local.audio_source}
          onChange={(e) => up({ audio_source: Number(e.target.value) })}
        >
          {Array.from({ length: Math.max(sourceCount, 1) }, (_, i) => (
            <option key={i} value={i}>
              {config?.audio.sources[i]?.id ?? `src ${i}`}
            </option>
          ))}
        </select>
        <span className="spacer" />
        <button onClick={() => client.moveLayer(index, Math.max(0, index - 1))}>↑</button>
        <button onClick={() => client.moveLayer(index, index + 1)}>↓</button>
        <button className="danger" onClick={() => client.removeLayer(index)}>
          ✕
        </button>
      </div>
      <div className="layer-sliders">
        <Slider label="Opacity" value={local.opacity} onChange={(v) => up({ opacity: v })} />
        <Slider label="Speed" value={local.speed} min={-4} max={4} onChange={(v) => up({ speed: v })} />
        <Slider label="Scale" value={local.scale} min={0.05} max={5} onChange={(v) => up({ scale: v })} />
        <Slider label="Audio" value={local.audio_amount} onChange={(v) => up({ audio_amount: v })} />
        <Slider label="Hue" value={local.hue} onChange={(v) => up({ hue: v })} />
        <Slider label="Hue range" value={local.hue_range} onChange={(v) => up({ hue_range: v })} />
        <Slider label="Saturation" value={local.saturation} onChange={(v) => up({ saturation: v })} />
        <Slider label="Brightness" value={local.brightness} max={2} onChange={(v) => up({ brightness: v })} />
        <Slider label="Tilt (IMU)" value={local.tilt_amount} onChange={(v) => up({ tilt_amount: v })} />
        <Slider label="Walk" value={local.walk_amount} onChange={(v) => up({ walk_amount: v })} />
        <Slider label={params[0] ?? "Param A"} value={local.param_a} onChange={(v) => up({ param_a: v })} />
        <Slider label={params[1] ?? "Param B"} value={local.param_b} onChange={(v) => up({ param_b: v })} />
        <Slider label={params[2] ?? "Param C"} value={local.param_c} onChange={(v) => up({ param_c: v })} />
        <Slider label={params[3] ?? "Param D"} value={local.param_d} onChange={(v) => up({ param_d: v })} />
      </div>
    </div>
  );
}

function LayersPanel({ config }: { config: AppConfig }) {
  const { client } = useGate();
  const [kind, setKind] = useState<LayerKind>("noise_field");
  return (
    <section className="panel">
      <h2>Layers</h2>
      <p className="hint">Rendered bottom to top. Each layer picks an audio source to react to.</p>
      {config.layers.map((l, i) => (
        <LayerEditor key={i} layer={l} index={i} />
      ))}
      <div className="add-row">
        <select value={kind} onChange={(e) => setKind(e.target.value as LayerKind)}>
          {LAYER_KINDS.map((k) => (
            <option key={k} value={k}>
              {LAYER_LABELS[k]}
            </option>
          ))}
        </select>
        <button onClick={() => client.addLayer(defaultLayer(kind))}>Add layer</button>
        <button
          onClick={() => {
            const have = new Set(config.layers.map((l) => l.kind));
            for (const k of LAYER_KINDS) {
              if (!have.has(k)) client.addLayer(defaultLayer(k));
            }
          }}
        >
          Add missing kinds
        </button>
      </div>
      <p className="hint">
        "Add missing kinds" gives the stack one layer of every pattern — pair it with
        Autopilot's "walk which layers play" (Control tab) to tour them all hands-free.
      </p>
    </section>
  );
}

// ---------------------------------------------------------------------------

function AudioPanel({ config }: { config: AppConfig }) {
  const { client, status } = useGate();
  const sources = config.audio.sources;

  const commit = (next: AudioSourceConfig[]) => {
    client.setConfig({ ...config, audio: { sources: next } });
  };

  const updateSource = (i: number, patch: Partial<AudioSourceConfig>) => {
    const next = sources.map((s, j) => (j === i ? ({ ...s, ...patch } as AudioSourceConfig) : s));
    commit(next);
  };

  return (
    <section className="panel">
      <h2>Audio sources</h2>
      <p className="hint">
        Up to 4 analyzed in parallel — e.g. main stage feed + a local mic. Channels lets one
        multichannel interface feed several sources (blank = mix all). Remote sources take
        features from a browser client's mic; Video soundtrack follows the current Media-tab source.
      </p>
      {sources.map((s, i) => {
        const st = status?.audio[i];
        return (
          <div className="source-card" key={i}>
            <div className="layer-head">
              <input
                value={s.id}
                onChange={(e) => updateSource(i, { id: e.target.value })}
                style={{ width: "8em" }}
              />
              <select
                value={s.kind === "device" ? (s.loopback ? "loopback" : "device") : s.kind}
                onChange={(e) => {
                  const kind = e.target.value;
                  const base = { id: s.id, gain: s.gain };
                  commit(
                    sources.map((x, j) =>
                      j === i
                        ? kind === "remote"
                          ? { ...base, kind: "remote", client_id: "" }
                          : kind === "video"
                            ? { ...base, kind: "video" }
                            : {
                              ...base,
                              kind: "device",
                              device: null,
                              channels: [],
                              loopback: kind === "loopback",
                            }
                        : x,
                    ) as AudioSourceConfig[],
                  );
                }}
              >
                <option value="device">Input device</option>
                <option value="loopback">System output (loopback)</option>
                <option value="remote">Remote (browser mic)</option>
                <option value="video">Video soundtrack</option>
              </select>
              {s.kind === "device" && (
                <>
                  <select
                    value={s.device ?? ""}
                    onChange={(e) => updateSource(i, { device: e.target.value || null })}
                  >
                    <option value="">{s.loopback ? "Default output" : "System default"}</option>
                    {(s.loopback ? (status?.output_devices ?? []) : (status?.input_devices ?? [])).map(
                      (d) => (
                        <option key={d.name} value={d.name}>
                          {d.name} ({d.channels} ch)
                        </option>
                      ),
                    )}
                  </select>
                  <ChannelPicker source={s} onChange={(channels) => updateSource(i, { channels })} />
                </>
              )}
              {s.kind === "remote" && (
                <input
                  placeholder="client id"
                  value={s.client_id}
                  onChange={(e) => updateSource(i, { client_id: e.target.value })}
                  style={{ width: "10em" }}
                />
              )}
              <span className="spacer" />
              <button className="danger" onClick={() => commit(sources.filter((_, j) => j !== i))}>
                ✕
              </button>
            </div>
            {st && (
              <div className="meters">
                <Meter label="Level" v={st.level} />
                <Meter label="Bass" v={st.bass} />
                <Meter label="Mid" v={st.mid} />
                <Meter label="Treble" v={st.treble} />
                <span className="bpm">
                  {st.bpm > 0 && st.bpm_confidence >= 0.35
                    ? `${st.bpm.toFixed(0)} BPM`
                    : st.bpm > 0
                      ? "finding beat…"
                      : "—"}
                </span>
                <span className={st.active ? "ok" : "warn"}>
                  {st.active ? "active" : st.detail || "inactive"}
                </span>
              </div>
            )}
          </div>
        );
      })}
      {sources.length < 4 && (
        <button
          onClick={() =>
            commit([
              ...sources,
              {
                id: `source-${sources.length}`,
                kind: "device",
                device: null,
                channels: [],
                loopback: false,
                gain: 1,
              },
            ])
          }
        >
          Add source
        </button>
      )}
    </section>
  );
}

/// Channels as checkboxes (labeled 1-based like audio gear; stored 0-based).
/// All-checked is stored as [] = "all channels", so new devices Just Work.
/// Falls back to a text field when the count is unknown or absurd.
function ChannelPicker({
  source,
  onChange,
}: {
  source: AudioSourceConfig & { kind: "device" };
  onChange: (channels: number[]) => void;
}) {
  const { status } = useGate();
  const list = source.loopback ? (status?.output_devices ?? []) : (status?.input_devices ?? []);
  const count = source.device
    ? (list.find((d) => d.name === source.device)?.channels ?? 0)
    : source.loopback
      ? (status?.default_output_channels ?? 0)
      : (status?.default_input_channels ?? 0);

  if (count < 1 || count > 16) {
    return (
      <input
        placeholder="channels: blank = all"
        defaultValue={source.channels.join(",")}
        style={{ width: "10em" }}
        onBlur={(e) =>
          onChange(
            e.target.value
              .split(",")
              .map((x) => parseInt(x.trim(), 10))
              .filter((x) => !Number.isNaN(x) && x >= 0),
          )
        }
      />
    );
  }

  const active = (ch: number) => source.channels.length === 0 || source.channels.includes(ch);
  const toggle = (ch: number) => {
    const next = Array.from({ length: count }, (_, c) => c).filter((c) =>
      c === ch ? !active(c) : active(c),
    );
    // Never allow zero channels; and all-selected collapses to [] ("all").
    if (next.length === 0) return;
    onChange(next.length === count ? [] : next);
  };

  return (
    <span className="channel-picker">
      <span className="hint">ch</span>
      {Array.from({ length: count }, (_, ch) => (
        <button
          key={ch}
          className={`channel-btn ${active(ch) ? "active" : ""}`}
          onClick={() => toggle(ch)}
        >
          {ch + 1}
        </button>
      ))}
    </span>
  );
}

function Meter({ label, v }: { label: string; v: number }) {
  return (
    <span className="meter">
      <span className="meter-label">{label}</span>
      <span className="meter-track">
        <span className="meter-fill" style={{ width: `${Math.min(100, v * 100)}%` }} />
      </span>
    </span>
  );
}

// ---------------------------------------------------------------------------

function OutputPanel({ config }: { config: AppConfig }) {
  const { client, status } = useGate();
  const out = config.output;
  const commit = (patch: Partial<AppConfig["output"]>) =>
    client.setConfig({ ...config, output: { ...out, ...patch } });

  // LED-wire fps ceiling: 800 kbps WS281x, 24 bits/px + ~300 µs reset per frame.
  const pxPerString = config.geometry.pixels_per_spoke;
  const wireFpsCap = 1 / ((pxPerString * 30e-6) + 300e-6);

  return (
    <section className="panel">
      <h2>sACN output</h2>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={out.enabled}
          onChange={(e) => client.setSacnEnabled(e.target.checked)}
        />
        Enable sACN output
        {status &&
          (out.enabled ? (
            status.sacn_pps > 0 ? (
              <span className="live-pill">
                TRANSMITTING · {status.sacn_universes} universes · {status.sacn_pps} pkt/s
              </span>
            ) : (
              <span className="warn">⚠ enabled but nothing on the wire — check the interface below</span>
            )
          ) : (
            <span className="hint">off</span>
          ))}
      </label>
      <label className="field-row" style={{ maxWidth: 460 }}>
        <span>Network interface</span>
        <select
          value={out.interface}
          onChange={(e) => commit({ interface: e.target.value })}
          style={{ flex: 1 }}
        >
          <option value="">OS default route</option>
          {(status?.interfaces ?? []).map((i) => {
            const ip = i.split("—").pop()?.trim() ?? i;
            return (
              <option key={i} value={ip}>
                {i}
              </option>
            );
          })}
        </select>
      </label>
      <p className="hint">
        Pick the interface that is on the lighting network — multicast leaves through this NIC.
      </p>
      <label className="field-row" style={{ maxWidth: 460 }}>
        <span>Source name</span>
        <input
          type="text"
          key={out.source_name}
          defaultValue={out.source_name}
          maxLength={63}
          placeholder="Empyrean Gate"
          onBlur={(e) => commit({ source_name: e.target.value })}
          style={{ flex: 1 }}
        />
      </label>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={out.discovery}
          onChange={(e) => commit({ discovery: e.target.checked })}
        />
        Advertise our universe list on the discovery universe (64214) every 10 s
      </label>
      <p className="hint">
        Receivers and tools like sACNView identify this source by name, and by its CID{" "}
        <code>{out.cid}</code> — generated once and persistent, so a restart or a handover
        between instances looks like the <em>same</em> source instead of a second one
        fighting the first in the receiver's merge. Discovery is what makes the source and
        its universes appear in those tools; turn it off only on a network where the extra
        multicast is unwelcome.
      </p>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={out.sync_to_render}
          onChange={(e) => commit({ sync_to_render: e.target.checked })}
        />
        Sync sACN to render fps (capped by the fps field below)
      </label>
      <div className="field-grid">
        <NumberField
          label={out.sync_to_render ? "fps cap" : "Fixed fps"}
          value={out.fps}
          onCommit={(v) => commit({ fps: v })}
        />
        <NumberField
          label="Sync universe (0 = off)"
          value={out.sync_universe}
          onCommit={(v) => commit({ sync_universe: v })}
        />
        <NumberField
          label="Start universe"
          value={out.start_universe}
          onCommit={(v) => commit({ start_universe: v })}
        />
        <NumberField
          label="Pixels / universe"
          value={out.pixels_per_universe}
          onCommit={(v) => commit({ pixels_per_universe: v })}
        />
        <NumberField
          label="Strings / controller"
          value={out.strings_per_controller}
          onCommit={(v) => commit({ strings_per_controller: v })}
        />
        <NumberField
          label="LED gamma"
          value={out.led_gamma}
          step={0.1}
          onCommit={(v) => commit({ led_gamma: v })}
        />
        <NumberField label="Priority" value={out.priority} onCommit={(v) => commit({ priority: v })} />
      </div>
      <p className="hint">
        LED-wire ceiling at {pxPerString} px/string: ~{wireFpsCap.toFixed(0)} fps (800 kbps ×
        24 bits/px + reset). Ethernet is not the limit. Sync universe uses E1.31 universe
        synchronization — PixLite Mk4 latches all universes on the sync packet (tear-free);
        receivers without support ignore it.
      </p>
      <div className="output-mode" role="group" aria-label="sACN destination mode">
        <button
          className={out.multicast ? "active" : ""}
          onClick={() => commit({ multicast: true })}
        >
          Multicast
          <span>Standard 239.255.x.x groups</span>
        </button>
        <button
          className={!out.multicast ? "active" : ""}
          onClick={() => commit({ multicast: false })}
        >
          Unicast
          <span>Only the controller IPs below</span>
        </button>
      </div>
      <p className="hint">
        Choose one destination mode. Multicast benefits from an IGMP-snooping switch;
        unicast keeps lighting traffic off unrelated switch ports.
      </p>
      <label className="field-col">
        <span>Controller IPs (one per line, in spoke order; controller N drives spokes N×4…N×4+3)</span>
        <textarea
          rows={6}
          defaultValue={out.controllers.join("\n")}
          onBlur={(e) =>
            commit({
              controllers: e.target.value
                .split("\n")
                .map((s) => s.trim())
                .filter((s) => s.length > 0),
            })
          }
          placeholder={"10.0.0.101\n10.0.0.102\n…"}
        />
      </label>
      {!out.multicast && out.controllers.length === 0 && (
        <p className="warn">Unicast is selected but no controller IPs are configured.</p>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------

function GeometryPanel({ config }: { config: AppConfig }) {
  const { client } = useGate();
  const g = config.geometry;
  const commit = (patch: Partial<AppConfig["geometry"]>) =>
    client.setConfig({ ...config, geometry: { ...g, ...patch } });

  const stripFt = (g.pixels_per_spoke / g.leds_per_meter) * 3.28084;
  const spanFt = g.outer_radius_ft - g.inner_radius_ft;

  return (
    <section className="panel">
      <h2>Geometry</h2>
      <div className="field-grid">
        <NumberField label="Spokes" value={g.spokes} onCommit={(v) => commit({ spokes: v })} />
        <NumberField
          label="Pixels / spoke"
          value={g.pixels_per_spoke}
          onCommit={(v) => commit({ pixels_per_spoke: v })}
        />
        <NumberField
          label="Outer radius (ft)"
          value={g.outer_radius_ft}
          step={0.5}
          onCommit={(v) => commit({ outer_radius_ft: v })}
        />
        <NumberField
          label="Inner radius (ft)"
          value={g.inner_radius_ft}
          step={0.5}
          onCommit={(v) => commit({ inner_radius_ft: v })}
        />
        <NumberField
          label="LEDs / meter"
          value={g.leds_per_meter}
          onCommit={(v) => commit({ leds_per_meter: v })}
        />
      </div>
      <p className="hint">
        Sanity check: {g.pixels_per_spoke} px at {g.leds_per_meter}/m = {stripFt.toFixed(1)} ft of
        strip; the outer→inner span is {spanFt.toFixed(1)} ft.{" "}
        {Math.abs(stripFt - spanFt) > 2 ? "⚠ these disagree — one of the numbers is off." : "✓ consistent."}
      </p>
      <p className="hint">Pixel 0 of every string is at the OUTER edge (fed from outside).</p>
    </section>
  );
}

// ---------------------------------------------------------------------------

function ThisDevicePanel() {
  const { client } = useGate();
  const [micStop, setMicStop] = useState<(() => void) | null>(null);
  const [imuStop, setImuStop] = useState<(() => void) | null>(null);
  const [err, setErr] = useState("");

  return (
    <section className="panel this-device-panel">
      <h2>This device</h2>
      <p className="hint">
        Contribute this device's inputs to the show. Client id: <code>{client.clientId}</code> — add
        a Remote audio source with this id to use the mic as a beat source.
      </p>
      <label className="field-row device-name-row">
        <span>Device name</span>
        <input
          defaultValue={client.deviceName}
          placeholder="e.g. DJ booth iPad"
          onBlur={(e) => client.setDeviceName(e.target.value)}
        />
      </label>
      <div className="add-row">
        <button
          onClick={async () => {
            try {
              setErr("");
              if (micStop) {
                micStop();
                setMicStop(null);
              } else {
                const stop = await startMic(client);
                setMicStop(() => stop);
              }
            } catch (e) {
              setErr(e instanceof Error ? e.message : String(e));
            }
          }}
        >
          {micStop ? "Stop microphone" : "Send microphone"}
        </button>
        <button
          onClick={async () => {
            try {
              setErr("");
              if (imuStop) {
                imuStop();
                setImuStop(null);
              } else {
                const stop = await startImu(client);
                setImuStop(() => stop);
              }
            } catch (e) {
              setErr(e instanceof Error ? e.message : String(e));
            }
          }}
        >
          {imuStop ? "Stop motion" : "Send motion / orientation"}
        </button>
      </div>
      {err && <p className="warn" role="alert">{err}</p>}
    </section>
  );
}
