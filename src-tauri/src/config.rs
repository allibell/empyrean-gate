//! Application configuration: geometry of the installation, sACN output, audio
//! sources, server, and the layer stack. Persisted as JSON in the user config dir.

use crate::layers::{BlendMode, LayerCfg, LayerKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeometryConfig {
    /// Number of radial spokes (strings).
    pub spokes: u32,
    /// Pixels per spoke. Pixel 0 is at the OUTER radius (strings are fed from outside).
    pub pixels_per_spoke: u32,
    /// Outer (major) radius in feet — 50 ft diameter installation.
    pub outer_radius_ft: f32,
    /// Inner (minor) radius in feet, where the last pixel of each spoke sits.
    pub inner_radius_ft: f32,
    /// Informational; used to sanity-check spoke length against pixel count in the UI.
    pub leds_per_meter: f32,
}

impl Default for GeometryConfig {
    fn default() -> Self {
        Self {
            spokes: 64,
            pixels_per_spoke: 350,
            outer_radius_ft: 25.0,
            inner_radius_ft: 8.0,
            leds_per_meter: 60.0,
        }
    }
}

impl GeometryConfig {
    pub fn pixel_count(&self) -> usize {
        (self.spokes * self.pixels_per_spoke) as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Master switch — defaults OFF so a fresh install never floods a network.
    pub enabled: bool,
    /// Local interface IP to send from (source for unicast, egress for multicast).
    /// Empty = OS default route — on a multi-homed machine that is often the wrong
    /// NIC for the lighting network, so pick one in Settings.
    pub interface: String,
    /// Send an sACN frame for every rendered frame (capped by `fps`).
    pub sync_to_render: bool,
    /// sACN frame-rate cap (also the fixed rate when `sync_to_render` is off).
    pub fps: f32,
    /// E1.31 universe synchronization: data packets carry this sync address and a
    /// sync packet per frame releases all universes at once (tear-free on receivers
    /// that support it, e.g. PixLite Mk4; others ignore it). 0 = disabled.
    pub sync_universe: u16,
    /// First universe number; each spoke starts on a fresh universe boundary.
    pub start_universe: u16,
    /// Pixels per universe (170 * 3 = 510 channels fits the 512-channel DMX frame).
    pub pixels_per_universe: u16,
    /// Unicast destinations, one per controller in spoke order. Used only when
    /// `multicast` is false; controller i drives `strings_per_controller` spokes.
    /// Empty entries leave the corresponding spokes without an output destination.
    pub controllers: Vec<String>,
    pub strings_per_controller: u32,
    /// Destination mode: true sends only to sACN multicast groups (239.255.u.u),
    /// false sends only to the configured controller addresses.
    pub multicast: bool,
    /// E1.31 priority (default 100).
    pub priority: u8,
    /// Gamma applied to LED output only (preview shows the raw pattern).
    pub led_gamma: f32,
    /// E1.31 source identity: a UUID that receivers key ALL per-source state on —
    /// merge arbitration, sequence tracking, and the 2.5 s source-loss timeout.
    /// Generated once on first run and then persistent: the spec requires it to
    /// survive restarts and upgrades, and a changed CID makes every receiver treat
    /// us as a brand-new source while the old identity lingers in its merge table
    /// (visible HTP-merge artifacts, and controllers with a 2–4 source cap can
    /// refuse the new one). A handover between instances is seamless precisely
    /// because the successor reads this same CID out of the config.
    pub cid: String,
    /// E1.31 source name, shown by receivers and diagnostic tools. 64 bytes on the
    /// wire (UTF-8, null-terminated); longer names are truncated.
    pub source_name: String,
    /// Advertise our universe list on the E1.31 discovery universe (64214,
    /// 239.255.250.214) every 10 s while transmitting. This is what makes the
    /// source — and which universes it drives — visible in sACNView and controller
    /// UIs. Costs one small multicast packet per 10 s.
    pub discovery: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interface: String::new(),
            sync_to_render: true,
            fps: 60.0,
            sync_universe: 0,
            start_universe: 1,
            pixels_per_universe: 170,
            controllers: Vec::new(),
            strings_per_controller: 4,
            multicast: true,
            priority: 100,
            led_gamma: 2.2,
            cid: String::new(), // filled on first load (see `load`)
            source_name: "Empyrean Gate".into(),
            discovery: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    /// Max clients streaming the live preview at once — the preview is >98% of
    /// per-client bandwidth, so this is the WiFi safety valve. Clients beyond the
    /// cap keep full control (taps/drawing/effects are tiny) and wait in a queue
    /// for a viewing slot.
    pub max_preview_clients: u32,
    /// Legacy placeholder, superseded by `join_token` + `require_token`.
    pub auth_token: Option<String>,
    /// Join token embedded in the connect QR URL. Generated on first run; the
    /// "rotate" action replaces it (locking out devices that only had the old one,
    /// when `require_token` is on).
    pub join_token: String,
    /// When true, unknown clients must present the join token (scan the QR) to
    /// connect. Loopback clients (the desktop app's own webview) always may.
    /// Off = open LAN access; revocation is then only a client-id blocklist.
    pub require_token: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0".into(),
            port: 9520,
            max_preview_clients: 10,
            auth_token: None,
            join_token: String::new(),
            require_token: false,
        }
    }
}

/// A client device that has connected at least once. Identified by the persistent
/// id the client keeps in localStorage; named for humans; revocable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientRecord {
    pub id: String,
    pub name: String,
    pub revoked: bool,
}

/// Random URL-safe token (join links). Seeded from `RandomState`, which is
/// randomly keyed per process — fine for LAN join control, not cryptography.
pub fn generate_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64 ^ (i as u64) << 32);
        out.push_str(&format!("{:08x}", h.finish() as u32));
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioSourceKind {
    /// A local capture device (cpal). `device: None` = system default input.
    /// `channels`: which channels of the device to mix into this source's mono
    /// analysis signal; empty = all channels. Lets one multichannel interface feed
    /// several sources (e.g. stage feed on 1+2, local mic on 3).
    /// `loopback: true` captures an OUTPUT device's playback (WASAPI loopback) —
    /// use whatever is playing on this machine as the beat source.
    Device {
        device: Option<String>,
        channels: Vec<u32>,
        #[serde(default)]
        loopback: bool,
    },
    /// Features streamed from a remote browser client (its microphone) over WebSocket.
    /// `client_id` is matched against the id the remote client announces.
    Remote { client_id: String },
    /// Features extracted in the browser from the soundtrack of the currently
    /// active video. Packets are accepted only from that video's owning client.
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSourceConfig {
    /// Stable id, referenced by layers (`audio_source` index is positional though —
    /// the id is for display and remote matching).
    pub id: String,
    #[serde(flatten)]
    pub kind: AudioSourceKind,
    pub gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Up to `layers::MAX_AUDIO_SOURCES` analyzed in parallel; layers pick one by index.
    pub sources: Vec<AudioSourceConfig>,
}

/// Where the musical clock used by beat-synchronized visuals comes from. Audio
/// energy remains per-layer regardless of this choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RhythmSource {
    /// Preserve the original behavior: each layer follows the beat detector for
    /// the same audio source it uses for level/bands/spectrum.
    #[default]
    LayerAudio,
    /// One MIDI Timing Clock drives every layer; audio remains independently
    /// selectable per layer. Intended for DJ mixers, bridges, and controllers.
    MidiClock,
    /// Passively receive beat/status packets from a Pioneer/AlphaTheta PRO DJ
    /// LINK network. This app never announces a virtual deck or sends commands.
    ProDjLink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RhythmConfig {
    pub source: RhythmSource,
    /// Exact operating-system MIDI input port name. None means no port selected;
    /// it never silently substitutes another device after a disconnect.
    pub midi_port: Option<String>,
    /// 0 follows the reported tempo master. 1..6 pins one player number, useful
    /// with hardware that broadcasts beat packets but not full status to listeners.
    pub pro_dj_link_player: u8,
    /// Shift the lighting clock relative to the external input to compensate for LED/audio
    /// transport latency. Positive values make the visual beat happen later.
    pub latency_ms: f32,
    /// If the external clock stops arriving, keep the show moving from this audio detector.
    pub fallback_to_audio: bool,
    pub fallback_audio_source: u32,
}

impl Default for RhythmConfig {
    fn default() -> Self {
        Self {
            source: RhythmSource::LayerAudio,
            midi_port: None,
            pro_dj_link_player: 0,
            latency_ms: 0.0,
            fallback_to_audio: true,
            fallback_audio_source: 0,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sources: vec![AudioSourceConfig {
                id: "main".into(),
                kind: AudioSourceKind::Device {
                    device: None,
                    channels: Vec::new(),
                    loopback: false,
                },
                gain: 1.0,
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderConfig {
    pub fps: f32,
    pub master_brightness: f32,
    pub master_speed: f32,
    /// When set, drive the lighting beat clock at this BPM instead of following
    /// the audio detector. Half/normal/double time is applied afterward.
    pub manual_bpm: Option<f32>,
    /// Musical clock presented to lighting effects. Tempo detection remains at the
    /// source rate; this only changes the beat phase/BPM consumed by the show.
    pub beat_time: BeatTime,
    /// Autopilot: slow mean-reverting random walk over layer parameters, so an
    /// unattended show keeps evolving for hours. Each layer's `walk_amount` scales
    /// how far its parameters may wander from where the sliders are set.
    pub walk_enabled: bool,
    /// Also walk WHICH layers play (Gray-code style: one layer fades in or out per
    /// step), on top of the parameter walk.
    pub walk_layers: bool,
    /// Never fewer than this many of the enabled layers playing at once.
    pub walk_min_layers: u32,
    /// Walk rate multiplier (1.0 ≈ minutes-scale evolution).
    pub walk_speed: f32,
    /// Global multiplier on every layer's walk amount: how FAR parameters wander.
    /// 1.0 = subtle; 2-3 = clearly visible evolution.
    pub walk_depth: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BeatTime {
    Half,
    #[default]
    Normal,
    Double,
}

impl BeatTime {
    pub fn multiplier(self) -> f32 {
        match self {
            Self::Half => 0.5,
            Self::Normal => 1.0,
            Self::Double => 2.0,
        }
    }
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            fps: 60.0,
            master_brightness: 1.0,
            master_speed: 1.0,
            manual_bpm: None,
            beat_time: BeatTime::Normal,
            walk_enabled: true,
            walk_layers: false,
            walk_min_layers: 2,
            walk_speed: 1.0,
            walk_depth: 1.0,
        }
    }
}

/// Automated "taps": fire a burst on every beat at a point that orbits the ring —
/// the automated version of tapping the preview in a circle on the beat, which
/// makes fun spiral effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BeatTapConfig {
    pub enabled: bool,
    /// Which audio source's beat drives the taps.
    pub audio_source: u32,
    /// Orbit speed in turns per beat; negative reverses. 0.0833 = one lap / 12 beats.
    pub spin: f32,
    /// Slowly drift the spin speed (autopilot-style), so the spiral keeps changing.
    pub vary: bool,
    /// Tap position radius, 0 (center) .. 1 (outer edge).
    pub radius: f32,
    pub intensity: f32,
    /// Hue in turns; negative = white.
    pub hue: f32,
    /// Fire on every Nth beat (1 = every beat).
    pub every: u32,
}

impl Default for BeatTapConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            audio_source: 0,
            spin: 0.0833,
            vary: true,
            radius: 0.8,
            intensity: 0.7,
            hue: -1.0,
            every: 1,
        }
    }
}

/// Self-update behavior (see `updater.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Poll GitHub Releases for newer versions (startup + every 6 h).
    pub auto_check: bool,
    /// Install updates as soon as they are found. The swap is a seamless takeover,
    /// but taking an update mid-show is the operator's call — off by default.
    pub auto_install: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_install: false,
        }
    }
}

/// Desktop window bookkeeping, so a restart (or self-update handover) restores the
/// same set of windows. Geometry itself is persisted per-label by
/// tauri-plugin-window-state; this only remembers WHICH aux windows were open.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsConfig {
    /// Tabs with a popped-out window open (e.g. "control", "live").
    pub aux_open: Vec<String>,
}

/// A named, reusable capture of the layer stack and the motion settings that
/// shape it. Saved with the main config so every control device sees the same
/// library and it survives backend restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedStack {
    pub id: String,
    pub name: String,
    pub layers: Vec<LayerCfg>,
    pub master_speed: f32,
    pub walk_enabled: bool,
    pub walk_layers: bool,
    pub walk_min_layers: u32,
    pub walk_speed: f32,
    pub walk_depth: f32,
}

impl Default for SavedStack {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "Untitled stack".into(),
            layers: Vec::new(),
            master_speed: 1.0,
            walk_enabled: false,
            walk_layers: false,
            walk_min_layers: 1,
            walk_speed: 1.0,
            walk_depth: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub geometry: GeometryConfig,
    pub output: OutputConfig,
    pub server: ServerConfig,
    pub audio: AudioConfig,
    pub rhythm: RhythmConfig,
    pub render: RenderConfig,
    pub update: UpdateConfig,
    pub windows: WindowsConfig,
    pub beat_taps: BeatTapConfig,
    pub layers: Vec<LayerCfg>,
    /// Named layer-stack captures shared by all clients.
    pub saved_stacks: Vec<SavedStack>,
    /// Known client devices (see `ClientRecord`).
    pub clients: Vec<ClientRecord>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            geometry: GeometryConfig::default(),
            output: OutputConfig::default(),
            server: ServerConfig::default(),
            audio: AudioConfig::default(),
            rhythm: RhythmConfig::default(),
            render: RenderConfig::default(),
            update: UpdateConfig::default(),
            windows: WindowsConfig::default(),
            beat_taps: BeatTapConfig::default(),
            layers: default_layer_stack(),
            saved_stacks: Vec::new(),
            clients: Vec::new(),
        }
    }
}

/// A stack that looks good out of the box: deep noise base, harmonic rings riding
/// the bass, sparkles on the treble, and beat rings.
fn default_layer_stack() -> Vec<LayerCfg> {
    vec![
        LayerCfg {
            kind: LayerKind::NoiseColor,
            name: "Nebula base".into(),
            blend: BlendMode::AlphaOver,
            opacity: 1.0,
            speed: 0.25,
            scale: 1.2,
            audio_amount: 0.3,
            hue: 0.65,
            hue_range: 0.25,
            brightness: 0.5,
            ..Default::default()
        },
        LayerCfg {
            kind: LayerKind::RadialWaves,
            name: "Harmonic rings".into(),
            blend: BlendMode::Add,
            opacity: 0.6,
            speed: 1.0,
            scale: 1.0,
            audio_amount: 0.8,
            hue: 0.55,
            hue_range: 0.1,
            param_a: 3.0,
            param_b: 4.0,
            ..Default::default()
        },
        LayerCfg {
            kind: LayerKind::Sparkle,
            name: "Treble glitter".into(),
            blend: BlendMode::Add,
            opacity: 0.7,
            speed: 1.0,
            audio_amount: 0.9,
            hue: 0.12,
            hue_range: 0.05,
            saturation: 0.3,
            param_a: 0.15,
            ..Default::default()
        },
        LayerCfg {
            kind: LayerKind::BeatRings,
            name: "Beat rings".into(),
            blend: BlendMode::Add,
            opacity: 0.8,
            audio_amount: 1.0,
            hue: 0.85,
            hue_range: 0.0,
            param_a: 0.08,
            ..Default::default()
        },
    ]
}

pub fn config_path() -> PathBuf {
    // Override for tests / portable installs / running several isolated instances.
    if let Ok(p) = std::env::var("EMPYREAN_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("EmpyreanGate")
        .join("config.json")
}

pub fn load() -> AppConfig {
    let path = config_path();
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(cfg) => {
                log::info!("loaded config from {}", path.display());
                cfg
            }
            Err(e) => {
                log::error!("config at {} is invalid ({e}); using defaults", path.display());
                AppConfig::default()
            }
        },
        Err(_) => {
            log::info!("no config at {}; using defaults", path.display());
            AppConfig::default()
        }
    };
    // First-run identities. Both must be written back immediately: the sACN CID in
    // particular is only useful if it is the SAME one next launch.
    let mut dirty = false;
    if cfg.server.join_token.is_empty() {
        cfg.server.join_token = generate_token();
        dirty = true;
    }
    if cfg.output.cid.is_empty() {
        cfg.output.cid = uuid::Uuid::new_v4().to_string();
        log::info!("generated sACN source CID {}", cfg.output.cid);
        dirty = true;
    }
    if dirty {
        save(&cfg);
    }
    // Isolated integration tests and parallel local instances can choose a port
    // without rewriting the persisted operator configuration.
    if let Some(port) = std::env::var("EMPYREAN_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        cfg.server.port = port;
    }
    cfg
}

pub fn save(cfg: &AppConfig) {
    let path = config_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(cfg) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                log::error!("failed to save config to {}: {e}", path.display());
            }
        }
        Err(e) => log::error!("failed to serialize config: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_without_rhythm_section_keeps_legacy_behavior() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("rhythm");
        let config: AppConfig = serde_json::from_value(value).unwrap();
        assert_eq!(config.rhythm.source, RhythmSource::LayerAudio);
        assert!(config.rhythm.fallback_to_audio);
    }
}
