//! The WebSocket wire protocol shared by every UI client (the Tauri webview, browsers
//! on the LAN, phones). Text frames carry JSON messages; preview frames are binary
//! (see `PREVIEW_MAGIC` layout below).

use crate::config::AppConfig;
use crate::layers::{DabPoint, EffectCfg, LayerCfg, PenKind};
use serde::{Deserialize, Serialize};

/// Binary preview frame layout (little endian):
/// `u32 magic, u32 frame_number, u16 spokes, u16 pixels_per_spoke_after_decimation,`
/// then `spokes * pixels` RGB triplets (pixel 0 = outer end of spoke).
pub const PREVIEW_MAGIC: u32 = 0x4547_5056; // "VPGE"

/// Binary video input frame (client -> backend), little endian:
/// `u32 magic, u32 sequence, u16 width, u16 height`, then RGBA8 pixels.
pub const VIDEO_FRAME_MAGIC: u32 = 0x4547_5646; // "FVGE"
pub const MAX_VIDEO_DIMENSION: u16 = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAudioStream {
    #[default]
    Microphone,
    Video,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        #[serde(default)]
        name: String,
        /// For remote audio sources: matched against `AudioSourceKind::Remote.client_id`.
        #[serde(default)]
        client_id: String,
        /// Auth token; only checked when `server.auth_token` is configured.
        #[serde(default)]
        token: String,
    },
    GetState,
    /// Full config replace (settings page "everything" updates go through this).
    SetConfig {
        config: Box<AppConfig>,
    },
    SetMaster {
        #[serde(default)]
        brightness: Option<f32>,
        #[serde(default)]
        speed: Option<f32>,
    },
    SetSacnEnabled {
        enabled: bool,
    },
    AddLayer {
        layer: LayerCfg,
    },
    UpdateLayer {
        index: usize,
        layer: LayerCfg,
    },
    RemoveLayer {
        index: usize,
    },
    MoveLayer {
        from: usize,
        to: usize,
    },
    TriggerEffect {
        effect: EffectCfg,
    },
    /// Live drawing: a batch of stroke points (coalesced per pointer frame) painted
    /// with the given pen. Collaborative — dabs from all clients merge.
    Paint {
        pen: PenKind,
        points: Vec<DabPoint>,
        /// Hue in turns; negative = white.
        #[serde(default)]
        hue: f32,
        #[serde(default = "default_saturation")]
        saturation: f32,
        #[serde(default = "default_brightness")]
        brightness: f32,
        /// Dab radius as a fraction of the array radius.
        #[serde(default = "default_dab_size")]
        size: f32,
        #[serde(default = "default_intensity")]
        intensity: f32,
    },
    SubscribePreview {
        /// Max frames per second this client wants.
        fps: f32,
        /// Keep every Nth pixel along each spoke (1 = full resolution).
        decimate: u32,
    },
    UnsubscribePreview,
    /// Claim the single live video input. Binary VIDEO_FRAME_MAGIC messages from
    /// this connection are accepted until it stops or disconnects.
    StartVideo {
        #[serde(default)]
        title: String,
        #[serde(default)]
        source_url: String,
    },
    StopVideo {
        /// Normal cleanup may stop only the caller's own source. An explicit UI
        /// action can force-stop another connected device's source.
        #[serde(default)]
        force: bool,
    },
    /// Audio features computed client-side from a remote browser microphone or
    /// from the soundtrack of the browser-decoded video.
    /// Sent at the client's analysis hop rate (~40 Hz).
    AudioFrame {
        /// Absent means microphone for backwards compatibility with older UIs.
        #[serde(default)]
        stream: BrowserAudioStream,
        level: f32,
        bass: f32,
        mid: f32,
        treble: f32,
        /// Rectified spectral flux — the onset signal the beat tracker consumes.
        flux: f32,
    },
    /// Set this device's own friendly name (shown in the Clients panel).
    SetClientName {
        name: String,
    },
    /// Operator: rename a known client.
    RenameClient {
        id: String,
        name: String,
    },
    /// Operator: revoke a client — kicks it live and blocks rejoin by id.
    RevokeClient {
        id: String,
    },
    UnrevokeClient {
        id: String,
    },
    /// Operator: forget a disconnected client record entirely.
    ForgetClient {
        id: String,
    },
    /// Operator: replace the join token (invalidates old QR codes when
    /// `require_token` is on).
    RotateJoinToken,
    SetRequireToken {
        require: bool,
    },
    /// Create the Windows Firewall port rule (one UAC prompt on the Gate machine).
    AuthorizeFirewall,
    /// Ask the updater to poll GitHub Releases now.
    CheckUpdate,
    /// Download + hot-swap to the staged update (two-phase takeover).
    InstallUpdate,
    /// Windows only: create/remove this user's Startup-folder shortcut.
    SetLaunchAtStartup {
        enabled: bool,
    },
    /// Phone orientation / motion, mapped onto the global control bus.
    Imu {
        /// Compass-ish heading in radians.
        yaw: f32,
        /// Forward/back tilt, roughly -1..1.
        pitch: f32,
        /// Left/right tilt, roughly -1..1.
        roll: f32,
        /// Acceleration magnitude (shake), m/s^2 above gravity.
        #[serde(default)]
        shake: f32,
    },
}

fn default_dab_size() -> f32 {
    0.12
}

fn default_intensity() -> f32 {
    1.0
}

fn default_saturation() -> f32 {
    0.85
}

fn default_brightness() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    State {
        config: Box<AppConfig>,
        status: RuntimeStatus,
    },
    Status {
        status: RuntimeStatus,
    },
    Beat {
        source: u32,
        bpm: f32,
    },
    PreviewMeta {
        spokes: u32,
        pixels: u32,
        decimate: u32,
        outer_radius_ft: f32,
        inner_radius_ft: f32,
    },
    Error {
        message: String,
    },
    /// Access refused (revoked, or join token required). The client stops
    /// reconnecting and shows the reason.
    Denied {
        reason: String,
    },
    /// The live-preview slots are full; this client is queued at `position`
    /// (1 = next). Control input still works while waiting. Sent with position 0
    /// when a slot is granted after queueing.
    PreviewQueue {
        position: u32,
    },
}

/// Everything a freshly-started backend needs to take over from this one with
/// visual continuity (see `POST /handover`).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct HandoverGrant {
    pub config: AppConfig,
    /// Per-layer animation phases, so patterns continue instead of jumping.
    pub layer_phases: Vec<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ClientInfo {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub revoked: bool,
}

/// An audio device as shown in the settings UI.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceInfo {
    pub name: String,
    /// Channel count of the device's default config (0 = unknown).
    pub channels: u16,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioSourceStatus {
    pub id: String,
    pub active: bool,
    /// Human-readable health note, e.g. "waiting for device". Empty when running.
    pub detail: String,
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub bpm: f32,
    /// 0..1 confidence in `bpm`; UIs hide or dim the number when low.
    pub bpm_confidence: f32,
    pub beat_phase: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RhythmStatus {
    /// True when the configured clock is driving the lights. For MIDI this means
    /// recent clock pulses, a valid tempo, and no explicit transport Stop.
    pub active: bool,
    pub using_fallback: bool,
    pub source: String,
    pub detail: String,
    pub bpm: f32,
    pub beat_phase: f32,
    pub running: bool,
    pub age_ms: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProDjLinkDeviceInfo {
    pub number: u8,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VideoSourceStatus {
    pub active: bool,
    pub owner_id: String,
    pub owner_name: String,
    pub title: String,
    pub source_url: String,
    pub width: u16,
    pub height: u16,
    pub fps: f32,
    pub frames: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScheduledShowStatus {
    pub enabled: bool,
    pub playlist_id: String,
    pub playlist_name: String,
    pub scene_name: String,
    /// Zero-based active entry.
    pub index: u32,
    pub total: u32,
    pub remaining_secs: f32,
    /// 0 outside a transition; otherwise 0..1 as the incoming scene arrives.
    pub transition_progress: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuntimeStatus {
    /// Set when Vulkan init failed — the UI shows this prominently. No fallbacks.
    pub gpu_error: Option<String>,
    pub gpu_name: String,
    pub engine_fps: f32,
    pub frame_time_ms: f32,
    pub sacn_enabled: bool,
    pub sacn_universes: u16,
    /// sACN packets actually sent per second — the "is it transmitting" truth.
    /// (Last full one-second bucket.)
    pub sacn_pps: u32,
    /// Frames rendered in each of the last ~30 one-second buckets (oldest first).
    pub fps_history: Vec<u32>,
    /// sACN packets sent in each of the last ~30 one-second buckets (oldest first).
    pub pps_history: Vec<u32>,
    pub clients: u32,
    pub audio: Vec<AudioSourceStatus>,
    pub rhythm: RhythmStatus,
    /// Hot-plug refreshed MIDI input names.
    pub midi_ports: Vec<String>,
    pub pro_dj_link_devices: Vec<ProDjLinkDeviceInfo>,
    /// Available local capture devices, for the settings UI dropdowns.
    pub input_devices: Vec<DeviceInfo>,
    /// Output devices (selectable as loopback beat sources).
    pub output_devices: Vec<DeviceInfo>,
    /// Channel counts of the default devices (0 = unknown), so the UI can render
    /// per-channel checkboxes even when "system default" is selected.
    pub default_input_channels: u16,
    pub default_output_channels: u16,
    /// Local IPv4 interfaces as "name — ip", for the sACN interface picker.
    pub interfaces: Vec<String>,
    /// Windows only: the firewall allow rule for our port is missing, so LAN
    /// clients may be blocked (and every new binary re-triggers the security
    /// prompt). The UI offers one-click authorization.
    pub firewall_pending: bool,
    /// Whether this platform can create a per-user Startup-folder shortcut.
    pub startup_supported: bool,
    /// The shortcut's observed state (not merely the persisted preference).
    pub startup_enabled: bool,
    /// Human-readable result or unsupported/error note.
    pub startup_state: String,
    /// Cache/download state per video playlist entry.
    pub video_cache: Vec<crate::videocache::VideoCacheStatus>,
    /// Known + connected client devices.
    pub client_list: Vec<ClientInfo>,
    pub master_brightness: f32,
    pub master_speed: f32,
    /// Running app version (CARGO_PKG_VERSION).
    pub version: String,
    /// Newer release version, when one is known.
    pub update_available: Option<String>,
    /// Updater progress / result note ("up to date", "downloading…", errors).
    pub update_state: String,
    pub video: VideoSourceStatus,
    pub show: ScheduledShowStatus,
}

#[cfg(test)]
mod startup_tests {
    use super::ClientMsg;

    #[test]
    fn launch_at_startup_message_is_explicitly_typed() {
        let message: ClientMsg = serde_json::from_str(
            r#"{"type":"set_launch_at_startup","enabled":true}"#,
        )
        .unwrap();
        assert!(matches!(
            message,
            ClientMsg::SetLaunchAtStartup { enabled: true }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_stop_video_message_is_owner_scoped() {
        let message: ClientMsg = serde_json::from_str(r#"{"type":"stop_video"}"#).unwrap();
        assert!(matches!(message, ClientMsg::StopVideo { force: false }));
    }

    #[test]
    fn legacy_audio_frame_defaults_to_microphone() {
        let message: ClientMsg = serde_json::from_str(
            r#"{"type":"audio_frame","level":0.1,"bass":0.2,"mid":0.3,"treble":0.4,"flux":0.5}"#,
        )
        .unwrap();
        assert!(matches!(
            message,
            ClientMsg::AudioFrame {
                stream: BrowserAudioStream::Microphone,
                ..
            }
        ));
    }

    #[test]
    fn legacy_paint_defaults_to_the_original_color_profile() {
        let message: ClientMsg = serde_json::from_str(
            r#"{"type":"paint","pen":"glow","points":[],"hue":0.5,"size":0.12,"intensity":1.0}"#,
        )
        .unwrap();
        assert!(matches!(
            message,
            ClientMsg::Paint {
                saturation: 0.85,
                brightness: 1.0,
                ..
            }
        ));
    }
}
