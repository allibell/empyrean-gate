// Control tab: performance surface — effect pads, master faders, and per-layer
// quick faders. Built for touch (big targets), works everywhere.

import { useEffect, useState } from "react";
import { EFFECTS } from "./effects";
import { SCENE_PRESETS, type ScenePreset } from "./scenes";
import Sparkbars from "./Sparkbars";
import { useGate, useThrottled } from "./state";
import {
  LAYER_LABELS,
  type PlaylistEntry,
  type SavedPlaylist,
  type SavedStack,
} from "./types";

const newId = (prefix: string) =>
  `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;

function stackFromScene(scene: ScenePreset): SavedStack {
  return {
    id: `built-in-${scene.id}`,
    name: scene.name,
    layers: scene.layers.map((layer) => ({ ...layer })),
    master_speed: scene.masterSpeed,
    walk_enabled: true,
    walk_layers: false,
    walk_min_layers: 1,
    walk_speed: scene.walkSpeed,
    walk_depth: scene.walkDepth,
  };
}

function playlistEntry(stack: SavedStack): PlaylistEntry {
  return {
    id: newId("cue"),
    name: stack.name,
    stack: { ...stack, layers: stack.layers.map((layer) => ({ ...layer })) },
    duration_secs: 35 * 60,
    transition_secs: 20,
  };
}

function choose(n: number, k: number): number {
  if (k < 0 || k > n) return 0;
  let r = 1;
  for (let i = 0; i < k; i++) r = (r * (n - i)) / (i + 1);
  return Math.round(r);
}

function humanize(seconds: number): string {
  if (seconds < 90) return `${Math.round(seconds)} s`;
  if (seconds < 5400) return `${Math.round(seconds / 60)} min`;
  if (seconds < 172800) return `${(seconds / 3600).toFixed(1)} h`;
  return `${(seconds / 86400).toFixed(1)} days`;
}

/** The autopilot's time horizons, computed from the current config. */
function autopilotForecast(
  enabledLayers: number,
  minOn: number,
  walkSpeed: number,
  walkLayers: boolean,
): { stepS: number; combos: number; tourS: number | null } {
  const stepS = 45 / Math.max(0.05, walkSpeed);
  if (!walkLayers || enabledLayers === 0) return { stepS, combos: 0, tourS: null };
  const m = Math.min(minOn, enabledLayers);
  let combos = 0;
  for (let k = m; k <= enabledLayers; k++) combos += choose(enabledLayers, k);
  // Expected cover time of the one-flip random walk over those states:
  // coupon-collector core S·(ln S + γ) with a ~1.3 walk-vs-sampling factor.
  const tourSteps = combos <= 1 ? 0 : combos * (Math.log(combos) + 0.577) * 1.3;
  return { stepS, combos, tourS: tourSteps * stepS };
}

export default function Control() {
  const { client, config, status } = useGate();
  const setBrightness = useThrottled((v: number) => client.setMaster({ brightness: v }));
  const setSpeed = useThrottled((v: number) => client.setMaster({ speed: v }));
  const setRender = useThrottled((patch: Partial<NonNullable<typeof config>["render"]>) => {
    if (config) {
      client.setConfig({ ...config, render: { ...config.render, ...patch } });
    }
  });

  // Local mirror of master sliders so they track remote changes when idle.
  const [brightness, setBrightnessLocal] = useState(1);
  const [speed, setSpeedLocal] = useState(1);
  useEffect(() => {
    if (config) {
      setBrightnessLocal(config.render.master_brightness);
      setSpeedLocal(config.render.master_speed);
    }
  }, [config]);

  return (
    <div className="control-page">
      <section className="panel">
        <h2>Effects</h2>
        <div className="effect-row big">
          {EFFECTS.map((e) => (
            <button
              key={e.kind}
              className="effect-btn"
              onClick={() =>
                client.triggerEffect({ kind: e.kind, angle: Math.random() * Math.PI * 2 })
              }
            >
              {e.label}
              <span className="key-hint">{e.key}</span>
            </button>
          ))}
        </div>
      </section>

      <section className="panel">
        <h2>Master</h2>
        <label className="slider-row">
          <span>Brightness</span>
          <input
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={brightness}
            onChange={(e) => {
              const v = Number(e.target.value);
              setBrightnessLocal(v);
              setBrightness(v);
            }}
          />
          <span className="slider-val">{brightness.toFixed(2)}</span>
        </label>
        <label className="slider-row">
          <span>Speed</span>
          <input
            type="range"
            min={0}
            max={4}
            step={0.05}
            value={speed}
            onChange={(e) => {
              const v = Number(e.target.value);
              setSpeedLocal(v);
              setSpeed(v);
            }}
          />
          <span className="slider-val">{speed.toFixed(2)}</span>
        </label>
        {status?.sacn_enabled && (
          <p className="warn">
            sACN output is LIVE{" "}
            <Sparkbars
              data={status.pps_history}
              color="#7c5cff"
              label="pkt/s"
              value={String(status.sacn_pps)}
              warn={status.sacn_pps === 0}
            />
          </p>
        )}
        {status && (
          <Sparkbars
            data={status.fps_history}
            color="#38d1c2"
            label="fps"
            value={String(status.fps_history.at(-1) ?? 0)}
          />
        )}
      </section>

      <BeatTapsPanel />

      <ScenesPanel />

      <section className="panel">
        <h2>Autopilot</h2>
        <p className="hint">
          Slow random walk across layer parameters so the show evolves for hours
          unattended. Each layer's "Walk" slider (Settings) limits how far its
          parameters may wander from where you set them.
        </p>
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={config?.render.walk_enabled ?? true}
            onChange={(e) => setRender({ walk_enabled: e.target.checked })}
          />
          Enabled
        </label>
        <label className="slider-row">
          <span>Walk speed</span>
          <input
            type="range"
            min={0.1}
            max={5}
            step={0.1}
            value={config?.render.walk_speed ?? 1}
            onChange={(e) => setRender({ walk_speed: Number(e.target.value) })}
          />
        </label>
        <label className="slider-row">
          <span>Walk depth</span>
          <input
            type="range"
            min={0}
            max={3}
            step={0.1}
            value={config?.render.walk_depth ?? 1}
            onChange={(e) => setRender({ walk_depth: Number(e.target.value) })}
          />
        </label>
        <p className="hint">
          Depth is how far parameters wander from your sliders (1 = subtle, 3 = wild);
          speed is how fast. The wander is mean-reverting, so it always comes home.
        </p>
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={config?.render.walk_layers ?? false}
            onChange={(e) => setRender({ walk_layers: e.target.checked })}
          />
          Walk which layers play (one fades in or out per step)
        </label>
        {config?.render.walk_layers && (
          <label className="field-row" style={{ maxWidth: 280 }}>
            <span>Minimum layers on</span>
            <input
              type="number"
              min={1}
              max={24}
              value={config.render.walk_min_layers}
              onChange={(e) =>
                setRender({ walk_min_layers: Math.max(1, Number(e.target.value) || 1) })
              }
            />
          </label>
        )}
        {config && <AutopilotForecast />}
      </section>

      <section className="panel">
        <h2>Layers</h2>
        {config?.layers.map((l, i) => (
          <LayerFader key={i} index={i} name={l.name || LAYER_LABELS[l.kind]} />
        ))}
      </section>
    </div>
  );
}

function ShowSchedulerPanel() {
  const { client, config, status } = useGate();
  if (!config) return null;

  const playlists = config.saved_playlists ?? [];
  const scheduler = config.show_scheduler ?? {
    enabled: false,
    active_playlist_id: "",
    current_index: 0,
  };
  const active = playlists.find((playlist) => playlist.id === scheduler.active_playlist_id)
    ?? playlists[0];
  const activeIndex = active
    ? Math.min(scheduler.current_index, Math.max(0, active.entries.length - 1))
    : 0;

  const commit = (
    nextPlaylists: SavedPlaylist[],
    schedulerPatch: Partial<typeof scheduler> = {},
  ) => client.setConfig({
    ...config,
    saved_playlists: nextPlaylists,
    show_scheduler: { ...scheduler, ...schedulerPatch },
  });

  const updateActive = (update: (playlist: SavedPlaylist) => SavedPlaylist) => {
    if (!active) return;
    commit(playlists.map((playlist) => playlist.id === active.id ? update(playlist) : playlist));
  };

  const createShow = () => {
    const id = newId("show");
    const playlist: SavedPlaylist = {
      id,
      name: `All-night journey ${playlists.length + 1}`,
      entries: SCENE_PRESETS.map((scene) => playlistEntry(stackFromScene(scene))),
      repeat: true,
    };
    commit([...playlists, playlist], { active_playlist_id: id, current_index: 0 });
  };

  const stop = () => {
    const entry = active?.entries[activeIndex];
    if (!entry) {
      commit(playlists, { enabled: false });
      return;
    }
    const stack = entry.stack;
    client.setConfig({
      ...config,
      layers: stack.layers.map((layer) => ({ ...layer })),
      render: {
        ...config.render,
        master_speed: stack.master_speed,
        walk_enabled: stack.walk_enabled,
        walk_layers: stack.walk_layers,
        walk_min_layers: stack.walk_min_layers,
        walk_speed: stack.walk_speed,
        walk_depth: stack.walk_depth,
      },
      show_scheduler: { ...scheduler, enabled: false },
    });
  };

  const move = (index: number, direction: -1 | 1) => {
    updateActive((playlist) => {
      const entries = [...playlist.entries];
      const destination = index + direction;
      if (destination < 0 || destination >= entries.length) return playlist;
      [entries[index], entries[destination]] = [entries[destination], entries[index]];
      return { ...playlist, entries };
    });
  };

  const show = status?.show;
  const runningHere = Boolean(show?.enabled && show.playlist_id === active?.id);
  const progress = show?.transition_progress ?? 0;

  return (
    <div className={`show-scheduler ${scheduler.enabled ? "running" : ""}`}>
      <div className="show-scheduler-head">
        <div>
          <p className="eyebrow">Set and forget</p>
          <h3>Unattended show</h3>
          <p>Gate advances and crossfades in the backend—even with every controller closed.</p>
        </div>
        {scheduler.enabled ? (
          <button className="danger" onClick={stop}>Stop &amp; hold scene</button>
        ) : (
          <button className="primary" disabled={!active?.entries.length}
            onClick={() => commit(playlists, {
              enabled: true,
              active_playlist_id: active?.id ?? "",
              current_index: activeIndex,
            })}
          >▶ Start show</button>
        )}
      </div>

      {playlists.length === 0 ? (
        <button className="show-create" onClick={createShow}>
          <strong>Create an all-night journey</strong>
          <span>Preload every built-in scene · 35 min each · 20 s crossfades · repeat forever</span>
        </button>
      ) : (
        <>
          <div className="show-toolbar">
            <select value={active?.id ?? ""} onChange={(event) => commit(playlists, {
              active_playlist_id: event.target.value,
              current_index: 0,
              enabled: false,
            })}>
              {playlists.map((playlist) => <option key={playlist.id} value={playlist.id}>{playlist.name}</option>)}
            </select>
            <button onClick={createShow}>＋ New all-night show</button>
            <button className="danger" onClick={() => {
              if (!active || !window.confirm(`Delete show “${active.name}”?`)) return;
              const next = playlists.filter((playlist) => playlist.id !== active.id);
              commit(next, {
                enabled: false,
                active_playlist_id: next[0]?.id ?? "",
                current_index: 0,
              });
            }}>Delete</button>
          </div>

          {active && (
            <>
              <div className="show-name-row">
                <input defaultValue={active.name} maxLength={80} aria-label="Show name"
                  onBlur={(event) => updateActive((playlist) => ({
                    ...playlist,
                    name: event.target.value.trim() || playlist.name,
                  }))}
                />
                <label><input type="checkbox" checked={active.repeat}
                  onChange={(event) => updateActive((playlist) => ({ ...playlist, repeat: event.target.checked }))}
                /> Repeat forever</label>
              </div>

              {runningHere && show && (
                <div className="show-now">
                  <div><strong>Now: {show.scene_name}</strong><span>{show.index + 1} of {show.total} · next in {humanize(show.remaining_secs)}</span></div>
                  <div className="show-transport">
                    <button onClick={() => commit(playlists, { current_index: (activeIndex - 1 + active.entries.length) % active.entries.length })}>←</button>
                    <button onClick={() => commit(playlists, { current_index: (activeIndex + 1) % active.entries.length })}>Next →</button>
                  </div>
                  {progress > 0 && <i style={{ width: `${progress * 100}%` }} />}
                </div>
              )}

              <div className="show-cues">
                {active.entries.map((entry, index) => (
                  <div key={entry.id} className={`show-cue ${runningHere && show?.index === index ? "active" : ""}`}>
                    <span className="show-cue-number">{index + 1}</span>
                    <strong>{entry.name}</strong>
                    <label><span>minutes</span><input type="number" min={1} max={1440} step={1}
                      value={Math.round(entry.duration_secs / 60)}
                      onChange={(event) => updateActive((playlist) => ({ ...playlist, entries: playlist.entries.map((item) =>
                        item.id === entry.id ? { ...item, duration_secs: Math.max(60, Number(event.target.value) * 60) } : item) }))}
                    /></label>
                    <label><span>fade seconds</span><input type="number" min={0} max={300} step={1}
                      value={entry.transition_secs}
                      onChange={(event) => updateActive((playlist) => ({ ...playlist, entries: playlist.entries.map((item) =>
                        item.id === entry.id ? { ...item, transition_secs: Math.max(0, Number(event.target.value)) } : item) }))}
                    /></label>
                    <div className="show-cue-actions">
                      <button disabled={index === 0} onClick={() => move(index, -1)}>↑</button>
                      <button disabled={index === active.entries.length - 1} onClick={() => move(index, 1)}>↓</button>
                      <button className="danger" onClick={() => updateActive((playlist) => ({
                        ...playlist,
                        entries: playlist.entries.filter((item) => item.id !== entry.id),
                      }))}>×</button>
                    </div>
                  </div>
                ))}
              </div>

              <select className="show-add-scene" defaultValue="" onChange={(event) => {
                const value = event.target.value;
                const builtIn = SCENE_PRESETS.find((scene) => `built:${scene.id}` === value);
                const saved = config.saved_stacks.find((stack) => `saved:${stack.id}` === value);
                const stack = builtIn ? stackFromScene(builtIn) : saved;
                if (stack) updateActive((playlist) => ({ ...playlist, entries: [...playlist.entries, playlistEntry(stack)] }));
                event.target.value = "";
              }}>
                <option value="">＋ Add a scene…</option>
                <optgroup label="Built-in scenes">
                  {SCENE_PRESETS.map((scene) => <option key={scene.id} value={`built:${scene.id}`}>{scene.name}</option>)}
                </optgroup>
                {config.saved_stacks.length > 0 && <optgroup label="Your saved stacks">
                  {config.saved_stacks.map((stack) => <option key={stack.id} value={`saved:${stack.id}`}>{stack.name}</option>)}
                </optgroup>}
              </select>
            </>
          )}
        </>
      )}
    </div>
  );
}

function ScenesPanel() {
  const { client, config } = useGate();
  const [capturing, setCapturing] = useState(false);
  const [stackName, setStackName] = useState("");
  const [sceneSpeed, setSceneSpeedLocal] = useState(1);
  const setSceneSpeed = useThrottled((speed: number) => client.setMaster({ speed }));

  useEffect(() => {
    if (config) setSceneSpeedLocal(config.render.master_speed);
  }, [config?.render.master_speed]);

  if (!config) return null;

  const savedStacks = config.saved_stacks ?? [];
  const signature = JSON.stringify(config.layers);
  const active = SCENE_PRESETS.find(
    (scene) => JSON.stringify(scene.layers) === signature,
  );
  const activeSaved = savedStacks.find((stack) => JSON.stringify(stack.layers) === signature);

  const capture = (id: string, name: string): SavedStack => ({
    id,
    name,
    layers: config.layers.map((item) => ({ ...item })),
    master_speed: config.render.master_speed,
    walk_enabled: config.render.walk_enabled,
    walk_layers: config.render.walk_layers,
    walk_min_layers: config.render.walk_min_layers,
    walk_speed: config.render.walk_speed,
    walk_depth: config.render.walk_depth,
  });

  const saveCurrent = () => {
    const name = stackName.trim();
    if (!name) return;
    const id = `stack-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
    client.setConfig({ ...config, saved_stacks: [...savedStacks, capture(id, name)] });
    setStackName("");
    setCapturing(false);
  };

  const loadSaved = (stack: SavedStack) => {
    client.setConfig({
      ...config,
      render: {
        ...config.render,
        master_speed: stack.master_speed,
        walk_enabled: stack.walk_enabled,
        walk_layers: stack.walk_layers,
        walk_min_layers: stack.walk_min_layers,
        walk_speed: stack.walk_speed,
        walk_depth: stack.walk_depth,
      },
      layers: stack.layers.map((item) => ({ ...item })),
    });
  };

  const updateSaved = (stack: SavedStack) => {
    client.setConfig({
      ...config,
      saved_stacks: savedStacks.map((item) =>
        item.id === stack.id ? capture(stack.id, stack.name) : item,
      ),
    });
  };

  const deleteSaved = (stack: SavedStack) => {
    if (!window.confirm(`Delete saved stack “${stack.name}”?`)) return;
    client.setConfig({
      ...config,
      saved_stacks: savedStacks.filter((item) => item.id !== stack.id),
    });
  };

  const load = (scene: ScenePreset) => {
    client.setConfig({
      ...config,
      render: {
        ...config.render,
        walk_enabled: true,
        walk_layers: false,
        master_speed: scene.masterSpeed,
        walk_speed: scene.walkSpeed,
        walk_depth: scene.walkDepth,
      },
      beat_taps: { ...config.beat_taps, enabled: false },
      layers: scene.layers.map((item) => ({ ...item })),
    });
  };

  return (
    <section className="panel scene-library">
      <div className="scene-library-head">
        <div>
          <p className="eyebrow">Uprising studies</p>
          <h2>Saved scenes</h2>
        </div>
        <button
          className="primary"
          onClick={() => {
            const baseName = activeSaved?.name ?? active?.name;
            setStackName(baseName ? `${baseName} study` : `Stack ${savedStacks.length + 1}`);
            setCapturing(true);
          }}
        >
          ＋ Save current stack
        </button>
      </div>
      <ShowSchedulerPanel />
      <p className="scene-library-lede">
        Authored compositions translated from saved Uprising pieces. Loading one replaces
        the current layer stack; then every layer remains editable below. A restrained
        drift keeps the composition moving without changing which layers play.
      </p>
      <div className="scene-speed-control">
        <div className="scene-speed-copy">
          <strong>Scene speed</strong>
          <span>Scale every layer&apos;s motion without changing the composition.</span>
        </div>
        <div className="scene-speed-presets" aria-label="Scene speed presets">
          {([
            { value: 0.1, label: "Glacial" },
            { value: 0.25, label: "Slow" },
            { value: 0.5, label: "Drift" },
            { value: 1, label: "Normal" },
          ] as const).map((preset) => (
            <button
              key={preset.value}
              className={Math.abs(sceneSpeed - preset.value) < 0.01 ? "active" : ""}
              onClick={() => {
                setSceneSpeedLocal(preset.value);
                setSceneSpeed(preset.value);
              }}
            >
              {preset.label}<span>{preset.value}×</span>
            </button>
          ))}
        </div>
        <label className="scene-speed-slider">
          <input
            type="range"
            min={0.05}
            max={2}
            step={0.05}
            value={sceneSpeed}
            onChange={(event) => {
              const value = Number(event.target.value);
              setSceneSpeedLocal(value);
              setSceneSpeed(value);
            }}
          />
          <output>{sceneSpeed.toFixed(2)}×</output>
        </label>
      </div>
      {capturing && (
        <div className="stack-capture">
          <label>
            <span>Name this stack</span>
            <input
              autoFocus
              type="text"
              maxLength={80}
              value={stackName}
              onChange={(e) => setStackName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveCurrent();
                if (e.key === "Escape") setCapturing(false);
              }}
            />
          </label>
          <button className="primary" disabled={!stackName.trim()} onClick={saveCurrent}>Save</button>
          <button onClick={() => setCapturing(false)}>Cancel</button>
        </div>
      )}
      {savedStacks.length > 0 && (
        <div className="saved-stack-section">
          <div className="scene-section-title">
            <h3>Your stacks</h3>
            <span>{savedStacks.length} saved on Gate</span>
          </div>
          <div className="saved-stack-list">
            {savedStacks.map((stack) => {
              const selected = activeSaved?.id === stack.id;
              return (
                <article key={stack.id} className={`saved-stack-row ${selected ? "active" : ""}`}>
                  <div>
                    <strong>{stack.name}</strong>
                    <span>{stack.layers.length} layers · {stack.master_speed.toFixed(2)}× speed</span>
                  </div>
                  <div className="saved-stack-actions">
                    <button onClick={() => loadSaved(stack)}>{selected ? "Reload" : "Load"}</button>
                    <button onClick={() => updateSaved(stack)}>Update</button>
                    <button className="danger" onClick={() => deleteSaved(stack)} aria-label={`Delete ${stack.name}`}>×</button>
                  </div>
                </article>
              );
            })}
          </div>
        </div>
      )}
      <div className="scene-section-title built-ins">
        <h3>Starting points</h3>
        <span>{SCENE_PRESETS.length} built in</span>
      </div>
      <div className="scene-grid">
        {SCENE_PRESETS.map((scene) => {
          const selected = active?.id === scene.id;
          return (
            <article key={scene.id} className={`scene-card ${selected ? "active" : ""}`}>
              <div className="scene-palette" aria-label={`${scene.name} palette`}>
                {scene.palette.map((color) => <i key={color} style={{ background: color }} />)}
              </div>
              <p className="scene-source">{scene.source}</p>
              <h3>{scene.name}</h3>
              <p>{scene.description}</p>
              <div className="scene-card-foot">
                <span>{scene.layers.length} layers</span>
                <button
                  className={selected ? "active" : ""}
                  onClick={() => load(scene)}
                >
                  {selected ? "Reload scene" : "Load scene"}
                </button>
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

function BeatTapsPanel() {
  const { client, config } = useGate();
  const bt = config?.beat_taps;
  const commit = useThrottled((patch: Partial<NonNullable<typeof bt>>) => {
    if (config && bt) client.setConfig({ ...config, beat_taps: { ...bt, ...patch } });
  });
  if (!config || !bt) return null;
  return (
    <section className="panel">
      <h2>Beat taps</h2>
      <p className="hint">
        Fires a burst on every beat at a point that orbits the ring — like tapping the
        array in a circle on the beat. Spin sets how far it advances per beat; Drift
        slowly varies the spin so the spiral keeps changing character.
      </p>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={bt.enabled}
          onChange={(e) => commit({ enabled: e.target.checked })}
        />
        Enabled
        <span className="spacer" />
        <select
          value={bt.audio_source}
          onChange={(e) => commit({ audio_source: Number(e.target.value) })}
        >
          {config.audio.sources.map((s, i) => (
            <option key={i} value={i}>
              beat from: {s.id}
            </option>
          ))}
        </select>
      </label>
      <label className="slider-row">
        <span>Spin / beat</span>
        <input
          type="range"
          min={-0.25}
          max={0.25}
          step={0.005}
          value={bt.spin}
          onChange={(e) => commit({ spin: Number(e.target.value) })}
        />
        <span className="slider-val">
          {bt.spin === 0 ? "0" : `${(1 / Math.abs(bt.spin)).toFixed(0)} beats/lap${bt.spin < 0 ? " ↺" : " ↻"}`}
        </span>
      </label>
      <label className="slider-row">
        <span>Radius</span>
        <input
          type="range"
          min={0}
          max={1}
          step={0.02}
          value={bt.radius}
          onChange={(e) => commit({ radius: Number(e.target.value) })}
        />
        <span className="slider-val">{bt.radius.toFixed(2)}</span>
      </label>
      <label className="slider-row">
        <span>Intensity</span>
        <input
          type="range"
          min={0.1}
          max={1.5}
          step={0.05}
          value={bt.intensity}
          onChange={(e) => commit({ intensity: Number(e.target.value) })}
        />
        <span className="slider-val">{bt.intensity.toFixed(2)}</span>
      </label>
      <label className="toggle-row">
        <input
          type="checkbox"
          checked={bt.vary}
          onChange={(e) => commit({ vary: e.target.checked })}
        />
        Drift the spin speed over time
      </label>
      <label className="field-row" style={{ maxWidth: 260 }}>
        <span>Every Nth beat</span>
        <input
          type="number"
          min={1}
          max={16}
          value={bt.every}
          onChange={(e) => commit({ every: Math.max(1, Number(e.target.value) || 1) })}
        />
      </label>
    </section>
  );
}

function AutopilotForecast() {
  const { config } = useGate();
  if (!config) return null;
  const enabled = config.layers.filter((l) => l.enabled).length;
  const f = autopilotForecast(
    enabled,
    config.render.walk_min_layers,
    config.render.walk_speed,
    config.render.walk_layers,
  );
  return (
    <div className="forecast">
      <p className="hint">
        One walk step every ~{humanize(f.stepS)}.{" "}
        {f.tourS !== null && f.combos > 1 ? (
          <>
            {f.combos} combinations of your {enabled} enabled layers (min{" "}
            {Math.min(config.render.walk_min_layers, enabled)} on) — expect to tour them all in
            roughly <strong>{humanize(f.tourS)}</strong>.{" "}
          </>
        ) : null}
        The parameter walk itself never repeats — it drifts continuously with a ~
        {humanize(f.stepS)} memory, so the show is different every night.
      </p>
    </div>
  );
}

function LayerFader({ index, name }: { index: number; name: string }) {
  const { client, config } = useGate();
  const layer = config?.layers[index];
  const [value, setValue] = useState(layer?.opacity ?? 1);
  const [enabled, setEnabled] = useState(layer?.enabled ?? true);
  useEffect(() => {
    if (layer) {
      setValue(layer.opacity);
      setEnabled(layer.enabled);
    }
    // Only resync when the backend's values change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layer?.opacity, layer?.enabled]);
  const send = useThrottled((patch: { opacity?: number; enabled?: boolean }) => {
    if (layer) client.updateLayer(index, { ...layer, ...patch });
  });
  if (!layer) return null;
  return (
    <label className="slider-row">
      <input
        type="checkbox"
        checked={enabled}
        onChange={(e) => {
          setEnabled(e.target.checked);
          send({ enabled: e.target.checked, opacity: value });
        }}
      />
      <span>{name}</span>
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={value}
        onChange={(e) => {
          const v = Number(e.target.value);
          setValue(v);
          send({ opacity: v, enabled });
        }}
      />
      <span className="slider-val">{value.toFixed(2)}</span>
    </label>
  );
}
