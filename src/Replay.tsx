import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import GateCanvas, { type GatePreviewSource } from "./GateCanvas";
import type { PreviewFrame, PreviewMeta } from "./types";

const SPOKES = 64;
const PIXELS = 378;
const FRAME_BYTES = SPOKES * PIXELS * 3;

const UPRISING_META: PreviewMeta = {
  spokes: SPOKES,
  pixels: PIXELS,
  decimate: 1,
  outer_radius_ft: 1,
  inner_radius_ft: 0.2416,
};

class FixtureSource implements GatePreviewSource {
  meta = UPRISING_META;
  pixel0AtInner = true;
  private listeners = new Set<(frame: PreviewFrame) => void>();

  subscribe = (listener: (frame: PreviewFrame) => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  emit(frame: PreviewFrame) {
    this.listeners.forEach((listener) => listener(frame));
  }
}

interface Clip {
  file: File;
  frames: number;
}

function timeLabel(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00.0";
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${(seconds % 60).toFixed(1).padStart(4, "0")}`;
}

/**
 * Development-only fixture viewer for checking old Uprising RGB output against
 * the current stage preview. App.tsx deliberately keeps this out of production
 * navigation and only accepts #replay in a Vite development build.
 */
export default function Replay() {
  const source = useMemo(() => new FixtureSource(), []);
  const [clip, setClip] = useState<Clip | null>(null);
  const [playing, setPlaying] = useState(false);
  const [loop, setLoop] = useState(true);
  const [fps, setFps] = useState(30);
  const [frame, setFrameState] = useState(0);
  const [error, setError] = useState("");
  const frameRef = useRef(0);
  const requestRef = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const showFrame = useCallback(async (index: number) => {
    if (!clip) return;
    const clamped = Math.max(0, Math.min(clip.frames - 1, Math.floor(index)));
    const request = ++requestRef.current;
    const start = clamped * FRAME_BYTES;
    const rgb = new Uint8Array(await clip.file.slice(start, start + FRAME_BYTES).arrayBuffer());
    if (request !== requestRef.current) return;
    frameRef.current = clamped;
    setFrameState(clamped);
    source.emit({ frameNumber: clamped, spokes: SPOKES, pixels: PIXELS, rgb });
  }, [clip, source]);

  const openFile = useCallback((file: File) => {
    setError("");
    if (file.size < FRAME_BYTES || file.size % FRAME_BYTES !== 0) {
      setPlaying(false);
      setClip(null);
      setError(
        `${file.name} is not a 64×378 RGB fixture: its size must be an exact ` +
          `multiple of ${FRAME_BYTES.toLocaleString()} bytes.`,
      );
      return;
    }
    frameRef.current = 0;
    setFrameState(0);
    setClip({ file, frames: file.size / FRAME_BYTES });
    setPlaying(true);
  }, []);

  useEffect(() => {
    if (clip) void showFrame(0);
  }, [clip, showFrame]);

  useEffect(() => {
    if (!playing || !clip) return;
    let animation = 0;
    const startedAt = performance.now();
    const startedFrame = frameRef.current;
    let lastRequested = startedFrame - 1;

    const tick = (now: number) => {
      let target = startedFrame + Math.floor(((now - startedAt) * fps) / 1000);
      if (target >= clip.frames) {
        if (loop) target %= clip.frames;
        else {
          void showFrame(clip.frames - 1);
          setPlaying(false);
          return;
        }
      }
      if (target !== lastRequested) {
        lastRequested = target;
        void showFrame(target);
      }
      animation = requestAnimationFrame(tick);
    };
    animation = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(animation);
  }, [clip, fps, loop, playing, showFrame]);

  const duration = clip ? clip.frames / fps : 0;

  return (
    <div className="replay-page">
      <section className="replay-stage">
        <GateCanvas previewSource={source} />
        {!clip && (
          <button className="replay-drop" onClick={() => inputRef.current?.click()}>
            <strong>Open an Uprising frame fixture</strong>
            <span>Drop or choose a raw .eg.data file</span>
            <span className="hint">Development tool · 64 spokes × 378 pixels × RGB</span>
          </button>
        )}
      </section>

      <aside className="replay-panel">
        <div>
          <div className="eyebrow">Development fixture</div>
          <h2>Uprising frame check</h2>
          <p className="hint">
            Compares old show output with the current point renderer. The file stays local
            and is read one frame at a time.
          </p>
        </div>

        <input
          ref={inputRef}
          className="visually-hidden"
          aria-hidden="true"
          tabIndex={-1}
          type="file"
          accept=".data,.eg.data,application/octet-stream"
          onChange={(event) => {
            const file = event.target.files?.[0];
            if (file) openFile(file);
          }}
        />
        <div
          className="replay-file-drop"
          onDragOver={(event) => event.preventDefault()}
          onDrop={(event) => {
            event.preventDefault();
            const file = event.dataTransfer.files[0];
            if (file) openFile(file);
          }}
        >
          <button onClick={() => inputRef.current?.click()}>
            {clip ? "Choose another fixture" : "Choose fixture"}
          </button>
          <span>{clip?.file.name ?? "or drop a file here"}</span>
        </div>

        {error && <div className="replay-error">{error}</div>}

        {clip && (
          <>
            <div className="replay-transport">
              <button className="transport-main" onClick={() => setPlaying((value) => !value)}>
                {playing ? "Pause" : frame >= clip.frames - 1 ? "Replay" : "Play"}
              </button>
              <button onClick={() => {
                setPlaying(false);
                void showFrame(0);
              }}>
                Restart
              </button>
            </div>

            <label className="replay-scrubber">
              <input
                type="range"
                min={0}
                max={Math.max(0, clip.frames - 1)}
                value={frame}
                onChange={(event) => {
                  setPlaying(false);
                  void showFrame(Number(event.target.value));
                }}
              />
              <span>{timeLabel(frame / fps)}</span>
              <span>{timeLabel(duration)}</span>
            </label>

            <div className="replay-stats">
              <div><span>Frames</span><strong>{clip.frames.toLocaleString()}</strong></div>
              <div><span>Pixels</span><strong>24,192</strong></div>
              <div><span>Size</span><strong>{(clip.file.size / 1_048_576).toFixed(1)} MB</strong></div>
            </div>

            <div className="replay-options">
              <label>
                Playback speed
                <select value={fps} onChange={(event) => setFps(Number(event.target.value))}>
                  <option value={3}>0.1× · 3 FPS</option>
                  <option value={7.5}>0.25× · 7.5 FPS</option>
                  <option value={15}>0.5× · 15 FPS</option>
                  <option value={24}>0.8× · 24 FPS</option>
                  <option value={30}>1× · 30 FPS</option>
                  <option value={45}>1.5× · 45 FPS</option>
                  <option value={60}>2× · 60 FPS</option>
                </select>
              </label>
              <label className="check-row">
                <input type="checkbox" checked={loop} onChange={(event) => setLoop(event.target.checked)} />
                Loop fixture
              </label>
            </div>
          </>
        )}

        <div className="replay-note">
          <strong>Fetch the local fixture</strong>
          <code>bun run demo:uprising</code>
          <span className="hint">Then choose it from demo-data/uprising/.</span>
        </div>
      </aside>
    </div>
  );
}
