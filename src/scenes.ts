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
  {
    id: "bioluminescent-tide",
    name: "Bioluminescent Tide",
    source: "Empyrean Gate · nocturne study",
    description: "Slow cyan surf folds through midnight blue while tiny organisms answer the beat.",
    palette: ["#031d35", "#00b9c5", "#a6fff2"],
    masterSpeed: 0.42,
    walkSpeed: 0.26,
    walkDepth: 0.55,
    layers: [
      layer("noise_color", "Midnight water", {
        blend: "alpha_over", opacity: 0.48, speed: 0.04, scale: 0.72,
        audio_amount: 0.04, hue: 0.55, hue_range: 0.1, saturation: 0.94,
        brightness: 0.42, walk_amount: 0.12,
      }),
      layer("radial_waves", "Luminous undertow", {
        blend: "screen", opacity: 0.48, speed: -0.09, scale: 0.74,
        audio_amount: 0.18, hue: 0.48, hue_range: 0.1, saturation: 0.82,
        brightness: 0.74, walk_amount: 0.17, param_a: 0.22, param_b: 0.62,
      }),
      layer("warp", "Tidal filaments", {
        blend: "add", opacity: 0.3, speed: 0.07, scale: 0.58,
        audio_amount: 0.12, hue: 0.52, hue_range: 0.12, saturation: 0.74,
        brightness: 0.8, walk_amount: 0.14, param_a: 0.13, param_b: 0.2,
      }),
      layer("sparkle", "Plankton pulse", {
        blend: "add", opacity: 0.2, speed: 0.12, audio_amount: 0.4,
        hue: 0.45, hue_range: 0.18, saturation: 0.5, brightness: 0.95,
        walk_amount: 0.1, param_a: 0.1, param_b: 0.18,
      }),
    ],
  },
  {
    id: "desert-bloom",
    name: "Desert Bloom",
    source: "Empyrean Gate · playa study",
    description: "Rose-gold petals open across warm dust, then dissolve into lavender heat shimmer.",
    palette: ["#ff713d", "#ffc66e", "#b47cff"],
    masterSpeed: 0.5,
    walkSpeed: 0.31,
    walkDepth: 0.62,
    layers: [
      layer("gradient_radial", "Sun-warmed dust", {
        blend: "add", opacity: 0.34, speed: 0.04, scale: 0.82,
        audio_amount: 0.05, hue: 0.06, hue_range: 0.13, saturation: 0.86,
        brightness: 0.54, walk_amount: 0.12,
      }),
      layer("wedges", "Opening petals", {
        blend: "screen", opacity: 0.46, speed: 0.08, scale: 0.68,
        audio_amount: 0.14, hue: 0.96, hue_range: 0.18, saturation: 0.82,
        brightness: 0.76, walk_amount: 0.18, param_a: 0.3, param_b: 0.56,
      }),
      layer("plasma", "Lavender mirage", {
        blend: "screen", opacity: 0.34, speed: -0.06, scale: 0.9,
        audio_amount: 0.08, hue: 0.76, hue_range: 0.2, saturation: 0.7,
        brightness: 0.68, walk_amount: 0.16,
      }),
      layer("sparkle", "Gold pollen", {
        blend: "add", opacity: 0.16, speed: 0.1, audio_amount: 0.18,
        hue: 0.1, hue_range: 0.12, saturation: 0.55, brightness: 0.94,
        walk_amount: 0.08, param_a: 0.08, param_b: 0.14,
      }),
    ],
  },
  {
    id: "cathedral-pulse",
    name: "Cathedral Pulse",
    source: "Empyrean Gate · architectural study",
    description: "Deep indigo arches breathe outward as restrained gold geometry marks the rhythm.",
    palette: ["#171142", "#4f3fe1", "#ffd16a"],
    masterSpeed: 0.46,
    walkSpeed: 0.24,
    walkDepth: 0.46,
    layers: [
      layer("breathe", "Indigo nave", {
        blend: "alpha_over", opacity: 0.58, speed: 0.08, scale: 0.74,
        audio_amount: 0.12, hue: 0.66, hue_range: 0.1, saturation: 0.92,
        brightness: 0.5, walk_amount: 0.1, param_a: 0.24,
      }),
      layer("interference", "Vaulted harmonics", {
        blend: "screen", opacity: 0.42, speed: 0.05, scale: 0.55,
        audio_amount: 0.14, hue: 0.7, hue_range: 0.16, saturation: 0.76,
        brightness: 0.68, walk_amount: 0.14, param_a: 0.16, param_b: 0.58,
      }),
      layer("beat_rings", "Gilded bell", {
        blend: "add", opacity: 0.34, speed: 0.12, scale: 0.72,
        audio_amount: 0.7, hue: 0.11, hue_range: 0.05, saturation: 0.72,
        brightness: 0.92, walk_amount: 0.08, param_a: 0.07,
      }),
      layer("spoke_chase", "Processional light", {
        blend: "add", opacity: 0.16, speed: 0.07, scale: 0.8,
        audio_amount: 0.12, hue: 0.62, hue_range: 0.32, saturation: 0.58,
        brightness: 0.84, walk_amount: 0.1, param_a: 0.1, param_b: 0.14,
      }),
    ],
  },
  {
    id: "ember-constellation",
    name: "Ember Constellation",
    source: "Empyrean Gate · fire-sky study",
    description: "A near-black sky gathers wandering coals into slow spirals and meteor constellations.",
    palette: ["#160a13", "#d93d20", "#ffbf55"],
    masterSpeed: 0.4,
    walkSpeed: 0.22,
    walkDepth: 0.58,
    layers: [
      layer("noise_field", "Coal-dark sky", {
        blend: "alpha_over", opacity: 0.5, speed: 0.03, scale: 0.66,
        audio_amount: 0.03, hue: 0.98, hue_range: 0.07, saturation: 0.88,
        brightness: 0.32, walk_amount: 0.1,
      }),
      layer("fire", "Banked embers", {
        blend: "screen", opacity: 0.4, speed: 0.08, scale: 0.62,
        audio_amount: 0.12, hue: 0.015, hue_range: 0.1, saturation: 0.93,
        brightness: 0.7, walk_amount: 0.16, param_a: 0.42, param_b: 0.34,
      }),
      layer("spiral", "Constellation arms", {
        blend: "add", opacity: 0.28, speed: -0.04, scale: 0.68,
        audio_amount: 0.09, hue: 0.06, hue_range: 0.12, saturation: 0.84,
        brightness: 0.82, walk_amount: 0.14, param_a: 0.18, param_b: 0.38,
        param_c: 0.58,
      }),
      layer("meteors", "Drifting coals", {
        blend: "add", opacity: 0.32, speed: 0.09, scale: 1,
        audio_amount: 0.16, hue: 0.035, hue_range: 0.12, saturation: 0.86,
        brightness: 0.94, walk_amount: 0.12, param_a: 0.12, param_b: 0.1,
        param_c: 0.18,
      }),
    ],
  },
];
