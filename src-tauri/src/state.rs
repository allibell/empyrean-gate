//! Shared state between the frame-generation thread (primary), the audio threads,
//! the WebSocket server, and the Tauri shell. Everything is lock-light: the frame
//! loop takes short locks to snapshot inputs and never blocks on clients.

use crate::config::AppConfig;
use crate::layers::{EffectCfg, MAX_AUDIO_SOURCES};
use crate::protocol::{RuntimeStatus, ServerMsg};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;

/// Per-source audio features, written by an analysis chain, read by the frame loop.
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioFeatures {
    pub active: bool,
    /// See `audio::HEALTH_*` — surfaces "waiting for device" states to the UI.
    pub health: u8,
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    /// Smoothed (~0.25 s) twins of the bands, MilkDrop-style: `bass / bass_att`
    /// is per-band punch; `bass_att` is the groove.
    pub bass_att: f32,
    pub mid_att: f32,
    pub treble_att: f32,
    /// Decaying onset pulse (1.0 at each detected onset).
    pub onset: f32,
    /// 0..1 phase within the current beat.
    pub beat_phase: f32,
    pub bpm: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_frames_are_bounded_and_owned_by_one_connection() {
        let state = SharedState::new(AppConfig::default());
        state.start_video(7, "ipad", "clip".into(), "https://example.com/clip.mp4".into());
        let rgba = vec![42u8; 4 * 3 * 4];

        assert!(!state.push_video_frame(8, 4, 3, &rgba), "wrong connection");
        assert!(!state.push_video_frame(7, 0, 3, &[]), "zero width");
        assert!(!state.push_video_frame(7, 4, 3, &rgba[..rgba.len() - 1]), "bad payload size");
        assert!(state.push_video_frame(7, 4, 3, &rgba));

        {
            let video = state.video.lock();
            assert!(video.active);
            assert_eq!((video.width, video.height), (4, 3));
            assert_eq!(video.frames, 1);
            assert_eq!(video.rgba, rgba);
        }

        state.stop_video(Some(8));
        assert!(state.video.lock().active, "a stale connection cannot stop the owner");
        state.stop_video(Some(7));
        assert!(!state.video.lock().active);
        assert!(state.video.lock().rgba.is_empty());
    }
}

/// Raw audio shapes shipped to the GPU each frame: the recent waveform (a ring
/// oscilloscope's worth) and the log-spaced spectrum. Written by capture threads.
pub struct ScopeData {
    pub wave: [f32; 256],
    pub spectrum: [f32; crate::audio::analysis::SPECTRUM_BINS],
}

impl Default for ScopeData {
    fn default() -> Self {
        Self {
            wave: [0.0; 256],
            spectrum: [0.0; crate::audio::analysis::SPECTRUM_BINS],
        }
    }
}

/// Control inputs from remote phones (IMU) — a small global "control bus".
#[derive(Debug, Clone, Copy, Default)]
pub struct ControlInputs {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// Decays over time; spiked by phone shakes.
    pub shake: f32,
}

pub struct ActiveEffect {
    pub cfg: EffectCfg,
    pub born: Instant,
}

pub struct ActiveDab {
    pub kind: crate::layers::PenKind,
    pub angle: f32,
    pub radius: f32,
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub size: f32,
    pub intensity: f32,
    pub dir: f32,
    pub born: Instant,
}

/// One frame as produced by the engine: raw perceptual RGB (no LED gamma).
pub struct PreviewFrame {
    pub frame_number: u64,
    pub spokes: u32,
    pub pixels_per_spoke: u32,
    pub rgb: Vec<u8>,
}

/// Rations concurrent preview streams (the bandwidth-heavy part of a client) to
/// `max` slots; everyone else waits FIFO. Control traffic is never gated.
#[derive(Default)]
pub struct PreviewGate {
    pub active: Vec<u64>,
    pub waiting: Vec<u64>,
}

impl PreviewGate {
    /// Register interest; returns true if immediately active.
    pub fn request(&mut self, conn: u64, max: usize) -> bool {
        if self.active.contains(&conn) {
            return true;
        }
        if !self.waiting.contains(&conn) {
            self.waiting.push(conn);
        }
        self.promote(max);
        self.active.contains(&conn)
    }

    pub fn release(&mut self, conn: u64, max: usize) {
        self.active.retain(|c| *c != conn);
        self.waiting.retain(|c| *c != conn);
        self.promote(max);
    }

    /// True when this connection holds a slot; promotes waiters as slots free up.
    pub fn is_active(&mut self, conn: u64, max: usize) -> bool {
        self.promote(max);
        self.active.contains(&conn)
    }

    /// 1-based queue position, if waiting.
    pub fn position(&self, conn: u64) -> Option<u32> {
        self.waiting.iter().position(|c| *c == conn).map(|p| p as u32 + 1)
    }

    fn promote(&mut self, max: usize) {
        while self.active.len() < max && !self.waiting.is_empty() {
            let next = self.waiting.remove(0);
            self.active.push(next);
        }
        // A lowered cap sheds the newest active first.
        while self.active.len() > max {
            if let Some(demoted) = self.active.pop() {
                self.waiting.insert(0, demoted);
            }
        }
    }
}

/// Latest browser-decoded video frame. Only the newest frame is retained: video
/// input is a real-time control signal, so network jitter drops frames instead of
/// building latency.
pub struct VideoInput {
    pub active: bool,
    pub owner_conn_id: u64,
    pub owner_id: String,
    pub title: String,
    pub source_url: String,
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
    pub revision: u64,
    pub frames: u64,
    pub fps: f32,
    last_frame: Option<Instant>,
}

impl Default for VideoInput {
    fn default() -> Self {
        Self {
            active: false,
            owner_conn_id: 0,
            owner_id: String::new(),
            title: String::new(),
            source_url: String::new(),
            width: 0,
            height: 0,
            rgba: Vec::new(),
            revision: 0,
            frames: 0,
            fps: 0.0,
            last_frame: None,
        }
    }
}

pub struct SharedState {
    pub config: RwLock<AppConfig>,
    /// Bumped on every config change; threads compare to notice reconfiguration.
    pub config_epoch: AtomicU32,
    pub effects: Mutex<Vec<ActiveEffect>>,
    pub dabs: Mutex<Vec<ActiveDab>>,
    pub audio: [Mutex<AudioFeatures>; MAX_AUDIO_SOURCES],
    pub scope: [Mutex<ScopeData>; MAX_AUDIO_SOURCES],
    pub control: Mutex<ControlInputs>,
    pub status: Mutex<RuntimeStatus>,
    pub shutdown: AtomicBool,
    /// Per-layer animation phases, owned by the engine loop but shared so a
    /// handover can transplant them into a successor instance.
    pub layer_phases: Mutex<Vec<f64>>,
    /// Set after writing transplanted phases into `layer_phases`; the engine loop
    /// swaps it false and adopts them.
    pub phases_transplanted: AtomicBool,
    /// sACN gated off while a takeover from an older instance is in progress
    /// (this instance must not send before the old one has stopped).
    pub sacn_hold: AtomicBool,
    /// Set once this instance has granted a handover: stop sACN immediately and
    /// shut down shortly after.
    pub leaving: AtomicBool,
    /// Engine ack that it observed `leaving` and skipped a send — after this, no
    /// more packets will ever leave this instance (the commit reply waits on it).
    pub sacn_quiesced: AtomicBool,
    /// Engine ack that it has sent (or deliberately skipped) E1.31 stream
    /// termination on shutdown. Process exit waits briefly on this, otherwise the
    /// terminate packets never make it out of the socket.
    pub sacn_terminated: AtomicBool,
    /// Total frames rendered; the takeover waits for its adopted config to have
    /// flowed through the render+readback pipeline before committing.
    pub frames_rendered: AtomicU64,
    /// Set by the UI/auto-check to ask the updater thread to act.
    pub update_check_requested: AtomicBool,
    pub update_install_requested: AtomicBool,
    /// Whether this instance runs headless (the updater passes it to a successor).
    pub headless: AtomicBool,
    /// Preview-stream slot rationing (see `PreviewGate`).
    pub preview_gate: Mutex<PreviewGate>,
    /// Currently-connected WS clients: connection serial -> client id.
    pub connected_clients: Mutex<HashMap<u64, String>>,
    pub conn_seq: AtomicU64,
    /// JSON events fanned out to every connected client.
    pub events: broadcast::Sender<ServerMsg>,
    /// Full-resolution frames; each client task decimates/throttles for itself.
    pub preview: broadcast::Sender<Arc<PreviewFrame>>,
    pub video: Mutex<VideoInput>,
    pub started: Instant,
}

impl SharedState {
    pub fn new(config: AppConfig) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        let (preview, _) = broadcast::channel(4);
        Arc::new(Self {
            config: RwLock::new(config),
            config_epoch: AtomicU32::new(0),
            effects: Mutex::new(Vec::new()),
            dabs: Mutex::new(Vec::new()),
            audio: Default::default(),
            scope: Default::default(),
            control: Mutex::new(ControlInputs::default()),
            status: Mutex::new(RuntimeStatus::default()),
            shutdown: AtomicBool::new(false),
            layer_phases: Mutex::new(Vec::new()),
            phases_transplanted: AtomicBool::new(false),
            sacn_hold: AtomicBool::new(false),
            leaving: AtomicBool::new(false),
            sacn_quiesced: AtomicBool::new(false),
            sacn_terminated: AtomicBool::new(false),
            frames_rendered: AtomicU64::new(0),
            update_check_requested: AtomicBool::new(false),
            update_install_requested: AtomicBool::new(false),
            headless: AtomicBool::new(false),
            preview_gate: Mutex::new(PreviewGate::default()),
            connected_clients: Mutex::new(HashMap::new()),
            conn_seq: AtomicU64::new(1),
            events,
            preview,
            video: Mutex::new(VideoInput::default()),
            started: Instant::now(),
        })
    }

    pub fn bump_config(&self) {
        self.config_epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn epoch(&self) -> u32 {
        self.config_epoch.load(Ordering::SeqCst)
    }

    /// Mutate the config, persist it, and notify all clients with fresh state.
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) {
        let snapshot = {
            let mut cfg = self.config.write();
            f(&mut cfg);
            cfg.clone()
        };
        crate::config::save(&snapshot);
        self.bump_config();
        self.broadcast_state();
    }

    pub fn broadcast_state(&self) {
        let config = Box::new(self.config.read().clone());
        let status = self.status.lock().clone();
        let _ = self.events.send(ServerMsg::State { config, status });
    }

    /// Add live-draw dabs; the oldest are evicted when the buffer is full so a
    /// crowd drawing at once degrades gracefully instead of erroring.
    pub fn paint(
        &self,
        kind: crate::layers::PenKind,
        points: &[crate::layers::DabPoint],
        hue: f32,
        saturation: f32,
        brightness: f32,
        size: f32,
        intensity: f32,
    ) {
        let mut dabs = self.dabs.lock();
        for p in points {
            if dabs.len() >= crate::layers::MAX_DABS {
                dabs.remove(0);
            }
            dabs.push(ActiveDab {
                kind,
                angle: p.angle,
                radius: p.radius.clamp(0.0, 1.2),
                hue,
                saturation: saturation.clamp(0.0, 1.0),
                brightness: brightness.clamp(0.0, 1.0),
                size: size.clamp(0.01, 1.0),
                intensity: intensity.clamp(0.0, 2.0),
                dir: p.dir,
                born: Instant::now(),
            });
        }
    }

    pub fn trigger_effect(&self, cfg: EffectCfg) {
        let mut effects = self.effects.lock();
        // Cap active effects; drop the oldest if the floor is spamming taps.
        if effects.len() >= crate::layers::MAX_EFFECTS {
            effects.remove(0);
        }
        effects.push(ActiveEffect {
            cfg,
            born: Instant::now(),
        });
    }

    pub fn start_video(&self, conn_id: u64, owner_id: &str, title: String, source_url: String) {
        let mut v = self.video.lock();
        v.active = true;
        v.owner_conn_id = conn_id;
        v.owner_id = owner_id.to_owned();
        v.title = title.chars().take(160).collect();
        v.source_url = source_url.chars().take(2048).collect();
        v.width = 0;
        v.height = 0;
        v.rgba.clear();
        v.frames = 0;
        v.fps = 0.0;
        v.last_frame = None;
        v.revision = v.revision.wrapping_add(1);
    }

    /// Stop the current source, returning whether this caller actually owned (or
    /// force-stopped) it. The return value lets the server clear soundtrack data
    /// without a stale disconnect blanking the new owner's beat source.
    pub fn stop_video(&self, conn_id: Option<u64>) -> bool {
        let mut v = self.video.lock();
        if conn_id.is_some_and(|id| id != v.owner_conn_id) {
            return false;
        }
        v.active = false;
        v.owner_conn_id = 0;
        v.width = 0;
        v.height = 0;
        v.rgba.clear();
        v.fps = 0.0;
        v.last_frame = None;
        v.revision = v.revision.wrapping_add(1);
        true
    }

    pub fn push_video_frame(
        &self,
        conn_id: u64,
        width: u16,
        height: u16,
        rgba: &[u8],
    ) -> bool {
        let expected = width as usize * height as usize * 4;
        if width == 0
            || height == 0
            || width > crate::protocol::MAX_VIDEO_DIMENSION
            || height > crate::protocol::MAX_VIDEO_DIMENSION
            || rgba.len() != expected
        {
            return false;
        }
        let mut v = self.video.lock();
        if !v.active || v.owner_conn_id != conn_id {
            return false;
        }
        let now = Instant::now();
        if let Some(last) = v.last_frame {
            let instant_fps = 1.0 / now.duration_since(last).as_secs_f32().max(0.001);
            v.fps = if v.fps == 0.0 {
                instant_fps
            } else {
                v.fps * 0.85 + instant_fps * 0.15
            };
        }
        v.last_frame = Some(now);
        v.width = width;
        v.height = height;
        v.rgba.clear();
        v.rgba.extend_from_slice(rgba);
        v.frames += 1;
        v.revision = v.revision.wrapping_add(1);
        true
    }
}
