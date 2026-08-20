import { defaultLayer, type LayerCfg, type LayerKind } from "./types";

export interface ScenePreset {
  id: string;
  name: string;
  source: string;
  description: string;
  palette: string[];
  masterSpeed: number;
  walkSpeed: number;
  walkDepth: number;
  layers: LayerCfg[];
}

function layer(
  kind: LayerKind,
  name: string,
  values: Partial<Omit<LayerCfg, "kind" | "name">>,
): LayerCfg {
  return { ...defaultLayer(kind), ...values, kind, name };
}

/**
 * Procedural scene studies translated from the authored Uprising-Data library.
 * They borrow the saved pieces' palette, motion, and compositing ideas without
 * requiring their source videos. The slow walk only varies parameters within
 * each composition; it never changes which layers are playing.
 */
export const SCENE_PRESETS: ScenePreset[] = [
  {
    id: "original-gate",
    name: "Original Gate",
    source: "Empyrean Gate · original default",
    description: "The pre-Uprising house stack: nebula, harmonic rings, treble glitter, and beat rings.",
    palette: ["#413f92", "#32d5d2", "#ffb5d7"],
    masterSpeed: 1,
    walkSpeed: 1,
    walkDepth: 1,
    layers: [
      layer("noise_color", "Nebula base", {
        blend: "alpha_over", opacity: 1, speed: 0.25, scale: 1.2,
        audio_amount: 0.3, hue: 0.65, hue_range: 0.25, saturation: 0.9,
        brightness: 0.5, walk_amount: 0.25,
      }),
      layer("radial_waves", "Harmonic rings", {
        blend: "add", opacity: 0.6, speed: 1, scale: 1,
        audio_amount: 0.8, hue: 0.55, hue_range: 0.1, saturation: 0.9,
        brightness: 1, walk_amount: 0.25, param_a: 3, param_b: 4,
      }),
      layer("sparkle", "Treble glitter", {
        blend: "add", opacity: 0.7, speed: 1, scale: 1,
        audio_amount: 0.9, hue: 0.12, hue_range: 0.05, saturation: 0.3,
        brightness: 1, walk_amount: 0.25, param_a: 0.15,
      }),
      layer("beat_rings", "Beat rings", {
        blend: "add", opacity: 0.8, speed: 1, scale: 1,
        audio_amount: 1, hue: 0.85, hue_range: 0, saturation: 0.9,
        brightness: 1, walk_amount: 0.25, param_a: 0.08,
      }),
    ],
  },
  {
    id: "warm-windstorm",
    name: "Warm Windstorm",
    source: "Uprising · Warm Windstorm",
    description: "A low amber weather system with rolling flame, smoke, and sparse embers.",
    palette: ["#ff4d20", "#ff9d35", "#ffd38a"],
    masterSpeed: 0.62,
    walkSpeed: 0.38,
    walkDepth: 0.52,
    layers: [
      layer("gradient_radial", "Windstorm glow", {
        blend: "add", opacity: 0.28, speed: 0.08, scale: 0.7,
        audio_amount: 0.05, hue: 0.015, hue_range: 0.11, saturation: 0.92,
        brightness: 0.52, walk_amount: 0.12,
      }),
      layer("fire", "Rolling amber", {
        blend: "screen", opacity: 0.78, speed: 0.28, scale: 0.72,
        audio_amount: 0.18, hue: 0.0, hue_range: 0.08, saturation: 0.94,
        brightness: 0.88, walk_amount: 0.16, param_a: 0.68, param_b: 0.45,
      }),
      layer("noise_field", "Warm smoke", {
        blend: "screen", opacity: 0.42, speed: 0.16, scale: 1.35,
        audio_amount: 0.08, hue: 0.055, hue_range: 0.09, saturation: 0.86,
        brightness: 0.64, walk_amount: 0.2, param_a: 0.48,
      }),
      layer("meteors", "Loose embers", {
        blend: "add", opacity: 0.28, speed: 0.14, scale: 1,
        audio_amount: 0.1, hue: 0.02, hue_range: 0.1, saturation: 0.88,
        brightness: 0.92, walk_amount: 0.12, param_a: 0.14, param_b: 0.14,
        param_c: 0.25,
      }),
    ],
  },
  {
    id: "calm-cool-story",
    name: "Calm Cool Story",
    source: "Uprising · Calm Cool Story",
    description: "Purple glowscape, cool harmonic rings, and a dusting of cyan light.",
    palette: ["#675cff", "#8d71ff", "#57e4e0"],
    masterSpeed: 0.52,
    walkSpeed: 0.3,
    walkDepth: 0.44,
    layers: [
      layer("gradient_radial", "Twilight field", {
        blend: "add", opacity: 0.3, speed: 0.05, scale: 0.75,
        audio_amount: 0.04, hue: 0.64, hue_range: 0.18, saturation: 0.88,
        brightness: 0.48, walk_amount: 0.1,
      }),
      layer("plasma", "Purple glowscape", {
        blend: "screen", opacity: 0.54, speed: 0.1, scale: 0.68,
        audio_amount: 0.08, hue: 0.69, hue_range: 0.14, saturation: 0.86,
        brightness: 0.68, walk_amount: 0.16,
      }),
      layer("radial_waves", "Cool story rings", {
        blend: "add", opacity: 0.34, speed: 0.12, scale: 0.65,
        audio_amount: 0.12, hue: 0.51, hue_range: 0.18, saturation: 0.78,
        brightness: 0.7, walk_amount: 0.14, param_a: 0.16, param_b: 0.58,
      }),
      layer("sparkle", "Pastel dust", {
        blend: "add", opacity: 0.18, speed: 0.1, scale: 1,
        audio_amount: 0.08, hue: 0.55, hue_range: 0.3, saturation: 0.58,
        brightness: 0.86, walk_amount: 0.1, param_a: 0.12, param_b: 0.12,
      }),
    ],
  },
  {
    id: "color-journey",
    name: "Color Journey",
    source: "Uprising · Color Journey",
    description: "A slow migration from warm paint and flower gold into ocean blue.",
    palette: ["#ffb13b", "#ff5da9", "#31cbd3"],
    masterSpeed: 0.58,
    walkSpeed: 0.34,
    walkDepth: 0.58,
    layers: [
      layer("rainbow", "Journey wash", {
        blend: "max", opacity: 0.3, speed: 0.06, scale: 0.72,
        audio_amount: 0.04, hue: 0.04, hue_range: 0.34, saturation: 0.82,
        brightness: 0.48, walk_amount: 0.12, param_a: 0.28,
      }),
      layer("plasma", "Paint puddle", {
        blend: "screen", opacity: 0.5, speed: 0.11, scale: 0.78,
        audio_amount: 0.08, hue: 0.9, hue_range: 0.34, saturation: 0.86,
        brightness: 0.68, walk_amount: 0.18,
      }),
      layer("spiral", "Gold sky spiral", {
        blend: "add", opacity: 0.48, speed: 0.08, scale: 0.62,
        audio_amount: 0.08, hue: 0.085, hue_range: 0.13, saturation: 0.9,
        brightness: 0.82, walk_amount: 0.14, param_a: 0.28, param_b: 0.68,
        param_c: 0.42,
      }),
      layer("radial_waves", "Ocean turn", {
        blend: "screen", opacity: 0.25, speed: 0.09, scale: 0.6,
        audio_amount: 0.1, hue: 0.5, hue_range: 0.24, saturation: 0.78,
        brightness: 0.68, walk_amount: 0.12, param_a: 0.12, param_b: 0.48,
      }),
      layer("sparkle", "Flower glints", {
        blend: "add", opacity: 0.16, speed: 0.08, audio_amount: 0.06,
        hue: 0.08, hue_range: 0.5, saturation: 0.64, brightness: 0.84,
        walk_amount: 0.08, param_a: 0.1, param_b: 0.1,
      }),
    ],
  },
  {
    id: "cosmic",
    name: "Cosmic",
    source: "Uprising · 🛸 ~Cosmic / 🦋 ~Cosmic",
    description: "Deep-space interference, mirrored violet arms, and a quiet star current.",
    palette: ["#25215f", "#706dff", "#ecb5ff"],
    masterSpeed: 0.48,
    walkSpeed: 0.28,
    walkDepth: 0.48,
    layers: [
      layer("noise_color", "Deep color field", {
        blend: "alpha_over", opacity: 0.3, speed: 0.04, scale: 0.62,
        audio_amount: 0.03, hue: 0.67, hue_range: 0.14, saturation: 0.88,
        brightness: 0.5, walk_amount: 0.1,
      }),
      layer("interference", "Interstellar veil", {
        blend: "screen", opacity: 0.58, speed: 0.07, scale: 0.52,
        audio_amount: 0.06, hue: 0.63, hue_range: 0.2, saturation: 0.82,
        brightness: 0.72, walk_amount: 0.16, param_a: 0.2, param_b: 0.62,
        param_c: 0.3,
      }),
      layer("spiral", "Butterfly arms", {
        blend: "add", opacity: 0.34, speed: -0.05, scale: 0.58,
        audio_amount: 0.05, hue: 0.77, hue_range: 0.17, saturation: 0.74,
        brightness: 0.74, walk_amount: 0.12, param_a: 0.42, param_b: 0.32,
        param_c: 0.66,
      }),
      layer("warp", "Quiet star current", {
        blend: "add", opacity: 0.32, speed: 0.05, scale: 0.8,
        audio_amount: 0.06, hue: 0.57, hue_range: 0.25, saturation: 0.55,
        brightness: 0.9, walk_amount: 0.1, param_a: 0.1, param_b: 0.16,
      }),
      layer("sparkle", "Cosmic points", {
        blend: "add", opacity: 0.14, speed: 0.06, audio_amount: 0.04,
        hue: 0.66, hue_range: 0.38, saturation: 0.34, brightness: 0.9,
        walk_amount: 0.08, param_a: 0.08, param_b: 0.08,
      }),
    ],
  },
];
