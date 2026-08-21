import { useEffect, useState } from "react";
import Control from "./Control";
import { EFFECTS } from "./effects";
import Live from "./Live";
import { loadSelectedLiveColor } from "./liveColors";
import Media from "./Media";
import Replay from "./Replay";
import Settings from "./Settings";
import { useGate } from "./state";

const TABS = [
  { id: "live", label: "Live" },
  { id: "media", label: "Media" },
  { id: "replay", label: "Archive" },
  { id: "control", label: "Control" },
  { id: "settings", label: "Settings" },
] as const;

const NAV_TABS: ReadonlyArray<{ id: TabId; label: string }> = TABS;

type TabId = (typeof TABS)[number]["id"];

function tabFromHash(): TabId {
  const h = location.hash.replace("#", "");
  // Old bookmarks / PWA shortcuts used #view and #draw; both merged into Live.
  if (h === "view" || h === "draw") return "live";
  return (TABS.find((t) => t.id === h)?.id ?? "live") as TabId;
}

const IN_TAURI = "__TAURI_INTERNALS__" in window;

async function openNewWindow(tab: TabId) {
  // Rust creates it with a stable label (aux-<tab>) so its geometry persists and
  // it is recreated after restarts/self-updates; re-invoking focuses the existing.
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("open_aux", { tab });
}

/// Fullscreen overlay while the backend is unreachable. Appears after a short
/// grace period (so sub-second blips never flash it) and dismisses itself the
/// moment the connection returns.
function DisconnectedOverlay({ disabled = false }: { disabled?: boolean }) {
  const { connected } = useGate();
  const [visible, setVisible] = useState(false);
  const [since, setSince] = useState<number | null>(null);
  const [, forceTick] = useState(0);

  useEffect(() => {
    if (connected) {
      setVisible(false);
      setSince(null);
      return;
    }
    const started = Date.now();
    setSince(started);
    const grace = setTimeout(() => setVisible(true), 2000);
    const tick = setInterval(() => forceTick((n) => n + 1), 1000);
    return () => {
      clearTimeout(grace);
      clearInterval(tick);
    };
  }, [connected]);

  if (disabled || !visible || connected) return null;
  const secs = since ? Math.floor((Date.now() - since) / 1000) : 0;
  return (
    <div className="disconnected-overlay">
      <div className="disconnected-box">
        <div className="disconnected-spinner" />
        <h1>Backend unreachable</h1>
        <p>
          Lost the connection to the Empyrean Gate backend
          {secs >= 5 ? ` ${secs} seconds ago` : ""}. Reconnecting automatically — this
          message will disappear as soon as it&apos;s back.
        </p>
        <p className="hint">
          If it doesn&apos;t come back: is the Gate app (or headless backend) running? Are
          you on the same network?
        </p>
      </div>
    </div>
  );
}

/// Small version tag in the corner. Click checks for updates; when a newer
/// release is known it lights up and a click hot-swaps to it (seamless takeover).
function VersionChip() {
  const { client, status } = useGate();
  const [busy, setBusy] = useState(false);
  if (!status?.version) return null;
  const next = status.update_available;
  const note = status.update_state;

  if (next) {
    return (
      <button
        className="version-chip update"
        onClick={() => {
          setBusy(true);
          client.send({ type: "install_update" });
        }}
      >
        v{status.version} → v{next}
        {busy || note ? ` · ${note || "updating…"}` : " · click to update"}
      </button>
    );
  }
  return (
    <button
      className="version-chip"
      onClick={() => client.send({ type: "check_update" })}
    >
      v{status.version}
      {note ? ` · ${note}` : ""}
    </button>
  );
}

function ConnectModal({ onClose }: { onClose: () => void }) {
  const { client, config, status } = useGate();
  const interfaces = status?.interfaces ?? [];
  const [ip, setIp] = useState<string>("");
  const chosen = ip || interfaces[0]?.split("—").pop()?.trim() || "";
  const port = config?.server.port ?? 9520;
  const url = `http://${chosen}:${port}/?join=${config?.server.join_token ?? ""}`;
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Connect a device</h2>
        <p className="hint">Scan from a phone/iPad on the same network, then Add to Home Screen.</p>
        {interfaces.length > 1 && (
          <select value={chosen} onChange={(e) => setIp(e.target.value)}>
            {interfaces.map((i) => {
              const addr = i.split("—").pop()?.trim() ?? i;
              return (
                <option key={i} value={addr}>
                  {i}
                </option>
              );
            })}
          </select>
        )}
        {chosen ? (
          <img
            className="qr"
            src={`${client.httpBase}/qr.svg?data=${encodeURIComponent(url)}`}
            alt={`QR code for ${url}`}
          />
        ) : (
          <p className="warn">No network interface found.</p>
        )}
        <code className="join-url">{url}</code>
        <button onClick={onClose}>Close</button>
      </div>
    </div>
  );
}

export default function App() {
  const { connected, status, errors, dismissError, client, denied, savedPulse } = useGate();
  const [tab, setTab] = useState<TabId>(tabFromHash);
  const [showConnect, setShowConnect] = useState(false);
  const [savedVisible, setSavedVisible] = useState(false);

  // Flash "saved" whenever the backend confirms a config change (from any client).
  useEffect(() => {
    if (savedPulse === 0) return;
    setSavedVisible(true);
    const t = setTimeout(() => setSavedVisible(false), 1200);
    return () => clearTimeout(t);
  }, [savedPulse]);

  // Hash <-> tab sync, so PWA shortcuts / popped-out windows can pin a mode.
  useEffect(() => {
    const onHash = () => setTab(tabFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const selectTab = (t: TabId) => {
    location.hash = t;
    setTab(t);
  };

  // Global keyboard: 1-4 fire effects.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      const fx = EFFECTS.find((f) => f.key === e.key);
      if (fx) {
        const color = loadSelectedLiveColor();
        client.triggerEffect({
          kind: fx.kind,
          angle: Math.random() * Math.PI * 2,
          hue: color.hue,
          saturation: color.saturation,
          brightness: color.brightness,
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [client]);

  if (denied) {
    return (
      <div className="app denied-screen">
        <h1>Not connected</h1>
        <p>{denied}</p>
        <p className="hint">
          Ask the operator, then reload this page (or re-scan the Connect QR code).
        </p>
      </div>
    );
  }

  return (
    <div className="app">
      <header className="topbar">
        <h1>Empyrean Gate</h1>
        <nav>
          {NAV_TABS.map((t) => (
            <button
              key={t.id}
              className={tab === t.id ? "active" : ""}
              onClick={() => selectTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </nav>
        <span className="spacer" />
        <span className={`saved-chip ${savedVisible ? "show" : ""}`}>✓ saved</span>
        <button className="ghost" onClick={() => setShowConnect(true)}>
          ⊕ Connect
        </button>
        {IN_TAURI && (
          <button className="ghost" onClick={() => void openNewWindow(tab)}>
            ⧉ New window
          </button>
        )}
        {status && <span className="gpu-name">{status.gpu_name}</span>}
        <span className={connected ? "conn ok" : "conn bad"}>
          {connected ? "connected" : "reconnecting…"}
        </span>
        <VersionChip />
      </header>

      {status?.gpu_error && (
        <div className="banner error">
          <strong>GPU error:</strong> {status.gpu_error}
        </div>
      )}
      {errors.map((e, i) => (
        <div key={i} className="banner warn" onClick={() => dismissError(i)}>
          {e} <span className="hint">(click to dismiss)</span>
        </div>
      ))}

      <main>
        {tab === "live" && <Live />}
        {/* Keep the decoder mounted while the operator visits Live/Settings.
            An offscreen composited video continues producing frames on iPadOS;
            unmounting it would stop the Gate feed at every tab change. */}
        <div
          className={tab === "media" ? "media-tab-active" : "media-tab-background"}
          aria-hidden={tab !== "media"}
          inert={tab !== "media"}
        >
          <Media />
        </div>
        {tab === "replay" && <Replay />}
        {tab === "control" && <Control />}
        {tab === "settings" && <Settings />}
      </main>

      {showConnect && <ConnectModal onClose={() => setShowConnect(false)} />}
      <DisconnectedOverlay disabled={tab === "replay"} />
    </div>
  );
}
