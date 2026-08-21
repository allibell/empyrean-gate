// WebSocket client for the Gate backend. Used identically by the Tauri webview,
// LAN browsers, and phones. Text = JSON protocol; binary = preview frames.

import type { EffectCfg, LayerCfg, AppConfig, PreviewFrame, ServerMsg } from "./types";

const PREVIEW_MAGIC = 0x45475056;
const VIDEO_FRAME_MAGIC = 0x45475646;

export interface ResolvedMedia {
  playbackUrl: string;
  title: string;
  sourceUrl: string;
  resolvedBy: string;
}

type Listener = (msg: ServerMsg) => void;
type FrameListener = (frame: PreviewFrame) => void;
type StatusListener = (connected: boolean) => void;

const DEV_BACKEND_KEY = "empyrean-dev-backend-port";
const DEV_BACKEND_PORTS = Array.from({ length: 10 }, (_, index) => 9520 + index);

function validPort(value: string | null): number | null {
  if (!value || !/^\d+$/.test(value)) return null;
  const port = Number(value);
  return port >= 1 && port <= 65535 ? port : null;
}

async function backendIsAvailable(hostname: string, port: number): Promise<boolean> {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 350);
  try {
    const response = await fetch(`http://${hostname}:${port}/handover/state`, {
      cache: "no-store",
      signal: controller.signal,
    });
    return response.ok;
  } catch {
    return false;
  } finally {
    window.clearTimeout(timeout);
  }
}

async function resolveDevBackendPort(): Promise<number> {
  const queryPort = validPort(new URLSearchParams(location.search).get("backend"));
  if (queryPort !== null) return queryPort;

  const remembered = validPort(localStorage.getItem(DEV_BACKEND_KEY));
  if (remembered !== null && await backendIsAvailable(location.hostname, remembered)) {
    return remembered;
  }

  const candidates = DEV_BACKEND_PORTS.filter((port) => port !== remembered);
  const availability = await Promise.all(
    candidates.map((port) => backendIsAvailable(location.hostname, port)),
  );
  const found = candidates.find((_port, index) => availability[index]);
  return found ?? remembered ?? 9520;
}

async function resolveBase(): Promise<{ http: string; ws: string }> {
  // Inside the Tauri webview, ask the shell which port the backend bound.
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
  if (w.__TAURI_INTERNALS__) {
    const { invoke } = await import("@tauri-apps/api/core");
    const info = (await invoke("backend_info")) as { wsPort: number };
    return { http: `http://127.0.0.1:${info.wsPort}`, ws: `ws://127.0.0.1:${info.wsPort}/ws` };
  }
  // Independent Vite worktrees may each use a different backend. An explicit
  // ?backend=9521 wins; otherwise discover a live local Gate and remember it.
  if (import.meta.env.DEV) {
    const port = await resolveDevBackendPort();
    return { http: `http://${location.hostname}:${port}`, ws: `ws://${location.hostname}:${port}/ws` };
  }
  // Served by the backend itself.
  const proto = location.protocol === "https:" ? "wss" : "ws";
  return { http: location.origin, ws: `${proto}://${location.host}/ws` };
}

/// On (re)connect, detect a UI bundle older than what the backend now serves and
/// refresh. Vite content-hashes bundle filenames, so comparing the entry script name
/// in the freshly-fetched index.html against the one this page loaded is exact.
/// Meaningless in the Tauri webview (assets ship with the binary) and vite dev (HMR).
async function reloadIfStale(): Promise<void> {
  if ("__TAURI_INTERNALS__" in window || import.meta.env.DEV) return;
  try {
    const res = await fetch("/index.html", { cache: "no-store" });
    if (!res.ok) return;
    const served = (await res.text()).match(/\/assets\/index-[\w-]+\.js/)?.[0];
    const current = document
      .querySelector('script[src*="/assets/index-"]')
      ?.getAttribute("src");
    if (!served || !current || served === current) return;
    // One reload per target bundle — never loop even if something still serves stale.
    if (sessionStorage.getItem("empyrean-reloaded-for") === served) return;
    sessionStorage.setItem("empyrean-reloaded-for", served);
    location.reload();
  } catch {
    // Offline or backend restarting; the next reconnect will check again.
  }
}

export class GateClient {
  private ws: WebSocket | null = null;
  private listeners = new Set<Listener>();
  private frameListeners = new Set<FrameListener>();
  private statusListeners = new Set<StatusListener>();
  private deniedListeners = new Set<(reason: string) => void>();
  private closed = false;
  private retryMs = 500;
  private videoSequence = 0;
  clientId: string;
  /** HTTP origin of the backend (for /qr.svg etc.); set once connect() resolves it. */
  httpBase = "";

  constructor() {
    this.clientId = localStorage.getItem("empyrean-client-id") ?? this.newClientId();
    // Arriving via a scanned connect QR: stash the join token, clean the URL.
    const params = new URLSearchParams(location.search);
    const join = params.get("join");
    if (join) {
      localStorage.setItem("empyrean-join-token", join);
      params.delete("join");
      const query = params.size > 0 ? `?${params.toString()}` : "";
      history.replaceState(null, "", location.pathname + query + location.hash);
    }
  }

  get deviceName(): string {
    return localStorage.getItem("empyrean-client-name") ?? "";
  }

  setDeviceName(name: string) {
    localStorage.setItem("empyrean-client-name", name);
    this.send({ type: "set_client_name", name });
  }

  private newClientId(): string {
    const id = `client-${Math.random().toString(36).slice(2, 8)}`;
    localStorage.setItem("empyrean-client-id", id);
    return id;
  }

  async connect(): Promise<void> {
    if (this.closed) return;
    const base = await resolveBase();
    this.httpBase = base.http;
    const ws = new WebSocket(base.ws);
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      if (import.meta.env.DEV) {
        const port = validPort(new URL(base.http).port);
        if (port !== null) localStorage.setItem(DEV_BACKEND_KEY, String(port));
      }
      this.retryMs = 500;
      void reloadIfStale();
      this.send({
        type: "hello",
        name: this.deviceName,
        client_id: this.clientId,
        token: localStorage.getItem("empyrean-join-token") ?? "",
      });
      this.statusListeners.forEach((l) => l(true));
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        const msg = JSON.parse(ev.data) as ServerMsg;
        if (msg.type === "denied") {
          // Refused: stop hammering the server with reconnects.
          this.closed = true;
          this.deniedListeners.forEach((l) => l(msg.reason));
        }
        this.listeners.forEach((l) => l(msg));
      } else {
        const frame = parsePreview(ev.data as ArrayBuffer);
        if (frame) this.frameListeners.forEach((l) => l(frame));
      }
    };
    ws.onclose = () => {
      this.statusListeners.forEach((l) => l(false));
      if (!this.closed) {
        setTimeout(() => void this.connect(), this.retryMs);
        this.retryMs = Math.min(this.retryMs * 2, 5000);
      }
    };
    ws.onerror = () => ws.close();
  }

  close() {
    this.closed = true;
    this.ws?.close();
  }

  onMessage(l: Listener): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  onFrame(l: FrameListener): () => void {
    this.frameListeners.add(l);
    return () => this.frameListeners.delete(l);
  }

  onStatus(l: StatusListener): () => void {
    this.statusListeners.add(l);
    return () => this.statusListeners.delete(l);
  }

  onDenied(l: (reason: string) => void): () => void {
    this.deniedListeners.add(l);
    return () => this.deniedListeners.delete(l);
  }

  send(msg: Record<string, unknown>) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  async resolveMedia(url: string): Promise<ResolvedMedia> {
    const response = await fetch(`${this.httpBase}/media/resolve`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        url,
        client_id: this.clientId,
        token: localStorage.getItem("empyrean-join-token") ?? "",
      }),
    });
    if (!response.ok) throw new Error((await response.text()) || `Media resolver returned ${response.status}`);
    const value = (await response.json()) as {
      playback_url: string;
      title: string;
      source_url: string;
      resolved_by: string;
    };
    return {
      playbackUrl: new URL(value.playback_url, `${this.httpBase}/`).toString(),
      title: value.title,
      sourceUrl: value.source_url,
      resolvedBy: value.resolved_by,
    };
  }

  startVideo(title: string, sourceUrl: string) {
    this.videoSequence = 0;
    this.send({ type: "start_video", title, source_url: sourceUrl });
  }

  stopVideo(force = false) {
    this.send({ type: "stop_video", force });
  }

  /** Sends one RGBA8 frame if the socket is keeping up; otherwise drops it. */
  sendVideoFrame(width: number, height: number, rgba: Uint8ClampedArray): boolean {
    const ws = this.ws;
    const size = width * height * 4;
    if (
      !ws ||
      ws.readyState !== WebSocket.OPEN ||
      rgba.byteLength !== size ||
      ws.bufferedAmount > size * 2
    ) {
      return false;
    }
    const packet = new Uint8Array(12 + size);
    const header = new DataView(packet.buffer);
    header.setUint32(0, VIDEO_FRAME_MAGIC, true);
    header.setUint32(4, this.videoSequence++, true);
    header.setUint16(8, width, true);
    header.setUint16(10, height, true);
    packet.set(rgba, 12);
    ws.send(packet);
    return true;
  }

  // --- convenience wrappers ---
  setConfig(config: AppConfig) {
    this.send({ type: "set_config", config });
  }
  setMaster(v: { brightness?: number; speed?: number }) {
    this.send({ type: "set_master", ...v });
  }
  setSacnEnabled(enabled: boolean) {
    this.send({ type: "set_sacn_enabled", enabled });
  }
  addLayer(layer: LayerCfg) {
    this.send({ type: "add_layer", layer });
  }
  updateLayer(index: number, layer: LayerCfg) {
    this.send({ type: "update_layer", index, layer });
  }
  removeLayer(index: number) {
    this.send({ type: "remove_layer", index });
  }
  moveLayer(from: number, to: number) {
    this.send({ type: "move_layer", from, to });
  }
  triggerEffect(effect: Partial<EffectCfg> & { kind: EffectCfg["kind"] }) {
    this.send({
      type: "trigger_effect",
      effect: {
        angle: 0,
        radius: 1,
        intensity: 1,
        size: 1,
        hue: -1,
        duration: 0,
        ...effect,
      },
    });
  }
  subscribePreview(fps: number, decimate: number) {
    this.send({ type: "subscribe_preview", fps, decimate });
  }
  sendAudioFrame(
    f: { level: number; bass: number; mid: number; treble: number; flux: number },
    stream: "microphone" | "video" = "microphone",
  ) {
    this.send({ type: "audio_frame", stream, ...f });
  }
  sendImu(f: { yaw: number; pitch: number; roll: number; shake: number }) {
    this.send({ type: "imu", ...f });
  }
}

function parsePreview(buf: ArrayBuffer): PreviewFrame | null {
  if (buf.byteLength < 12) return null;
  const view = new DataView(buf);
  if (view.getUint32(0, true) !== PREVIEW_MAGIC) return null;
  const frameNumber = view.getUint32(4, true);
  const spokes = view.getUint16(8, true);
  const pixels = view.getUint16(10, true);
  const rgb = new Uint8Array(buf, 12);
  if (rgb.length < spokes * pixels * 3) return null;
  return { frameNumber, spokes, pixels, rgb };
}
