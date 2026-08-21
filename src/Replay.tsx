import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type InputHTMLAttributes,
} from "react";
import GateCanvas, { type GatePreviewSource } from "./GateCanvas";
import type { PreviewFrame, PreviewMeta } from "./types";

const SPOKES = 64;
const PIXELS = 378;
const FRAME_BYTES = SPOKES * PIXELS * 3;
const RECENT_DB = "empyrean-replays";
const RECENT_STORE = "clips";
const MAX_RECENT = 6;
const DIRECTORY_INPUT_PROPS = {
  webkitdirectory: "",
  directory: "",
} as InputHTMLAttributes<HTMLInputElement>;

const UPRISING_META: PreviewMeta = {
  spokes: SPOKES,
  pixels: PIXELS,
  decimate: 1,
  outer_radius_ft: 1,
  inner_radius_ft: 0.2416,
};

class ReplaySource implements GatePreviewSource {
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

interface PersistentFileHandle extends FileSystemFileHandle {
  queryPermission: (options?: { mode?: "read" | "readwrite" }) => Promise<PermissionState>;
  requestPermission: (options?: { mode?: "read" | "readwrite" }) => Promise<PermissionState>;
}

interface StoredClip {
  key: string;
  name: string;
  size: number;
  lastModified: number;
  handle: PersistentFileHandle;
  usedAt: number;
}

interface RecentClip extends Omit<StoredClip, "handle"> {
  handle?: PersistentFileHandle;
  file?: File;
  label?: string;
}

interface FilePickerWindow extends Window {
  showOpenFilePicker?: (options?: {
    multiple?: boolean;
    types?: { description?: string; accept: Record<string, string[]> }[];
  }) => Promise<PersistentFileHandle[]>;
}

interface SharedFixture {
  name: string;
  size: number;
  url: string;
}

let sessionClip: Clip | null = null;
let sessionRecents: RecentClip[] = [];

function clipKey(file: File): string {
  return `${file.webkitRelativePath || file.name}:${file.size}:${file.lastModified}`;
}

function openReplayDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(RECENT_DB, 2);
    request.onupgradeneeded = () => {
      if (request.result.objectStoreNames.contains(RECENT_STORE)) {
        request.result.deleteObjectStore(RECENT_STORE);
      }
      request.result.createObjectStore(RECENT_STORE, { keyPath: "key" });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function storedReplays(): Promise<StoredClip[]> {
  const db = await openReplayDb();
  return new Promise((resolve, reject) => {
    const request = db.transaction(RECENT_STORE, "readonly").objectStore(RECENT_STORE).getAll();
    request.onsuccess = () => {
      db.close();
      resolve((request.result as StoredClip[]).sort((a, b) => b.usedAt - a.usedAt));
    };
    request.onerror = () => {
      db.close();
      reject(request.error);
    };
  });
}

async function rememberReplay(file: File, handle?: PersistentFileHandle): Promise<StoredClip[]> {
  if (!handle) return storedReplays();
  const db = await openReplayDb();
  const existing = await new Promise<StoredClip[]>((resolve, reject) => {
    const request = db.transaction(RECENT_STORE, "readonly").objectStore(RECENT_STORE).getAll();
    request.onsuccess = () => resolve(request.result as StoredClip[]);
    request.onerror = () => reject(request.error);
  });
  const next: StoredClip = {
    key: clipKey(file),
    name: file.name,
    size: file.size,
    lastModified: file.lastModified,
    handle,
    usedAt: Date.now(),
  };
  const keep = [next, ...existing.filter((item) => item.key !== next.key)]
    .sort((a, b) => b.usedAt - a.usedAt)
    .slice(0, MAX_RECENT);
  await new Promise<void>((resolve, reject) => {
    const tx = db.transaction(RECENT_STORE, "readwrite");
    const store = tx.objectStore(RECENT_STORE);
    store.clear();
    keep.forEach((item) => store.put(item));
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
  db.close();
  return keep;
}

function timeLabel(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00.0";
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${(seconds % 60).toFixed(1).padStart(4, "0")}`;
}

export default function Replay() {
  const source = useMemo(() => new ReplaySource(), []);
  const [clip, setClip] = useState<Clip | null>(sessionClip);
  const [recent, setRecent] = useState<RecentClip[]>(sessionRecents);
  const [library, setLibrary] = useState<RecentClip[]>([]);
  const [libraryKey, setLibraryKey] = useState("");
  const [playing, setPlaying] = useState(false);
  const [loop, setLoop] = useState(true);
  const [fps, setFps] = useState(30);
  const [frame, setFrameState] = useState(0);
  const [error, setError] = useState("");
  const [fixtures, setFixtures] = useState<SharedFixture[]>([]);
  const [archiveDirectory, setArchiveDirectory] = useState("");
  const [loadingFixture, setLoadingFixture] = useState("");
  const frameRef = useRef(0);
  const requestRef = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const directoryRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!import.meta.env.DEV) return;
    let cancelled = false;
    void fetch("/__empyrean/uprising")
      .then((response) => response.ok ? response.json() : Promise.reject(new Error("Archive unavailable")))
      .then((value: { directory?: string; fixtures?: SharedFixture[] }) => {
        if (cancelled) return;
        setFixtures(value.fixtures ?? []);
        setArchiveDirectory(value.directory ?? "");
      })
      .catch(() => undefined);
    return () => { cancelled = true; };
  }, []);

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

  const openFile = useCallback((file: File, handle?: PersistentFileHandle) => {
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
    const next = { file, frames: file.size / FRAME_BYTES };
    sessionClip = next;
    const recentItem: RecentClip = {
      key: clipKey(file),
      name: file.name,
      size: file.size,
      lastModified: file.lastModified,
      handle,
      file,
      usedAt: Date.now(),
    };
    sessionRecents = [recentItem, ...sessionRecents.filter((item) => item.key !== recentItem.key)]
      .slice(0, MAX_RECENT);
    setRecent((current) =>
      [recentItem, ...current.filter((item) => item.key !== recentItem.key)].slice(0, MAX_RECENT),
    );
    setClip(next);
    setPlaying(true);
    void rememberReplay(file, handle)
      .then((stored) => setRecent(
        [...sessionRecents, ...stored.filter((item) => !sessionRecents.some((s) => s.key === item.key))]
          .slice(0, MAX_RECENT),
      ))
      .catch(() => undefined);
  }, []);

  const chooseFile = useCallback(async () => {
    const picker = (window as FilePickerWindow).showOpenFilePicker;
    if (!picker) {
      inputRef.current?.click();
      return;
    }
    try {
      const [handle] = await picker.call(window, {
        multiple: false,
        types: [{
          description: "Uprising RGB recording",
          accept: { "application/octet-stream": [".data"] },
        }],
      });
      if (handle) openFile(await handle.getFile(), handle);
    } catch (reason) {
      if (reason instanceof DOMException && reason.name === "AbortError") return;
      setError(`Could not open recording: ${String(reason)}`);
    }
  }, [openFile]);

  const indexDirectory = useCallback(async (files: FileList | null) => {
    if (!files) return;
    setError("");
    const all = Array.from(files);
    const titles = new Map<string, string>();
    const index = all.find((file) => /(^|\/)media\/index\.json$/i.test(file.webkitRelativePath));
    if (index) {
      try {
        const parsed = JSON.parse(await index.text()) as {
          files?: { egFramesFile?: string; title?: string }[];
        };
        parsed.files?.forEach((item) => {
          if (item.egFramesFile && item.title) titles.set(item.egFramesFile, item.title);
        });
      } catch {
        // Filenames remain usable when a checkout has no readable index.
      }
    }
    const recordings: RecentClip[] = all
      .filter((file) => /\.eg\.data$/i.test(file.name))
      .filter((file) => file.size >= FRAME_BYTES && file.size % FRAME_BYTES === 0)
      .map((file) => ({
        key: clipKey(file),
        name: file.name,
        label: titles.get(file.name),
        size: file.size,
        lastModified: file.lastModified,
        file,
        usedAt: 0,
      }))
      .sort((a, b) => (a.label ?? a.name).localeCompare(b.label ?? b.name));
    setLibrary(recordings);
    setLibraryKey(recordings[0]?.key ?? "");
    if (recordings.length === 0) {
      const pointers = all.filter((file) => /\.eg\.data$/i.test(file.name) && file.size < 1024).length;
      setError(pointers > 0
        ? "This checkout contains Git LFS pointer files. Run `git lfs pull` in Uprising-Data, then choose it again."
        : "No complete 64×378 Uprising recordings were found in that folder.");
    }
  }, []);

  const openRecent = useCallback(async (item: RecentClip) => {
    try {
      if (item.file) {
        openFile(item.file, item.handle);
        return;
      }
      if (!item.handle) {
        setError(`${item.name} is no longer available; choose it again to restore the reference.`);
        return;
      }
      const permission = await item.handle.queryPermission({ mode: "read" });
      const granted = permission === "granted"
        || await item.handle.requestPermission({ mode: "read" }) === "granted";
      if (!granted) {
        setError(`Read access was not granted for ${item.name}.`);
        return;
      }
      openFile(await item.handle.getFile(), item.handle);
    } catch (reason) {
      setError(`Could not reopen ${item.name}: ${String(reason)}`);
    }
  }, [openFile]);

  useEffect(() => {
    void storedReplays().then((stored) => setRecent(
      [...sessionRecents, ...stored.filter((item) => !sessionRecents.some((s) => s.key === item.key))]
        .slice(0, MAX_RECENT),
    )).catch(() => undefined);
  }, []);

  const openSharedFixture = useCallback(async (fixture: SharedFixture) => {
    setError("");
    setLoadingFixture(fixture.name);
    try {
      const response = await fetch(fixture.url);
      if (!response.ok) throw new Error(`Archive returned ${response.status}`);
      openFile(new File([await response.blob()], fixture.name, { type: "application/octet-stream" }));
    } catch (reason) {
      setError(`Could not open ${fixture.name}: ${reason instanceof Error ? reason.message : reason}`);
    } finally {
      setLoadingFixture("");
    }
  }, [openFile]);

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
          <button className="replay-drop" onClick={() => void chooseFile()}>
            <strong>Open an Uprising recording</strong>
            <span>Drop or choose a raw .eg.data file</span>
            <span className="hint">64 spokes × 378 pixels × RGB, streamed from disk</span>
          </button>
        )}
      </section>

      <aside className="replay-panel">
        <div>
          <div className="eyebrow">Archived show playback</div>
          <h2>Uprising Replay</h2>
          <p className="hint">
            Plays the exact RGB frames used by the original show software. Files stay local
            and are read one frame at a time, so long recordings do not fill memory.
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
        <input
          {...DIRECTORY_INPUT_PROPS}
          ref={directoryRef}
          className="visually-hidden"
          aria-hidden="true"
          tabIndex={-1}
          type="file"
          multiple
          onChange={(event) => void indexDirectory(event.target.files)}
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
          <button onClick={() => void chooseFile()}>
            {clip ? "Choose another file" : "Choose file"}
          </button>
          <span>{clip?.file.name ?? "or drop a file here"}</span>
        </div>

        <button className="replay-folder-button" onClick={() => directoryRef.current?.click()}>
          Choose Uprising-Data folder
        </button>

        {library.length > 0 && (
          <div className="replay-library">
            <div className="eyebrow">Uprising library · {library.length} recordings</div>
            <div className="replay-library-picker">
              <select value={libraryKey} onChange={(event) => setLibraryKey(event.target.value)}>
                {library.map((item) => (
                  <option key={item.key} value={item.key}>
                    {item.label ?? item.name.replace(/\.eg\.data$/i, "")} · {(item.size / 1_048_576).toFixed(1)} MB
                  </option>
                ))}
              </select>
              <button onClick={() => {
                const item = library.find((candidate) => candidate.key === libraryKey);
                if (item) void openRecent(item);
              }}>Load</button>
            </div>
          </div>
        )}

        {recent.length > 0 && (
          <div className="replay-recents">
            <div className="eyebrow">Recent replays</div>
            <div className="replay-recent-list">
              {recent.map((item) => (
                <button key={item.key}
                  className={clip && clipKey(clip.file) === item.key ? "active" : ""}
                  onClick={() => void openRecent(item)}
                >
                  <strong>{item.name.replace(/\.eg\.data$/i, "")}</strong>
                  <span>{(item.size / 1_048_576).toFixed(1)} MB</span>
                </button>
              ))}
            </div>
            <span className="hint">Only local file references are saved; recordings are never copied.</span>
          </div>
        )}

        {fixtures.length > 0 && (
          <div className="replay-library">
            <div className="eyebrow">Shared Uprising archive</div>
            {fixtures.map((fixture) => (
              <button key={fixture.name} onClick={() => void openSharedFixture(fixture)}>
                <strong>{fixture.name.replace(/\.eg\.data$/i, "")}</strong>
                <span>{(fixture.size / 1_048_576).toFixed(1)} MB</span>
                <span>{loadingFixture === fixture.name ? "Opening…" : "Open"}</span>
              </button>
            ))}
          </div>
        )}

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
                Loop recording
              </label>
            </div>
          </>
        )}

        {import.meta.env.DEV && (
          <div className="replay-note">
            <strong>Optional shared development sample</strong>
            <code>bun run demo:uprising</code>
            <span className="hint">
              Saved for every worktree in {archiveDirectory || "the shared Empyrean data directory"}.
            </span>
          </div>
        )}
      </aside>
    </div>
  );
}
