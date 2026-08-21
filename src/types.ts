// Mirrors src-tauri/src/protocol.rs, config.rs, layers.rs (serde snake_case JSON).

export type LayerKind =
  | "solid"
  | "gradient_radial"
  | "noise_field"
  | "noise_color"
  | "radial_waves"
  | "spiral"
  | "plasma"
  | "spoke_chase"
  | "sparkle"
  | "beat_rings"
  | "breathe"
  | "rainbow"
  | "wedges"
  | "interference"
  | "fire"
  | "meteors"
  | "warp"
  | "waveform"
  | "spectrum"
  | "video";

export type BlendMode = "add" | "multiply" | "screen" | "alpha_over" | "max";

export type EffectKind = "burst" | "strobe" | "swoosh" | "collapse";

export type PenKind = "glow" | "ripple" | "sparkle" | "comet" | "ring" | "beam" | "ember";

export interface LayerCfg {
  kind: LayerKind;
  enabled: boolean;
  name: string;
  blend: BlendMode;
  opacity: number;
  speed: number;
  scale: number;
  audio_source: number;
  audio_amount: number;
  hue: number;
  hue_range: number;
  saturation: number;
  brightness: number;
  tilt_amount: number;
  walk_amount: number;
  param_a: number;
  param_b: number;
  param_c: number;
  param_d: number;
}

export interface SavedStack {
  id: string;
  name: string;
  layers: LayerCfg[];
  master_speed: number;
  walk_enabled: boolean;
  walk_layers: boolean;
  walk_min_layers: number;
  walk_speed: number;
  walk_depth: number;
}

export interface EffectCfg {
  kind: EffectKind;
  angle: number;
  radius: number;
  intensity: number;
  size: number;
  hue: number;
  saturation: number;
  brightness: number;
  duration: number;
}

export interface GeometryConfig {
  spokes: number;
  pixels_per_spoke: number;
  outer_radius_ft: number;
  inner_radius_ft: number;
  leds_per_meter: number;
}

export interface OutputConfig {
  enabled: boolean;
  interface: string;
  sync_to_render: boolean;
  fps: number;
  sync_universe: number;
  start_universe: number;
  pixels_per_universe: number;
  controllers: string[];
  strings_per_controller: number;
  multicast: boolean;
  priority: number;
  led_gamma: number;
  /** Persistent E1.31 source identity (UUID). Generated on first run; never changes. */
  cid: string;
  source_name: string;
  discovery: boolean;
}

export interface ServerConfig {
  bind: string;
  port: number;
  max_preview_clients: number;
  auth_token: string | null;
  join_token: string;
  require_token: boolean;
}

export interface ClientRecord {
  id: string;
  name: string;
  revoked: boolean;
}

export interface ClientInfo {
  id: string;
  name: string;
  connected: boolean;
  revoked: boolean;
}

export type AudioSourceConfig = {
  id: string;
  gain: number;
} & (
  | { kind: "device"; device: string | null; channels: number[]; loopback: boolean }
  | { kind: "remote"; client_id: string }
  | { kind: "video" }
);

export interface AudioConfig {
  sources: AudioSourceConfig[];
}

export interface RenderConfig {
  fps: number;
  master_brightness: number;
  master_speed: number;
  manual_bpm: number | null;
  beat_time: "half" | "normal" | "double";
  walk_enabled: boolean;
  walk_layers: boolean;
  walk_min_layers: number;
  walk_speed: number;
  walk_depth: number;
}

export interface UpdateConfig {
  auto_check: boolean;
  auto_install: boolean;
}

export interface WindowsConfig {
  aux_open: string[];
}

export interface BeatTapConfig {
  enabled: boolean;
  audio_source: number;
  spin: number;
  vary: boolean;
  radius: number;
  intensity: number;
  hue: number;
  every: number;
}

export interface AppConfig {
  geometry: GeometryConfig;
  output: OutputConfig;
  server: ServerConfig;
  audio: AudioConfig;
  render: RenderConfig;
  update: UpdateConfig;
  windows: WindowsConfig;
  beat_taps: BeatTapConfig;
  layers: LayerCfg[];
  saved_stacks: SavedStack[];
  clients: ClientRecord[];
}

export interface DeviceInfo {
  name: string;
  /** Channel count of the device's default config (0 = unknown). */
  channels: number;
}

export interface AudioSourceStatus {
  id: string;
  active: boolean;
  /** Health note, e.g. "waiting for device". Empty when running. */
  detail: string;
  level: number;
  bass: number;
  mid: number;
  treble: number;
  bpm: number;
  beat_phase: number;
}

export interface RuntimeStatus {
  gpu_error: string | null;
  gpu_name: string;
  engine_fps: number;
  frame_time_ms: number;
  sacn_enabled: boolean;
  sacn_universes: number;
  sacn_pps: number;
  fps_history: number[];
  pps_history: number[];
  clients: number;
  audio: AudioSourceStatus[];
  input_devices: DeviceInfo[];
  output_devices: DeviceInfo[];
  default_input_channels: number;
  default_output_channels: number;
  interfaces: string[];
  client_list: ClientInfo[];
  master_brightness: number;
  master_speed: number;
  version: string;
  update_available: string | null;
  update_state: string;
  video: VideoSourceStatus;
}

export interface VideoSourceStatus {
  active: boolean;
  owner_id: string;
  owner_name: string;
  title: string;
  source_url: string;
  width: number;
  height: number;
  fps: number;
  frames: number;
}

export type ServerMsg =
  | { type: "state"; config: AppConfig; status: RuntimeStatus }
  | { type: "status"; status: RuntimeStatus }
  | { type: "beat"; source: number; bpm: number }
  | {
      type: "preview_meta";
      spokes: number;
      pixels: number;
      decimate: number;
      outer_radius_ft: number;
      inner_radius_ft: number;
    }
  | { type: "error"; message: string }
  | { type: "denied"; reason: string }
  | { type: "preview_queue"; position: number };

export interface PreviewMeta {
  spokes: number;
  pixels: number;
  decimate: number;
  outer_radius_ft: number;
  inner_radius_ft: number;
}

export interface PreviewFrame {
  frameNumber: number;
  spokes: number;
  pixels: number;
  rgb: Uint8Array;
}

export const LAYER_KINDS: LayerKind[] = [
  "solid",
  "gradient_radial",
  "noise_field",
  "noise_color",
  "radial_waves",
  "spiral",
  "plasma",
  "spoke_chase",
  "sparkle",
  "beat_rings",
  "breathe",
  "rainbow",
  "wedges",
  "interference",
  "fire",
  "meteors",
  "warp",
  "waveform",
  "spectrum",
  "video",
];

export const BLEND_MODES: BlendMode[] = ["add", "multiply", "screen", "alpha_over", "max"];

export const LAYER_LABELS: Record<LayerKind, string> = {
  solid: "Solid",
  gradient_radial: "Radial Gradient",
  noise_field: "Noise Field",
  noise_color: "Color Noise",
  radial_waves: "Harmonic Rings",
  spiral: "Spiral",
  plasma: "Plasma",
  spoke_chase: "Spoke Chase",
  sparkle: "Sparkle",
  beat_rings: "Beat Rings",
  breathe: "Breathe",
  rainbow: "Rainbow",
  wedges: "Wedges",
  interference: "Interference",
  fire: "Fire",
  meteors: "Meteors",
  warp: "Warp",
  waveform: "Waveform",
  spectrum: "Spectrum",
  video: "Video",
};

/** Kind-specific labels for param_a..d, where meaningful. */
export const PARAM_LABELS: Partial<Record<LayerKind, [string?, string?, string?, string?]>> = {
  noise_field: ["Threshold"],
  radial_waves: ["Base freq", "Harmonics"],
  spiral: ["Arms", "Twist", "Sharpness"],
  spoke_chase: ["Speed", "Direction", "Tail length"],
  sparkle: ["Density", "Twinkle rate"],
  beat_rings: ["Ring width", "Direction"],
  breathe: ["Depth floor"],
  rainbow: ["Turns"],
  wedges: ["Slices", "Radial twist", "Edge softness"],
  interference: ["Frequency", "Orbit size", "Sharpness"],
  fire: ["Flame reach", "Flame stretch"],
  meteors: ["Density", "Rate/tail", "Direction"],
  warp: ["Star density", "Speed"],
  waveform: ["Ring radius", "Depth", "Thickness"],
  spectrum: ["Bar length", "From outer/inner"],
  video: ["Zoom", "Kaleidoscope", "Contrast", "Rotation"],
};

export function defaultLayer(kind: LayerKind): LayerCfg {
  const layer: LayerCfg = {
    kind,
    enabled: true,
    name: LAYER_LABELS[kind],
    blend: "add",
    opacity: 1.0,
    speed: 1.0,
    scale: 1.0,
    audio_source: 0,
    audio_amount: 0.5,
    hue: 0.6,
    hue_range: 0.2,
    saturation: 0.9,
    brightness: 1.0,
    tilt_amount: 0.0,
    walk_amount: 0.25,
    param_a: 0.5,
    param_b: 0.5,
    param_c: 0.5,
    param_d: 0.5,
  };
  if (kind === "video") {
    layer.blend = "alpha_over";
    layer.audio_amount = 0.7;
    layer.hue_range = 1;
    layer.saturation = 1;
    layer.walk_amount = 0;
    layer.param_a = 0.5;
    layer.param_b = 0;
    layer.param_c = 0.35;
  }
  return layer;
}
