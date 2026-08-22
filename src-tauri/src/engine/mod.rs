//! The GPU engine and the frame-generation loop — the primary feature of this app.
//! Runs on a dedicated OS thread, fully independent of any UI.
//!
//! wgpu locked to the Vulkan backend, deliberately: no fallbacks, just a clear error
//! surfaced to every client when Vulkan is unavailable.
//!
//! Readback uses two staging buffers in ping-pong: while the GPU computes frame N,
//! the CPU maps and distributes frame N-1 (sACN + preview). One frame of latency,
//! zero pipeline stalls.

use crate::layers::{GpuDab, GpuEffect, GpuLayer, MAX_AUDIO_SOURCES, MAX_DABS, MAX_EFFECTS, MAX_LAYERS};
use crate::protocol::{ScheduledShowStatus, ServerMsg, MAX_VIDEO_DIMENSION};
use crate::sacn::SacnSender;
use crate::state::{PreviewFrame, SharedState};
use anyhow::{Context, Result};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A scene may use every operator-visible layer slot; a true crossfade needs room
/// for one complete outgoing stack and one complete incoming stack at once.
const MAX_RENDER_LAYERS: usize = MAX_LAYERS * 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub spokes: u32,
    pub pixels: u32,
    pub layer_count: u32,
    pub effect_count: u32,
    pub time: f32,
    pub dt: f32,
    pub master: f32,
    pub inner_over_outer: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub shake: f32,
    pub yaw: f32,
    pub dab_count: u32,
    pub video_width: u32,
    pub video_height: u32,
    pub video_active: u32,
    /// Number of leading GPU layers that belong to the outgoing scene.
    pub transition_split: u32,
    /// Non-zero while the shader should compose two independent scene stacks.
    pub transition_active: u32,
    /// Smoothed 0..1 mix from the outgoing scene to the incoming scene.
    pub transition_progress: f32,
    pub _pad_transition: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AudioUniform {
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub onset: f32,
    pub beat_phase: f32,
    pub bpm: f32,
    /// 0..1 confidence in `bpm`; shaders and automation gate on it.
    pub bpm_conf: f32,
    pub bass_att: f32,
    pub mid_att: f32,
    pub treble_att: f32,
    pub _pad2: f32,
}

/// Floats per source in the scope storage buffer (256 waveform + 64 spectrum).
pub const SCOPE_FLOATS: usize = 256 + crate::audio::analysis::SPECTRUM_BINS;

pub struct FrameInputs {
    pub globals: Globals,
    pub audio: [AudioUniform; MAX_AUDIO_SOURCES],
    pub layers: Vec<GpuLayer>,
    pub effects: Vec<GpuEffect>,
    pub dabs: Vec<GpuDab>,
    /// Per-source waveform + spectrum, flattened (see `SCOPE_FLOATS`).
    pub scope: Vec<f32>,
    /// Present only when a newer browser-decoded frame needs uploading.
    pub video_upload: Option<Vec<u8>>,
}

pub struct Engine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    audio_buf: wgpu::Buffer,
    layers_buf: wgpu::Buffer,
    effects_buf: wgpu::Buffer,
    dabs_buf: wgpu::Buffer,
    scope_buf: wgpu::Buffer,
    video_buf: wgpu::Buffer,
    out_buf: wgpu::Buffer,
    staging: [wgpu::Buffer; 2],
    /// Submission index of the copy targeting each staging buffer.
    staging_submission: [Option<wgpu::SubmissionIndex>; 2],
    npix: u32,
    frame: u64,
    pub gpu_name: String,
    readback: Vec<u8>,
}

fn shader_source() -> String {
    #[cfg(feature = "shader-hot-reload")]
    {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/engine/shaders/gate.wgsl");
        if let Ok(src) = std::fs::read_to_string(path) {
            return src;
        }
    }
    include_str!("shaders/gate.wgsl").to_string()
}

impl Engine {
    pub fn new(npix: u32) -> Result<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        // Validation/debug layers only on request: the SDK validation layer has been
        // seen to crash inside vkCreateDevice on some drivers (Intel UHD + SDK 1.4.304).
        // Set EMPYREAN_GPU_DEBUG=1 to opt in when debugging GPU issues.
        desc.flags = if std::env::var("EMPYREAN_GPU_DEBUG").is_ok_and(|v| v == "1") {
            wgpu::InstanceFlags::debugging()
        } else {
            wgpu::InstanceFlags::empty()
        };
        let instance = wgpu::Instance::new(desc);

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .context(
            "No Vulkan adapter found. This app requires a working Vulkan driver (no fallback \
             renderer, by design). Install or update your GPU driver and verify with `vulkaninfo`.",
        )?;

        let info = adapter.get_info();
        let gpu_name = format!("{} ({:?})", info.name, info.backend);
        log::info!("using adapter: {gpu_name}");

        log::debug!("requesting device");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("empyrean"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))
            .context("Vulkan device creation failed")?;
        log::debug!("device created; allocating buffers");

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let audio_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("audio"),
            size: (std::mem::size_of::<AudioUniform>() * MAX_AUDIO_SOURCES) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layers_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layers"),
            size: (std::mem::size_of::<GpuLayer>() * MAX_RENDER_LAYERS) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let effects_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("effects"),
            size: (std::mem::size_of::<GpuEffect>() * MAX_EFFECTS) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dabs"),
            size: (std::mem::size_of::<GpuDab>() * MAX_DABS) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scope_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scope"),
            size: (SCOPE_FLOATS * MAX_AUDIO_SOURCES * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let video_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video-rgba"),
            size: MAX_VIDEO_DIMENSION as u64 * MAX_VIDEO_DIMENSION as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (out_buf, staging) = Self::make_pixel_buffers(&device, npix);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gate"),
            entries: &[
                uniform_entry(0),
                uniform_entry(1),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, false),
                storage_entry(5, true),
                storage_entry(6, true),
                storage_entry(7, true),
            ],
        });

        let mut engine = Self {
            bind_group: Self::make_bind_group(
                &device,
                &bind_group_layout,
                &globals_buf,
                &audio_buf,
                &layers_buf,
                &effects_buf,
                &out_buf,
                &dabs_buf,
                &scope_buf,
                &video_buf,
            ),
            pipeline: Self::make_pipeline(&device, &bind_group_layout)?,
            device,
            queue,
            bind_group_layout,
            globals_buf,
            audio_buf,
            layers_buf,
            effects_buf,
            dabs_buf,
            scope_buf,
            video_buf,
            out_buf,
            staging,
            staging_submission: [None, None],
            npix,
            frame: 0,
            gpu_name,
            readback: Vec::new(),
        };
        engine.readback = vec![0u8; (npix * 3) as usize];
        Ok(engine)
    }

    fn make_pipeline(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
    ) -> Result<wgpu::ComputePipeline> {
        // Error scope instead of wgpu's default panic-on-validation-error: a broken
        // shader (live editing with hot-reload!) must surface as a UI error, not
        // kill the engine thread.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gate.wgsl"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gate"),
            bind_group_layouts: &[Some(bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gate"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(err) = pollster::block_on(scope.pop()) {
            anyhow::bail!("shader/pipeline validation failed: {err}");
        }
        Ok(pipeline)
    }

    /// Rebuild the pipeline from the (possibly edited) shader source. Dev feature.
    pub fn reload_shader(&mut self) -> Result<()> {
        self.pipeline = Self::make_pipeline(&self.device, &self.bind_group_layout)?;
        log::info!("shader reloaded");
        Ok(())
    }

    fn make_pixel_buffers(device: &wgpu::Device, npix: u32) -> (wgpu::Buffer, [wgpu::Buffer; 2]) {
        let size = (npix as u64) * 4;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("out"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = std::array::from_fn(|i| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(if i == 0 { "staging-0" } else { "staging-1" }),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        (out, staging)
    }

    #[allow(clippy::too_many_arguments)]
    fn make_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        audio: &wgpu::Buffer,
        layers: &wgpu::Buffer,
        effects: &wgpu::Buffer,
        out: &wgpu::Buffer,
        dabs: &wgpu::Buffer,
        scope: &wgpu::Buffer,
        video: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gate"),
            layout,
            entries: &[
                bind(0, globals),
                bind(1, audio),
                bind(2, layers),
                bind(3, effects),
                bind(4, out),
                bind(5, dabs),
                bind(6, scope),
                bind(7, video),
            ],
        })
    }

    /// Resize pixel buffers when the configured geometry changes.
    pub fn ensure_capacity(&mut self, npix: u32) {
        if npix == self.npix {
            return;
        }
        let (out, staging) = Self::make_pixel_buffers(&self.device, npix);
        self.out_buf = out;
        self.staging = staging;
        self.staging_submission = [None, None];
        self.npix = npix;
        self.frame = 0;
        self.readback = vec![0u8; (npix * 3) as usize];
        self.bind_group = Self::make_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.globals_buf,
            &self.audio_buf,
            &self.layers_buf,
            &self.effects_buf,
            &self.out_buf,
            &self.dabs_buf,
            &self.scope_buf,
            &self.video_buf,
        );
    }

    /// Submit compute for this frame, then read back the PREVIOUS frame (ping-pong).
    /// Returns the previous frame's RGB bytes, or None on the very first frame.
    pub fn render(&mut self, inputs: &FrameInputs) -> Result<Option<&[u8]>> {
        let write_slot = (self.frame % 2) as usize;
        let read_slot = ((self.frame + 1) % 2) as usize;

        self.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&inputs.globals));
        self.queue
            .write_buffer(&self.audio_buf, 0, bytemuck::cast_slice(&inputs.audio));
        if !inputs.layers.is_empty() {
            self.queue
                .write_buffer(&self.layers_buf, 0, bytemuck::cast_slice(&inputs.layers));
        }
        if !inputs.effects.is_empty() {
            self.queue
                .write_buffer(&self.effects_buf, 0, bytemuck::cast_slice(&inputs.effects));
        }
        if !inputs.dabs.is_empty() {
            self.queue
                .write_buffer(&self.dabs_buf, 0, bytemuck::cast_slice(&inputs.dabs));
        }
        self.queue
            .write_buffer(&self.scope_buf, 0, bytemuck::cast_slice(&inputs.scope));
        if let Some(rgba) = inputs.video_upload.as_ref()
            && !rgba.is_empty()
        {
            self.queue.write_buffer(&self.video_buf, 0, rgba);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gate"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.npix.div_ceil(256), 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.out_buf,
            0,
            &self.staging[write_slot],
            0,
            (self.npix as u64) * 4,
        );
        let submission = self.queue.submit([encoder.finish()]);
        self.staging_submission[write_slot] = Some(submission);
        self.frame += 1;

        // Read back the previous frame — its copy was submitted a full frame ago, so
        // the wait below is effectively free.
        let Some(prev_submission) = self.staging_submission[read_slot].take() else {
            return Ok(None);
        };
        let buf = &self.staging[read_slot];
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(prev_submission),
                timeout: None,
            })
            .context("device poll failed")?;
        rx.recv()
            .context("map_async callback dropped")?
            .context("staging buffer map failed")?;
        {
            let data = slice.get_mapped_range();
            let words: &[u32] = bytemuck::cast_slice(&data);
            for (px, w) in words.iter().enumerate() {
                let o = px * 3;
                self.readback[o] = (*w & 0xff) as u8;
                self.readback[o + 1] = ((*w >> 8) & 0xff) as u8;
                self.readback[o + 2] = ((*w >> 16) & 0xff) as u8;
            }
        }
        buf.unmap();
        Ok(Some(&self.readback))
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bind(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

// ---------------------------------------------------------------------------
// Autopilot: slow mean-reverting random walk over layer parameters
// ---------------------------------------------------------------------------

const WALK_PARAMS: usize = 8; // speed, scale, hue, hue_range, brightness, pa, pb, pc

#[derive(Clone, Default)]
struct LayerWalk {
    offsets: [f32; WALK_PARAMS],
}

struct WalkRng(u64);

impl WalkRng {
    fn new() -> Self {
        Self(0x853c_49e6_748f_ea9b ^ std::process::id() as u64)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Roughly N(0,1) (sum of uniforms), plenty for drift noise.
    fn gaussian(&mut self) -> f32 {
        let mut acc = 0.0f32;
        for _ in 0..3 {
            acc += (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        }
        (acc - 1.5) * 1.6
    }
}

/// Ornstein-Uhlenbeck step: offsets drift smoothly, mean-revert to 0 (i.e. to the
/// slider positions), with stationary std ≈ 1. `tau` is the evolution time scale.
fn walk_step(walk: &mut LayerWalk, rng: &mut WalkRng, dt: f32, tau: f32) {
    let k = (dt / tau).min(0.5);
    for o in walk.offsets.iter_mut() {
        *o += -*o * k + rng.gaussian() * (2.0 * k).sqrt();
        *o = o.clamp(-2.5, 2.5);
    }
}

/// Wander around the authored speed without cancelling it or flipping direction.
/// Slow ambient layers previously used an additive offset large enough to stall;
/// exponential scaling keeps the same sign and a useful fraction of base motion.
fn walked_speed(base: f32, offset: f32, amount: f32) -> f32 {
    if base == 0.0 {
        return 0.0;
    }
    let exponent = (offset * 0.45 * amount).clamp(-1.0, 1.0);
    base * exponent.exp()
}

/// Apply a layer's walk offsets around its configured values. The user's slider
/// value is the center; `walk_amount` scales the wander radius per parameter.
fn walked_layer(l: &crate::layers::LayerCfg, w: &LayerWalk) -> crate::layers::LayerCfg {
    let a = l.walk_amount;
    let mut out = l.clone();
    out.speed = walked_speed(l.speed, w.offsets[0], a);
    out.scale = (l.scale * (1.0 + w.offsets[1] * 0.4 * a)).clamp(0.05, 6.0);
    out.hue = l.hue + w.offsets[2] * 0.12 * a; // hue wraps in the shader
    out.hue_range = (l.hue_range + w.offsets[3] * 0.08 * a).clamp(0.0, 1.0);
    out.brightness = (l.brightness * (1.0 + w.offsets[4] * 0.25 * a)).clamp(0.0, 2.0);
    out.param_a = (l.param_a + w.offsets[5] * 0.2 * a).clamp(0.0, 1.0);
    out.param_b = (l.param_b + w.offsets[6] * 0.2 * a).clamp(0.0, 1.0);
    out.param_c = (l.param_c + w.offsets[7] * 0.2 * a).clamp(0.0, 1.0);
    out
}

// ---------------------------------------------------------------------------
// Frame loop
// ---------------------------------------------------------------------------

pub fn spawn(state: Arc<SharedState>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || engine_thread(state))
        .expect("spawn engine thread")
}

/// Windows coarsens the sleep granularity to ~15.6 ms whenever no foreground app
/// requests better — alt-tabbing away from our window visibly dropped the real
/// frame rate. Pin the system timer to 1 ms for the life of the process so the
/// frame loop paces identically focused, background, and headless.
#[cfg(windows)]
fn raise_timer_resolution() {
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
    }
    unsafe {
        timeBeginPeriod(1);
    }
}

#[cfg(not(windows))]
fn raise_timer_resolution() {}

fn engine_thread(state: Arc<SharedState>) {
    raise_timer_resolution();
    while !state.shutdown.load(Ordering::Relaxed) {
        let npix = state.config.read().geometry.pixel_count() as u32;
        // catch_unwind: wgpu panics (rather than erroring) on some init failures,
        // e.g. no backend implemented for the platform. A panic here must surface
        // as a clear error to the UI, not silently kill the engine thread.
        let engine = std::panic::catch_unwind(|| Engine::new(npix)).unwrap_or_else(|p| {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".into());
            Err(anyhow::anyhow!("GPU init panicked: {msg}"))
        });
        match engine {
            Ok(mut engine) => {
                state.status.lock().gpu_error = None;
                state.status.lock().gpu_name = engine.gpu_name.clone();
                run_frames(&state, &mut engine);
            }
            Err(e) => {
                let msg = format!("{e:#}");
                log::error!("engine init failed: {msg}");
                state.status.lock().gpu_error = Some(msg.clone());
                let _ = state.events.send(ServerMsg::Error { message: msg });
                // Clear error + retry periodically; a driver install may fix it live.
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
    // Backstop for the paths that leave `run_frames` early (GPU re-init, init
    // failure): nothing more will be sent, so exit must not wait on us.
    state.sacn_terminated.store(true, Ordering::SeqCst);
}

#[cfg(feature = "shader-hot-reload")]
fn spawn_shader_watcher(flag: Arc<std::sync::atomic::AtomicBool>) -> Option<impl Sized> {
    use notify::Watcher;
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/engine/shaders"));
    if !dir.exists() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            flag.store(true, Ordering::Relaxed);
        }
    })
    .ok()?;
    watcher.watch(dir, notify::RecursiveMode::NonRecursive).ok()?;
    log::info!("shader hot-reload watching {}", dir.display());
    Some(watcher)
}

/// Convert the detector/manual base beat to the musical clock consumed by lights.
/// The beat count supplies the missing parity needed for a true half-time phase.
fn effective_beat_phase(
    base_phase: f32,
    base_beat_count: u64,
    beat_time: crate::config::BeatTime,
) -> f32 {
    match beat_time {
        crate::config::BeatTime::Half => (base_beat_count % 2) as f32 * 0.5 + base_phase * 0.5,
        crate::config::BeatTime::Normal => base_phase,
        crate::config::BeatTime::Double => (base_phase * 2.0).fract(),
    }
}

fn stack_from_config(cfg: &crate::config::AppConfig) -> crate::config::SavedStack {
    crate::config::SavedStack {
        id: "live-stack".into(),
        name: "Current look".into(),
        layers: cfg.layers.clone(),
        master_speed: cfg.render.master_speed,
        walk_enabled: cfg.render.walk_enabled,
        walk_layers: cfg.render.walk_layers,
        walk_min_layers: cfg.render.walk_min_layers,
        walk_speed: cfg.render.walk_speed,
        walk_depth: cfg.render.walk_depth,
    }
}

fn apply_stack_to_config(cfg: &mut crate::config::AppConfig, stack: &crate::config::SavedStack) {
    cfg.layers = stack.layers.clone();
    cfg.render.master_speed = stack.master_speed;
    cfg.render.walk_enabled = stack.walk_enabled;
    cfg.render.walk_layers = stack.walk_layers;
    cfg.render.walk_min_layers = stack.walk_min_layers;
    cfg.render.walk_speed = stack.walk_speed;
    cfg.render.walk_depth = stack.walk_depth;
}

/// Keep scene blend math intact by arranging complete outgoing and incoming stacks.
/// The shader composes each side independently and mixes only their finished colors.
fn transition_layers(
    outgoing: Option<&crate::config::SavedStack>,
    incoming: &crate::config::SavedStack,
) -> (Vec<crate::layers::LayerCfg>, usize) {
    let mut layers = Vec::with_capacity(MAX_RENDER_LAYERS);
    let split = outgoing.map_or(0, |stack| {
        layers.extend(stack.layers.iter().take(MAX_LAYERS).cloned());
        stack.layers.len().min(MAX_LAYERS)
    });
    layers.extend(incoming.layers.iter().take(MAX_LAYERS).cloned());
    (layers, split)
}

#[cfg(test)]
mod beat_time_tests {
    use super::{
        apply_stack_to_config, effective_beat_phase, stack_from_config, transition_layers,
        walked_speed,
    };
    use crate::config::{AppConfig, BeatTime, SavedStack};
    use crate::layers::MAX_LAYERS;

    #[test]
    fn half_time_spans_two_base_beats() {
        assert_eq!(effective_beat_phase(0.25, 0, BeatTime::Half), 0.125);
        assert_eq!(effective_beat_phase(0.25, 1, BeatTime::Half), 0.625);
        assert_eq!(effective_beat_phase(0.25, 2, BeatTime::Half), 0.125);
    }

    #[test]
    fn double_time_wraps_halfway_through_a_base_beat() {
        assert_eq!(effective_beat_phase(0.25, 0, BeatTime::Double), 0.5);
        assert_eq!(effective_beat_phase(0.75, 0, BeatTime::Double), 0.5);
    }

    #[test]
    fn transition_keeps_each_scenes_layer_opacity_intact() {
        let mut outgoing = SavedStack::default();
        outgoing.layers = AppConfig::default().layers;
        outgoing.layers[0].opacity = 0.73;
        let mut incoming = outgoing.clone();
        incoming.layers[0].opacity = 0.41;

        let (layers, split) = transition_layers(Some(&outgoing), &incoming);

        assert_eq!(split, outgoing.layers.len());
        assert_eq!(layers[0].opacity, 0.73);
        assert_eq!(layers[split].opacity, 0.41);
    }

    #[test]
    fn transition_has_capacity_for_two_complete_maximum_scenes() {
        let layer = AppConfig::default().layers[0].clone();
        let outgoing = SavedStack {
            layers: vec![layer.clone(); MAX_LAYERS],
            ..Default::default()
        };
        let incoming = SavedStack {
            layers: vec![layer; MAX_LAYERS],
            ..Default::default()
        };

        let (layers, split) = transition_layers(Some(&outgoing), &incoming);

        assert_eq!(split, MAX_LAYERS);
        assert_eq!(layers.len(), MAX_LAYERS * 2);
    }

    #[test]
    fn scheduled_stack_round_trips_through_live_config() {
        let mut cfg = AppConfig::default();
        let mut stack = stack_from_config(&cfg);
        stack.master_speed = 0.42;
        stack.walk_enabled = true;
        stack.walk_depth = 2.25;
        stack.layers[0].opacity = 0.37;

        apply_stack_to_config(&mut cfg, &stack);
        let mirrored = stack_from_config(&cfg);

        assert_eq!(mirrored.master_speed, stack.master_speed);
        assert_eq!(mirrored.walk_enabled, stack.walk_enabled);
        assert_eq!(mirrored.walk_depth, stack.walk_depth);
        assert_eq!(mirrored.layers[0].opacity, stack.layers[0].opacity);
    }

    #[test]
    fn speed_walk_preserves_motion_and_direction() {
        let slow_forward = walked_speed(0.03, -2.5, 3.0);
        let slow_reverse = walked_speed(-0.03, -2.5, 3.0);

        assert!(slow_forward >= 0.03 / std::f32::consts::E);
        assert!(slow_reverse <= -0.03 / std::f32::consts::E);
        assert_eq!(walked_speed(0.0, 2.5, 3.0), 0.0);
    }
}

fn run_frames(state: &Arc<SharedState>, engine: &mut Engine) {
    let mut sacn = match SacnSender::new() {
        Ok(s) => Some(s),
        Err(e) => {
            log::error!("sACN socket unavailable: {e}");
            state.status.lock().sacn_error = Some(format!("sACN socket unavailable: {e}"));
            None
        }
    };

    let mut epoch = u32::MAX;
    let mut output_cfg_key = String::new();
    // Phases live in SharedState so a handover can transplant them; the engine
    // loop is their only writer.
    let mut layer_phases: Vec<f64> = state.layer_phases.lock().clone();
    let mut layer_walks: Vec<LayerWalk> = Vec::new();
    let mut walk_rng = WalkRng::new();
    let mut last_frame = Instant::now();
    // Accumulator schedule: `next_sacn += interval` (not `= now`) so the sACN rate
    // never beats against the render tick — that aliasing showed up in sACNView as
    // an unsteady frame rate.
    let mut next_sacn = Instant::now();
    // Edge-detects output going off, which is when the stream gets terminated.
    let mut was_sending = false;
    let mut last_status = Instant::now();
    let mut fps_ema = 0.0f32;
    let mut frame_ms_ema = 0.0f32;
    let mut frame_number: u64 = 0;
    let mut video_revision: u64 = u64::MAX;
    let mut next_sacn_retry = Instant::now();

    // The show clock belongs to the backend, not a browser. `current_stack` and
    // `transition_from` are render-only snapshots; the durable selection/index
    // lives in AppConfig and is advanced below.
    let mut show_key = String::new();
    let mut scene_started = Instant::now();
    let mut current_stack: Option<crate::config::SavedStack> = None;
    let mut transition_from: Option<crate::config::SavedStack> = None;
    let mut advance_requested = false;

    // Per-second buckets for the UI history bars (frames rendered / packets sent).
    let mut sec_start = Instant::now();
    let mut frames_this_sec: u32 = 0;
    let mut pkts_this_sec: u32 = 0;
    let mut fps_hist: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    let mut pps_hist: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    const HIST_LEN: usize = 30;

    // Gray-code layer walk: at most one layer's on/off state changes per step, and
    // never fewer than `walk_min_layers` stay on. Fades via an opacity envelope so
    // entrances/exits are gentle.
    let mut layer_target: Vec<bool> = Vec::new();
    let mut layer_env: Vec<f32> = Vec::new();
    let mut next_flip = Instant::now();

    // Beat taps and the operator beat pulse follow the lighting-time clock, which
    // can run at half/normal/double the detector's inferred tempo.
    let mut prev_raw_beat_phase = [0.0f32; MAX_AUDIO_SOURCES];
    let mut raw_beat_count = [0u64; MAX_AUDIO_SOURCES];
    let mut manual_beat_phase = 0.0f32;
    let mut manual_beat_count = 0u64;
    let mut prev_beat_phase = [0.0f32; MAX_AUDIO_SOURCES];
    let mut last_beat_time = None;
    let mut last_timing_signature: Option<(u8, u32)> = None;
    let mut tap_angle: f32 = 0.0;
    let mut tap_beat_count: u64 = 0;
    let mut tap_spin_walk = LayerWalk::default();

    #[cfg(feature = "shader-hot-reload")]
    let shader_dirty = Arc::new(std::sync::atomic::AtomicBool::new(false));
    #[cfg(feature = "shader-hot-reload")]
    let _watcher = spawn_shader_watcher(shader_dirty.clone());

    while !state.shutdown.load(Ordering::Relaxed) {
        #[cfg(feature = "shader-hot-reload")]
        if shader_dirty.swap(false, Ordering::Relaxed)
            && let Err(e) = engine.reload_shader()
        {
            let msg = format!("shader reload failed: {e:#}");
            log::error!("{msg}");
            let _ = state.events.send(ServerMsg::Error { message: msg });
        }

        // Reconfigure buffers + the sACN plan only when geometry/output change —
        // NOT on every config epoch bump (sliders would reset sequence numbers).
        let cfg = state.config.read().clone();
        let current_epoch = state.epoch();
        if current_epoch != epoch {
            epoch = current_epoch;
            let key = serde_json::to_string(&(&cfg.geometry, &cfg.output)).unwrap_or_default();
            if key != output_cfg_key {
                output_cfg_key = key;
                engine.ensure_capacity(cfg.geometry.pixel_count() as u32);
                if let Some(s) = sacn.as_mut() {
                    s.configure(&cfg.geometry, &cfg.output);
                    let mut st = state.status.lock();
                    st.sacn_universes = s.universe_count();
                    st.sacn_error = s.error_message();
                }
            }
        }
        // UDP socket creation and interface binding can fail transiently during
        // boot, dock/NIC changes, or network profile transitions. Recover without
        // requiring a config edit or GPU restart.
        let retry_needed = sacn
            .as_ref()
            .map(|sender| sender.bind_error.is_some())
            .unwrap_or(true);
        if retry_needed && Instant::now() >= next_sacn_retry {
            next_sacn_retry = Instant::now() + Duration::from_secs(2);
            match sacn.as_mut() {
                Some(sender) => sender.configure(&cfg.geometry, &cfg.output),
                None => match SacnSender::new() {
                    Ok(mut sender) => {
                        sender.configure(&cfg.geometry, &cfg.output);
                        sacn = Some(sender);
                    }
                    Err(e) => {
                        state.status.lock().sacn_error =
                            Some(format!("sACN socket unavailable: {e}; retrying"));
                    }
                },
            }
            if let Some(sender) = sacn.as_ref() {
                let mut st = state.status.lock();
                st.sacn_universes = sender.universe_count();
                st.sacn_error = sender.error_message();
            }
        }
        if state.phases_transplanted.swap(false, Ordering::SeqCst) {
            layer_phases = state.layer_phases.lock().clone();
        }
        // Pace the loop.
        let target_dt = Duration::from_secs_f32(1.0 / cfg.render.fps.clamp(1.0, 240.0));
        let elapsed = last_frame.elapsed();
        if elapsed < target_dt {
            let remaining = target_dt - elapsed;
            if remaining > Duration::from_millis(2) {
                std::thread::sleep(remaining - Duration::from_millis(2));
            }
            while last_frame.elapsed() < target_dt {
                std::hint::spin_loop();
            }
        }
        let now = Instant::now();
        let dt = (now - last_frame).as_secs_f32();
        last_frame = now;

        // Resolve the active timed show and build a temporary layer stack. During
        // a transition the shader composes both scenes independently, then mixes
        // their completed colors. After the fade, incoming animation phases are
        // shifted down so finishing a transition never causes a motion jump.
        let scheduled = if cfg.show_scheduler.enabled {
            cfg.saved_playlists
                .iter()
                .find(|p| p.id == cfg.show_scheduler.active_playlist_id && !p.entries.is_empty())
                .map(|p| {
                    let index = (cfg.show_scheduler.current_index as usize).min(p.entries.len() - 1);
                    (p, index, &p.entries[index])
                })
        } else {
            None
        };
        let mut show_status = ScheduledShowStatus::default();
        let mut render_layers = cfg.layers.clone();
        let mut render_master_speed = cfg.render.master_speed;
        let mut render_walk_enabled = cfg.render.walk_enabled;
        let mut render_walk_layers = cfg.render.walk_layers;
        let mut render_walk_min_layers = cfg.render.walk_min_layers;
        let mut render_walk_speed = cfg.render.walk_speed;
        let mut render_walk_depth = cfg.render.walk_depth;
        let mut render_transition_split = 0usize;
        let mut render_transition_active = false;
        let mut render_transition_progress = 0.0f32;

        if let Some((playlist, index, entry)) = scheduled {
            // Timing edits should take effect without restarting the current cue;
            // identity/index changes are the actual transition boundary.
            let key = format!("{}:{index}:{}", playlist.id, entry.id);
            let scene_changed = key != show_key;
            if scene_changed {
                let previous = current_stack.take().unwrap_or_else(|| stack_from_config(&cfg));
                transition_from = Some(previous);
                current_stack = Some(entry.stack.clone());
                show_key = key;
                scene_started = now;
                advance_requested = false;

                // Make the scheduled target the actual live config too. Controllers
                // now show (and edit) the scene that the renderer is using instead
                // of a stale stack left over from before the journey started.
                let target = entry.stack.clone();
                state.update_config(move |c| apply_stack_to_config(c, &target));
            } else {
                // Config edits made while a cue is running are edits to that cue's
                // effective live stack. The embedded playlist snapshot stays intact.
                current_stack = Some(stack_from_config(&cfg));
            }

            let target = current_stack.as_ref().expect("scheduled stack");
            render_master_speed = target.master_speed;
            render_walk_enabled = target.walk_enabled;
            render_walk_layers = target.walk_layers;
            render_walk_min_layers = target.walk_min_layers;
            render_walk_speed = target.walk_speed;
            render_walk_depth = target.walk_depth;

            let elapsed = scene_started.elapsed().as_secs_f32();
            let transition_secs = entry.transition_secs.clamp(0.0, 300.0);
            let linear = if transition_secs <= 0.001 {
                1.0
            } else {
                (elapsed / transition_secs).clamp(0.0, 1.0)
            };
            let fade = linear * linear * (3.0 - 2.0 * linear);

            if linear >= 1.0 && transition_from.is_some() {
                let old_len = transition_from.as_ref().map_or(0, |s| s.layers.len().min(MAX_LAYERS));
                for i in 0..target.layers.len().min(MAX_LAYERS) {
                    if old_len + i < layer_phases.len() {
                        layer_phases[i] = layer_phases[old_len + i];
                        layer_walks[i] = layer_walks[old_len + i].clone();
                        layer_target[i] = layer_target[old_len + i];
                        layer_env[i] = layer_env[old_len + i];
                    }
                }
                transition_from = None;
            }

            (render_layers, render_transition_split) =
                transition_layers(transition_from.as_ref(), target);
            render_transition_active = transition_from.is_some();
            render_transition_progress = fade;

            let duration = entry.duration_secs.clamp(10.0, 86_400.0);
            show_status = ScheduledShowStatus {
                enabled: true,
                playlist_id: playlist.id.clone(),
                playlist_name: playlist.name.clone(),
                scene_name: entry.name.clone(),
                index: index as u32,
                total: playlist.entries.len() as u32,
                remaining_secs: (duration - elapsed).max(0.0),
                transition_progress: if linear < 1.0 { fade } else { 0.0 },
            };

            if elapsed >= duration && !advance_requested {
                advance_requested = true;
                let playlist_id = playlist.id.clone();
                let is_last = index + 1 >= playlist.entries.len();
                let repeat = playlist.repeat;
                let hold = target.clone();
                state.update_config(move |c| {
                    if is_last && !repeat {
                        c.layers = hold.layers;
                        c.render.master_speed = hold.master_speed;
                        c.render.walk_enabled = hold.walk_enabled;
                        c.render.walk_layers = hold.walk_layers;
                        c.render.walk_min_layers = hold.walk_min_layers;
                        c.render.walk_speed = hold.walk_speed;
                        c.render.walk_depth = hold.walk_depth;
                        c.show_scheduler.enabled = false;
                    } else if c.show_scheduler.active_playlist_id == playlist_id {
                        c.show_scheduler.current_index = if is_last { 0 } else { index as u32 + 1 };
                    }
                });
            }
        } else {
            show_key.clear();
            current_stack = None;
            transition_from = None;
            advance_requested = false;
        }

        layer_phases.resize(render_layers.len(), 0.0);
        layer_walks.resize(render_layers.len(), LayerWalk::default());
        layer_target.resize(render_layers.len(), true);
        layer_env.resize(render_layers.len(), 1.0);

        if let Some(bpm) = cfg.render.manual_bpm {
            let next = manual_beat_phase + dt * bpm.clamp(10.0, 400.0) / 60.0;
            manual_beat_count = manual_beat_count.wrapping_add(next.floor() as u64);
            manual_beat_phase = next.fract();
        }

        // Gather audio energy first. Timing is selected separately below so an
        // authoritative external clock can drive every layer without replacing
        // any layer's level/bands/waveform source.
        let audio_inputs: [crate::state::AudioFeatures; MAX_AUDIO_SOURCES] =
            std::array::from_fn(|i| *state.audio[i].lock());
        for (i, a) in audio_inputs.iter().enumerate() {
            if a.bpm <= 0.0 {
                raw_beat_count[i] = 0;
            } else if a.beat_phase < prev_raw_beat_phase[i] - 0.5 {
                raw_beat_count[i] = raw_beat_count[i].wrapping_add(1);
            }
            prev_raw_beat_phase[i] = a.beat_phase;
        }
        let clock_now = Instant::now();
        let clock_latency = cfg.rhythm.latency_ms.clamp(-500.0, 500.0);
        let midi_clock = state.midi_clock.lock().snapshot(clock_now, clock_latency);
        let (pioneer_clock, pioneer_label, pioneer_error, pioneer_devices) = {
            let link = state.pioneer_clock.lock();
            (
                link.snapshot(clock_now, clock_latency),
                link.player_label(),
                link.listen_error().to_owned(),
                link.devices(clock_now),
            )
        };
        let midi_selected = cfg.rhythm.source == crate::config::RhythmSource::MidiClock;
        let pioneer_selected = cfg.rhythm.source == crate::config::RhythmSource::ProDjLink;
        let external_selected = midi_selected || pioneer_selected;
        let external_clock = if pioneer_selected {
            pioneer_clock
        } else {
            midi_clock
        };
        let fallback_index =
            (cfg.rhythm.fallback_audio_source as usize).min(MAX_AUDIO_SOURCES.saturating_sub(1));

        let mut audio = [AudioUniform::default(); MAX_AUDIO_SOURCES];
        for (i, a) in audio_inputs.iter().enumerate() {
            let (base_phase, base_count, base_bpm) = match cfg.render.manual_bpm {
                Some(bpm) => (manual_beat_phase, manual_beat_count, bpm.clamp(10.0, 400.0)),
                None if external_selected && external_clock.usable => (
                    external_clock.beat_phase,
                    external_clock.beat_count,
                    external_clock.bpm,
                ),
                None if external_selected && cfg.rhythm.fallback_to_audio => {
                    let fallback = &audio_inputs[fallback_index];
                    (
                        fallback.beat_phase,
                        raw_beat_count[fallback_index],
                        fallback.bpm,
                    )
                }
                None if external_selected => (0.0, 0, 0.0),
                None => (a.beat_phase, raw_beat_count[i], a.bpm),
            };
            audio[i] = AudioUniform {
                level: a.level,
                bass: a.bass,
                mid: a.mid,
                treble: a.treble,
                onset: a.onset,
                beat_phase: effective_beat_phase(base_phase, base_count, cfg.render.beat_time),
                bpm: base_bpm * cfg.render.beat_time.multiplier(),
                // Manually-set tempo is definitionally trusted.
                bpm_conf: if cfg.render.manual_bpm.is_some() { 1.0 } else { a.bpm_conf },
                bass_att: a.bass_att,
                mid_att: a.mid_att,
                treble_att: a.treble_att,
                _pad2: 0.0,
            };
        }

        // Waveform + spectrum arrays for the GPU (see Waveform/Spectrum layers).
        let mut scope_data = vec![0.0f32; SCOPE_FLOATS * MAX_AUDIO_SOURCES];
        for (i, s) in state.scope.iter().enumerate() {
            let s = s.lock();
            let base = i * SCOPE_FLOATS;
            scope_data[base..base + 256].copy_from_slice(&s.wave);
            scope_data[base + 256..base + SCOPE_FLOATS].copy_from_slice(&s.spectrum);
        }
        let control = *state.control.lock();
        let walk_tau = 45.0 / render_walk_speed.clamp(0.05, 20.0);

        // Beat taps: on each detected beat (phase wrap) of the chosen source, fire
        // a burst at a point orbiting the ring — the automated spiral-tap.
        let bt = &cfg.beat_taps;
        let timing_signature = if cfg.render.manual_bpm.is_some() {
            (1, 0)
        } else if external_selected && external_clock.usable {
            (2, 0)
        } else if external_selected && cfg.rhythm.fallback_to_audio {
            (3, fallback_index as u32)
        } else if external_selected {
            (4, 0)
        } else {
            (0, 0)
        };
        let beat_time_changed = last_beat_time != Some(cfg.render.beat_time)
            || last_timing_signature != Some(timing_signature);
        last_beat_time = Some(cfg.render.beat_time);
        last_timing_signature = Some(timing_signature);
        for (i, a) in audio.iter().enumerate() {
            let wrapped = !beat_time_changed && a.beat_phase < prev_beat_phase[i] - 0.5;
            prev_beat_phase[i] = a.beat_phase;
            // Unconfident beats are noise: no beat events (UI pulse), no beat taps.
            let confident = a.bpm_conf >= 0.35;
            if wrapped && a.bpm > 0.0 && confident && i < cfg.audio.sources.len() {
                let _ = state.events.send(crate::protocol::ServerMsg::Beat {
                    source: i as u32,
                    bpm: a.bpm,
                });
            }
            if !bt.enabled || i != bt.audio_source as usize || !wrapped || a.bpm <= 0.0 || !confident
            {
                continue;
            }
            tap_beat_count += 1;
            // Optional slow drift of the spin rate (reuses the OU walk noise), so
            // the spiral tightens and loosens over minutes.
            let spin = if bt.vary {
                walk_step(&mut tap_spin_walk, &mut walk_rng, 60.0 / a.bpm, walk_tau);
                bt.spin * (tap_spin_walk.offsets[0] * 0.6).exp()
            } else {
                bt.spin
            };
            tap_angle = (tap_angle + spin * std::f32::consts::TAU).rem_euclid(std::f32::consts::TAU);
            if tap_beat_count.is_multiple_of(bt.every.max(1) as u64) {
                state.trigger_effect(crate::layers::EffectCfg {
                    kind: crate::layers::EffectKind::Burst,
                    angle: tap_angle,
                    radius: bt.radius.clamp(0.0, 1.0),
                    intensity: bt.intensity.clamp(0.0, 2.0),
                    size: 1.0,
                    hue: bt.hue,
                    duration: 0.0,
                    ..Default::default()
                });
            }
        }

        // Autopilot: evolve walk offsets (~minutes time scale) and render the
        // walked parameter values; the config itself is never modified.

        // Gray-code walk across which layers play: flip exactly one layer per step
        // among the user-enabled pool, keeping at least `walk_min_layers` on.
        let walk_layers_on = render_walk_enabled && render_walk_layers;
        let render_layer_limit = if render_transition_active {
            MAX_RENDER_LAYERS
        } else {
            MAX_LAYERS
        };
        if walk_layers_on && now >= next_flip {
            next_flip = now + Duration::from_secs_f32(walk_tau);
            let eligible: Vec<usize> = render_layers
                .iter()
                .enumerate()
                .take(render_layer_limit)
                .filter(|(_, l)| l.enabled)
                .map(|(i, _)| i)
                .collect();
            let min_on = (render_walk_min_layers as usize).min(eligible.len());
            let on_count = eligible.iter().filter(|i| layer_target[**i]).count();
            if !eligible.is_empty() {
                for _ in 0..8 {
                    let pick = eligible[(walk_rng.next_u64() % eligible.len() as u64) as usize];
                    if layer_target[pick] {
                        if on_count > min_on {
                            layer_target[pick] = false;
                            break;
                        }
                    } else {
                        layer_target[pick] = true;
                        break;
                    }
                }
            }
        }

        let mut layers = Vec::with_capacity(render_layers.len().min(render_layer_limit));
        let mut gpu_transition_split = 0u32;
        for (i, l) in render_layers.iter().take(render_layer_limit).enumerate() {
            // Envelope eases layers in/out of the mix over a few seconds.
            let target = if walk_layers_on { layer_target[i] } else { true };
            let goal = if target { 1.0 } else { 0.0 };
            layer_env[i] += (goal - layer_env[i]) * (dt / 4.0).min(1.0);
            if !l.enabled {
                continue;
            }
            if layer_env[i] < 0.005 {
                // Fully faded out by the walk — keep its phase moving, skip the GPU.
                layer_phases[i] += (l.speed * render_master_speed * dt) as f64;
                continue;
            }
            let mut l = if render_walk_enabled && l.walk_amount > 0.0 {
                walk_step(&mut layer_walks[i], &mut walk_rng, dt, walk_tau);
                // Global depth scales how far every layer wanders from its sliders.
                let mut scaled = l.clone();
                scaled.walk_amount = (l.walk_amount * render_walk_depth).clamp(0.0, 3.0);
                walked_layer(&scaled, &layer_walks[i])
            } else {
                l.clone()
            };
            l.opacity *= layer_env[i];
            layer_phases[i] += (l.speed * render_master_speed * dt) as f64;
            if render_transition_active && i < render_transition_split {
                gpu_transition_split += 1;
            }
            layers.push(l.to_gpu(layer_phases[i] as f32));
        }
        *state.layer_phases.lock() = layer_phases.clone();

        let effects: Vec<GpuEffect> = {
            let mut fx = state.effects.lock();
            fx.retain(|e| {
                let dur = if e.cfg.duration > 0.0 {
                    e.cfg.duration
                } else {
                    e.cfg.kind.default_duration()
                };
                e.born.elapsed().as_secs_f32() < dur
            });
            fx.iter()
                .map(|e| GpuEffect {
                    kind: e.cfg.kind.gpu_id(),
                    size: e.cfg.size.clamp(0.1, 4.0),
                    age: e.born.elapsed().as_secs_f32(),
                    duration: if e.cfg.duration > 0.0 {
                        e.cfg.duration
                    } else {
                        e.cfg.kind.default_duration()
                    },
                    angle: e.cfg.angle,
                    radius: e.cfg.radius,
                    intensity: e.cfg.intensity,
                    hue: e.cfg.hue,
                    saturation: e.cfg.saturation.clamp(0.0, 1.0),
                    brightness: e.cfg.brightness.clamp(0.0, 1.0),
                    _pad: [0.0; 2],
                })
                .collect()
        };

        let dabs: Vec<GpuDab> = {
            let mut dabs = state.dabs.lock();
            dabs.retain(|d| d.born.elapsed().as_secs_f32() < d.kind.lifetime());
            dabs.iter()
                .take(MAX_DABS)
                .map(|d| GpuDab {
                    kind: d.kind.gpu_id(),
                    age: d.born.elapsed().as_secs_f32() / d.kind.lifetime(),
                    angle: d.angle,
                    radius: d.radius,
                    hue: d.hue,
                    size: d.size,
                    intensity: d.intensity,
                    dir: d.dir,
                    saturation: d.saturation,
                    brightness: d.brightness,
                    _pad: [0.0; 2],
                })
                .collect()
        };

        let (video_width, video_height, video_active, video_upload) = {
            let v = state.video.lock();
            let upload = if v.revision != video_revision {
                video_revision = v.revision;
                Some(v.rgba.clone())
            } else {
                None
            };
            (
                v.width as u32,
                v.height as u32,
                v.active && v.width > 0 && v.height > 0,
                upload,
            )
        };

        let inputs = FrameInputs {
            globals: Globals {
                spokes: cfg.geometry.spokes,
                pixels: cfg.geometry.pixels_per_spoke,
                layer_count: layers.len() as u32,
                effect_count: effects.len() as u32,
                time: state.started.elapsed().as_secs_f32(),
                dt,
                master: cfg.render.master_brightness,
                inner_over_outer: (cfg.geometry.inner_radius_ft
                    / cfg.geometry.outer_radius_ft.max(0.001))
                .clamp(0.0, 1.0),
                tilt_x: control.roll,
                tilt_y: control.pitch,
                shake: control.shake,
                yaw: control.yaw,
                dab_count: dabs.len() as u32,
                video_width,
                video_height,
                video_active: u32::from(video_active),
                transition_split: gpu_transition_split,
                transition_active: u32::from(render_transition_active),
                transition_progress: render_transition_progress,
                _pad_transition: 0.0,
            },
            audio,
            layers,
            effects,
            dabs,
            scope: scope_data,
            video_upload,
        };

        let t0 = Instant::now();
        let rgb = match engine.render(&inputs) {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("render failed: {e:#}");
                log::error!("{msg}");
                let _ = state.events.send(ServerMsg::Error { message: msg });
                return; // drop back to engine_thread for re-init
            }
        };
        let frame_ms = t0.elapsed().as_secs_f32() * 1000.0;

        if let Some(rgb) = rgb {
            frame_number += 1;
            frames_this_sec += 1;

            state.frames_rendered.fetch_add(1, Ordering::Relaxed);

            // sACN first: the wire is the primary output. Held while a takeover
            // is pending (we must not send before the old instance stops) and
            // stopped for good once we've granted a handover to a successor.
            let leaving = state.leaving.load(Ordering::SeqCst);
            if leaving {
                // Ack: the commit reply to our successor waits on this — after it,
                // no more packets ever leave this instance.
                state.sacn_quiesced.store(true, Ordering::SeqCst);
            }
            let sacn_allowed = !state.sacn_hold.load(Ordering::Relaxed) && !leaving;
            let sending = cfg.output.enabled && sacn_allowed;
            if !sending && was_sending && !leaving {
                // Output was just switched off: close the stream with E1.31
                // termination packets so receivers release the universes now,
                // rather than holding the last frame through their 2.5 s
                // source-loss timeout. Never on `leaving` — the successor
                // instance carries the same CID's stream onward.
                if let Some(s) = sacn.as_mut() {
                    s.send_terminate();
                }
            }
            was_sending = sending;
            if sending
                && let Some(s) = sacn.as_mut()
            {
                let cap = cfg.output.fps.clamp(1.0, 120.0);
                let send = if cfg.output.sync_to_render && cap >= cfg.render.fps {
                    // Cap doesn't bind: one sACN frame per rendered frame, always.
                    true
                } else if now >= next_sacn {
                    // Accumulator schedule: steady cap rate, no aliasing against
                    // the render tick.
                    let interval = Duration::from_secs_f32(1.0 / cap);
                    next_sacn = (next_sacn + interval).max(now - interval);
                    true
                } else {
                    false
                };
                if send {
                    pkts_this_sec += s.send_frame(rgb) as u32;
                }
            }

            // Preview fan-out (subscribers decimate/throttle for themselves).
            if state.preview.receiver_count() > 0 {
                let _ = state.preview.send(Arc::new(PreviewFrame {
                    frame_number,
                    spokes: cfg.geometry.spokes,
                    pixels_per_spoke: cfg.geometry.pixels_per_spoke,
                    rgb: rgb.to_vec(),
                }));
            }
        }

        // Close out per-second history buckets.
        if sec_start.elapsed() >= Duration::from_secs(1) {
            sec_start += Duration::from_secs(1);
            fps_hist.push_back(frames_this_sec);
            pps_hist.push_back(pkts_this_sec);
            frames_this_sec = 0;
            pkts_this_sec = 0;
            while fps_hist.len() > HIST_LEN {
                fps_hist.pop_front();
            }
            while pps_hist.len() > HIST_LEN {
                pps_hist.pop_front();
            }
        }

        // Status at ~2 Hz.
        fps_ema = fps_ema * 0.9 + (1.0 / dt.max(1e-6)) * 0.1;
        frame_ms_ema = frame_ms_ema * 0.9 + frame_ms * 0.1;
        if last_status.elapsed() > Duration::from_millis(500) {
            last_status = Instant::now();
            let status = {
                let mut st = state.status.lock();
                st.engine_fps = fps_ema;
                st.frame_time_ms = frame_ms_ema;
                st.sacn_enabled = cfg.output.enabled;
                // Last full-second bucket: steady, unlike a fractional-window ratio.
                st.sacn_pps = pps_hist.back().copied().unwrap_or(0);
                if let Some(sender) = sacn.as_ref() {
                    st.sacn_error = sender.error_message();
                }
                st.fps_history = fps_hist.iter().copied().collect();
                st.pps_history = pps_hist.iter().copied().collect();
                st.master_brightness = cfg.render.master_brightness;
                st.master_speed = render_master_speed;
                st.show = show_status.clone();
                st.pro_dj_link_devices = pioneer_devices
                    .iter()
                    .map(|device| crate::protocol::ProDjLinkDeviceInfo {
                        number: device.number,
                        name: device.name.clone(),
                    })
                    .collect();
                st.video = {
                    let v = state.video.lock();
                    let owner_name = cfg
                        .clients
                        .iter()
                        .find(|c| c.id == v.owner_id)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| v.owner_id.clone());
                    crate::protocol::VideoSourceStatus {
                        active: v.active && v.width > 0 && v.height > 0,
                        owner_id: v.owner_id.clone(),
                        owner_name,
                        title: v.title.clone(),
                        source_url: v.source_url.clone(),
                        width: v.width,
                        height: v.height,
                        fps: v.fps,
                        frames: v.frames,
                    }
                };
                st.video_cache = {
                    let mut list: Vec<_> = state.video_cache.lock().values().cloned().collect();
                    list.sort_by(|a, b| a.id.cmp(&b.id));
                    list
                };
                st.client_list = {
                    let connected = state.connected_clients.lock();
                    cfg.clients
                        .iter()
                        .map(|c| crate::protocol::ClientInfo {
                            id: c.id.clone(),
                            name: c.name.clone(),
                            connected: connected.values().any(|id| *id == c.id),
                            revoked: c.revoked,
                        })
                        .collect()
                };
                st.rhythm = if let Some(bpm) = cfg.render.manual_bpm {
                    crate::protocol::RhythmStatus {
                        active: true,
                        source: "manual".into(),
                        detail: "manual override".into(),
                        bpm: bpm * cfg.render.beat_time.multiplier(),
                        beat_phase: audio[0].beat_phase,
                        running: true,
                        ..Default::default()
                    }
                } else if external_selected && external_clock.usable {
                    crate::protocol::RhythmStatus {
                        active: true,
                        source: if pioneer_selected {
                            "pro_dj_link".into()
                        } else {
                            "midi_clock".into()
                        },
                        detail: if pioneer_selected {
                            pioneer_label.clone()
                        } else {
                            cfg.rhythm.midi_port.clone().unwrap_or_default()
                        },
                        bpm: external_clock.bpm * cfg.render.beat_time.multiplier(),
                        beat_phase: audio[0].beat_phase,
                        running: external_clock.running,
                        age_ms: external_clock.age_ms,
                        ..Default::default()
                    }
                } else if external_selected && cfg.rhythm.fallback_to_audio {
                    let fallback = &audio_inputs[fallback_index];
                    let id = cfg
                        .audio
                        .sources
                        .get(fallback_index)
                        .map(|s| s.id.as_str())
                        .unwrap_or("missing");
                    crate::protocol::RhythmStatus {
                        active: fallback.active && fallback.bpm > 0.0,
                        using_fallback: true,
                        source: "audio".into(),
                        detail: format!(
                            "{} unavailable; following {id}",
                            if pioneer_selected {
                                "PRO DJ LINK"
                            } else {
                                "MIDI"
                            }
                        ),
                        bpm: audio[0].bpm,
                        beat_phase: audio[0].beat_phase,
                        running: fallback.active,
                        age_ms: external_clock.age_ms,
                    }
                } else if external_selected {
                    let detail = if pioneer_selected && !pioneer_error.is_empty() {
                        pioneer_error.clone()
                    } else if pioneer_selected {
                        "waiting for PRO DJ LINK beat packets".into()
                    } else if cfg.rhythm.midi_port.is_none() {
                        "select a MIDI input".into()
                    } else if !external_clock.running && external_clock.bpm > 0.0 {
                        "MIDI transport stopped".into()
                    } else if external_clock.bpm > 0.0 {
                        "MIDI clock timed out".into()
                    } else {
                        format!(
                            "waiting for {}",
                            cfg.rhythm.midi_port.as_deref().unwrap_or("MIDI clock")
                        )
                    };
                    crate::protocol::RhythmStatus {
                        source: if pioneer_selected {
                            "pro_dj_link".into()
                        } else {
                            "midi_clock".into()
                        },
                        detail,
                        running: external_clock.running,
                        age_ms: external_clock.age_ms,
                        ..Default::default()
                    }
                } else {
                    let display = audio_inputs
                        .iter()
                        .enumerate()
                        .find(|(i, a)| *i < cfg.audio.sources.len() && a.active && a.bpm > 0.0)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    let id = cfg
                        .audio
                        .sources
                        .get(display)
                        .map(|s| s.id.as_str())
                        .unwrap_or("none");
                    crate::protocol::RhythmStatus {
                        active: audio_inputs[display].active && audio[display].bpm > 0.0,
                        source: "layer_audio".into(),
                        detail: format!("per-layer audio; showing {id}"),
                        bpm: audio[display].bpm,
                        beat_phase: audio[display].beat_phase,
                        running: audio_inputs[display].active,
                        ..Default::default()
                    }
                };
                st.audio = state
                    .audio
                    .iter()
                    .zip(cfg.audio.sources.iter())
                    .enumerate()
                    .map(|(i, (slot, src))| {
                        let a = slot.lock();
                        crate::protocol::AudioSourceStatus {
                            id: src.id.clone(),
                            active: a.active,
                            detail: match a.health {
                                crate::audio::HEALTH_WAITING => "waiting for device".into(),
                                _ => String::new(),
                            },
                            level: a.level,
                            bass: a.bass,
                            mid: a.mid,
                            treble: a.treble,
                            bpm: audio[i].bpm,
                            bpm_confidence: audio[i].bpm_conf,
                            beat_phase: audio[i].beat_phase,
                        }
                    })
                    .collect();
                st.clone()
            };
            let _ = state.events.send(ServerMsg::Status { status });
        }

        // Decay the shake control input.
        state.control.lock().shake *= (-dt / 0.3).exp();
    }

    // Shutdown (app exit). Close the stream so the rig goes dark deliberately
    // instead of freezing on the last frame — but not after a handover, where the
    // successor is already driving the same CID.
    if !state.leaving.load(Ordering::SeqCst)
        && let Some(s) = sacn.as_mut()
    {
        s.send_terminate();
    }
    state.sacn_terminated.store(true, Ordering::SeqCst);
}
