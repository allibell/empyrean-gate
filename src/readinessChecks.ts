import type { AppConfig, RuntimeStatus } from "./types";

export type ReadinessLevel = "pass" | "warn" | "fail" | "info";

export interface ReadinessCheck {
  id: string;
  label: string;
  level: ReadinessLevel;
  summary: string;
  detail?: string;
  action?: "settings" | "control" | "firewall" | "updates";
}

export interface ReadinessReport {
  checks: ReadinessCheck[];
  failures: number;
  warnings: number;
  state: "ready" | "review" | "blocked" | "standby";
}

function interfaceAddresses(interfaces: string[]): string[] {
  return interfaces.map((entry) => entry.split("—").pop()?.trim() ?? entry.trim());
}

function updateCheck(config: AppConfig, status: RuntimeStatus): ReadinessCheck {
  const state = status.update_state.trim();
  const stateLower = state.toLowerCase();
  if (stateLower.includes("error") || stateLower.includes("failed")) {
    return {
      id: "updates",
      label: "Software update",
      level: "warn",
      summary: "The last update check failed",
      detail: state,
      action: "updates",
    };
  }
  if (status.update_available) {
    return {
      id: "updates",
      label: "Software update",
      level: "warn",
      summary: `v${status.update_available} is available`,
      detail: "Review release notes and update before soundcheck, not during a live show.",
      action: "updates",
    };
  }
  if (stateLower.includes("downloading") || stateLower.includes("install")) {
    return {
      id: "updates",
      label: "Software update",
      level: "warn",
      summary: state,
      detail: "Wait for the update handoff to finish before starting the show.",
    };
  }
  if (stateLower.includes("up to date")) {
    return { id: "updates", label: "Software update", level: "pass", summary: `v${status.version} is up to date` };
  }
  return {
    id: "updates",
    label: "Software update",
    level: "info",
    summary: `Running v${status.version || "unknown"}; update status not recently verified`,
    detail: config.update.auto_check ? "Automatic checks are enabled." : "Automatic update checks are off.",
    action: "updates",
  };
}

export function buildReadinessReport(
  config: AppConfig,
  status: RuntimeStatus,
  connected: boolean,
): ReadinessReport {
  const checks: ReadinessCheck[] = [];

  checks.push(connected
    ? { id: "backend", label: "Gate backend", level: "pass", summary: "Live status is connected" }
    : { id: "backend", label: "Gate backend", level: "fail", summary: "Status connection is down", detail: "Restore the backend/network connection before trusting any other result." });

  if (status.gpu_error) {
    checks.push({ id: "gpu", label: "GPU renderer", level: "fail", summary: "Renderer failed to initialize", detail: status.gpu_error, action: "settings" });
  } else if (!status.gpu_name || status.engine_fps <= 0) {
    checks.push({ id: "gpu", label: "GPU renderer", level: "fail", summary: "No rendered frames have been observed", detail: status.gpu_name || "No GPU reported." });
  } else {
    const target = Math.max(1, config.render.fps);
    const slow = status.engine_fps < target * 0.8;
    checks.push({
      id: "gpu",
      label: "GPU renderer",
      level: slow ? "warn" : "pass",
      summary: `${status.gpu_name} · ${status.engine_fps.toFixed(0)} fps · ${status.frame_time_ms.toFixed(1)} ms`,
      detail: slow ? `Below 80% of the ${target.toFixed(0)} fps render target.` : undefined,
      action: slow ? "settings" : undefined,
    });
  }

  const addresses = interfaceAddresses(status.interfaces);
  const explicitInterface = config.output.interface.trim();
  const interfaceMissing = explicitInterface !== "" && !addresses.includes(explicitInterface);
  const destinationMissing = !config.output.multicast && config.output.controllers.filter(Boolean).length === 0;
  if (!config.output.enabled) {
    checks.push({
      id: "sacn",
      label: "sACN output",
      level: "info",
      summary: "Standby — output is intentionally off",
      detail: "Configuration can be reviewed now, but packet delivery is not proven until output is enabled during soundcheck.",
      action: "settings",
    });
  } else if (!status.sacn_enabled) {
    checks.push({ id: "sacn", label: "sACN output", level: "fail", summary: "Configured on, but the engine reports output off", action: "settings" });
  } else if (destinationMissing) {
    checks.push({ id: "sacn", label: "sACN output", level: "fail", summary: "Unicast is selected with no controller addresses", action: "settings" });
  } else if (interfaceMissing) {
    checks.push({ id: "sacn", label: "sACN output", level: "fail", summary: `Configured interface ${explicitInterface} is not present`, detail: "Select the lighting-network interface again.", action: "settings" });
  } else if (status.sacn_pps <= 0) {
    checks.push({ id: "sacn", label: "sACN output", level: "fail", summary: "Enabled, but no packets are leaving Gate", detail: "Check the interface, destination mode, and controller addresses.", action: "settings" });
  } else {
    checks.push({
      id: "sacn",
      label: "sACN output",
      level: explicitInterface ? "pass" : "warn",
      summary: `${status.sacn_pps} pkt/s across ${status.sacn_universes} universes`,
      detail: explicitInterface ? `Bound to ${explicitInterface}.` : "Using the OS default route; explicitly select the lighting NIC on a multi-homed show machine.",
      action: explicitInterface ? undefined : "settings",
    });
  }

  const configuredAudio = config.audio.sources.length;
  const activeAudio = status.audio.filter((source) => source.active).length;
  const audioReactive = config.layers.some((layer) => layer.enabled && (layer.audio_amount > 0 || layer.kind === "waveform" || layer.kind === "spectrum")) || config.beat_taps.enabled;
  if (configuredAudio === 0) {
    checks.push({
      id: "audio",
      label: "Audio input",
      level: audioReactive ? "warn" : "info",
      summary: audioReactive ? "Audio-reactive content has no configured source" : "No audio source configured (visual-only is okay)",
      action: "settings",
    });
  } else if (activeAudio === 0) {
    checks.push({ id: "audio", label: "Audio input", level: audioReactive ? "fail" : "warn", summary: `No configured audio source is active (0/${configuredAudio})`, detail: status.audio.map((source) => source.detail).filter(Boolean).join(" · ") || undefined, action: "settings" });
  } else {
    checks.push({
      id: "audio",
      label: "Audio input",
      level: activeAudio === configuredAudio ? "pass" : "warn",
      summary: `${activeAudio}/${configuredAudio} source${configuredAudio === 1 ? "" : "s"} active`,
      detail: activeAudio < configuredAudio ? "At least one configured source is waiting or unavailable." : undefined,
      action: activeAudio < configuredAudio ? "settings" : undefined,
    });
  }

  const rhythm = status.rhythm;
  const manual = config.render.manual_bpm !== null;
  if (manual) {
    checks.push({ id: "rhythm", label: "Lighting clock", level: "pass", summary: `Manual clock · ${config.render.manual_bpm!.toFixed(1)} BPM`, detail: "External clock health does not affect the current manual override." });
  } else if (!rhythm.active || !rhythm.running) {
    checks.push({ id: "rhythm", label: "Lighting clock", level: config.rhythm.source === "layer_audio" ? "warn" : "fail", summary: rhythm.detail || "No running beat clock", detail: `Selected source: ${config.rhythm.source.replaceAll("_", " ")}.`, action: "settings" });
  } else {
    checks.push({
      id: "rhythm",
      label: "Lighting clock",
      level: rhythm.using_fallback ? "warn" : "pass",
      summary: `${rhythm.using_fallback ? "Audio fallback" : rhythm.source} · ${rhythm.bpm > 0 ? `${rhythm.bpm.toFixed(1)} BPM` : "tempo pending"}`,
      detail: rhythm.using_fallback ? "The selected external clock is unavailable; Gate is keeping time from audio." : rhythm.detail || undefined,
      action: rhythm.using_fallback ? "settings" : undefined,
    });
  }

  checks.push(status.firewall_pending
    ? { id: "firewall", label: "LAN access", level: "warn", summary: "Windows Firewall authorization is missing", detail: "Phones and tablets may not be able to connect.", action: "firewall" }
    : { id: "firewall", label: "LAN access", level: "pass", summary: "No missing firewall rule is reported" });

  checks.push(updateCheck(config, status));

  const selectedPlaylist = config.saved_playlists.find((playlist) => playlist.id === config.show_scheduler.active_playlist_id);
  if (config.show_scheduler.enabled && (!selectedPlaylist || selectedPlaylist.entries.length === 0)) {
    checks.push({ id: "show", label: "Scheduled show", level: "fail", summary: "Scheduler is enabled without a playable playlist", action: "control" });
  } else if (config.show_scheduler.enabled && !status.show.enabled) {
    checks.push({ id: "show", label: "Scheduled show", level: "fail", summary: "Scheduler is enabled, but no cue is running", detail: "Open Control and confirm the active playlist/cue.", action: "control" });
  } else if (status.show.enabled) {
    checks.push({ id: "show", label: "Scheduled show", level: "pass", summary: `${status.show.playlist_name} · cue ${status.show.index + 1}/${status.show.total}`, detail: `${status.show.scene_name} · ${Math.max(0, status.show.remaining_secs).toFixed(0)}s remaining`, action: "control" });
  } else {
    checks.push({ id: "show", label: "Scheduled show", level: "info", summary: `Manual operation; ${config.saved_playlists.length} saved playlist${config.saved_playlists.length === 1 ? "" : "s"}`, detail: "This is valid when an operator will run the show manually.", action: "control" });
  }

  const connectedClients = status.client_list.filter((client) => client.connected && !client.revoked);
  checks.push({
    id: "clients",
    label: "Controllers",
    level: connectedClients.length > 0 ? "pass" : "info",
    summary: connectedClients.length > 0 ? `${connectedClients.length} remote controller${connectedClients.length === 1 ? "" : "s"} connected` : "No remote controllers connected",
    detail: connectedClients.length > 0 ? connectedClients.map((client) => client.name).join(", ") : "Fine for local or unattended operation; connect a backup device if the show plan calls for one.",
  });

  if (status.master_brightness <= 0.001) {
    checks.push({ id: "brightness", label: "Master output", level: "warn", summary: "Master brightness is blacked out", detail: "This may be intentional during setup. Raise it before expecting visible output.", action: "control" });
  } else {
    checks.push({ id: "brightness", label: "Master output", level: "pass", summary: `${Math.round(status.master_brightness * 100)}% brightness` });
  }

  const failures = checks.filter((check) => check.level === "fail").length;
  const warnings = checks.filter((check) => check.level === "warn").length;
  const state = failures > 0 ? "blocked" : warnings > 0 ? "review" : config.output.enabled ? "ready" : "standby";
  return { checks, failures, warnings, state };
}
