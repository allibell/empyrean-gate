//! Headless Vulkan engine smoke test and repeatable performance harness.
//!
//! With no arguments this remains a quick correctness smoke test. `--suite` runs
//! the regression matrix up to 500k pixels; `--json` emits machine-readable output.

use anyhow::{Context, Result, bail};
use empyrean_gate_lib::config::AppConfig;
use empyrean_gate_lib::engine::{AudioUniform, Engine, FrameInputs, Globals, SCOPE_FLOATS};
use empyrean_gate_lib::layers::{
    GpuDab, GpuEffect, LayerCfg, LayerKind, MAX_AUDIO_SOURCES, MAX_DABS, MAX_EFFECTS, MAX_LAYERS,
};
use serde::Serialize;
use std::time::Instant;

const MAX_BENCH_PIXELS: u32 = 500_000;
const BENCH_VIDEO_DIMENSION: u32 = 96;

#[derive(Debug, Clone, PartialEq)]
struct Options {
    warmup_frames: u32,
    measured_frames: u32,
    fps_budget: f64,
    json: bool,
    suite: bool,
    pixels: u32,
    layers: usize,
    effects: usize,
    dabs: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            warmup_frames: 30,
            measured_frames: 120,
            fps_budget: 60.0,
            json: false,
            suite: false,
            pixels: 24_192,
            layers: AppConfig::default().layers.len(),
            effects: 0,
            dabs: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Scenario {
    name: String,
    pixels: u32,
    layers: usize,
    effects: usize,
    dabs: usize,
    video_only: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    app_version: &'static str,
    revision: String,
    gpu: String,
    os: &'static str,
    arch: &'static str,
    warmup_frames: u32,
    measured_frames: u32,
    fps_budget: f64,
    budget_ms: f64,
    results: Vec<ScenarioResult>,
}

#[derive(Debug, Serialize)]
struct ScenarioResult {
    name: String,
    pixels: u32,
    spokes: u32,
    pixels_per_spoke: u32,
    layers: usize,
    effects: usize,
    dabs: usize,
    samples: usize,
    mean_ms: f64,
    min_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    stddev_ms: f64,
    throughput_fps: f64,
    missed_budget_frames: usize,
    missed_budget_percent: f64,
    checksum: String,
    nonzero_bytes: usize,
}

#[derive(Debug, PartialEq)]
struct TimingStats {
    mean: f64,
    min: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    stddev: f64,
    missed: usize,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ENGINE BENCH FAILED: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }
    let options = parse_options(&args)?;
    let scenarios = scenarios(&options)?;

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let first_pixels = scenarios
        .first()
        .context("benchmark has no scenarios")?
        .pixels;
    let engine = std::panic::catch_unwind(|| Engine::new(first_pixels)).unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown panic".into());
        Err(anyhow::anyhow!(
            "No Vulkan adapter (GPU init panicked: {msg})"
        ))
    });
    let mut engine = engine.context("engine initialization")?;
    if !options.json {
        println!("adapter: {}", engine.gpu_name);
    }

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        engine.ensure_capacity(scenario.pixels);
        if !options.json {
            println!(
                "running {}: {} px, {} layers, {} effects, {} dabs ({} warmup + {} measured)",
                scenario.name,
                scenario.pixels,
                scenario.layers,
                scenario.effects,
                scenario.dabs,
                options.warmup_frames,
                options.measured_frames
            );
        }
        results.push(run_scenario(&mut engine, &options, scenario)?);
    }

    let report = Report {
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION"),
        revision: revision(),
        gpu: engine.gpu_name.clone(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        warmup_frames: options.warmup_frames,
        measured_frames: options.measured_frames,
        fps_budget: options.fps_budget,
        budget_ms: 1000.0 / options.fps_budget,
        results,
    };
    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(())
}

fn run_scenario(
    engine: &mut Engine,
    options: &Options,
    scenario: Scenario,
) -> Result<ScenarioResult> {
    let (spokes, pixels_per_spoke) = geometry_for(scenario.pixels);
    let mut inputs = make_inputs(&scenario, spokes, pixels_per_spoke);
    for frame in 0..options.warmup_frames {
        advance_inputs(&mut inputs, frame);
        engine
            .render(&inputs)
            .with_context(|| format!("{} warmup frame {frame}", scenario.name))?;
    }

    let mut samples = Vec::with_capacity(options.measured_frames as usize);
    let mut checksum = 0u64;
    let mut nonzero_bytes = 0usize;
    for frame in 0..options.measured_frames {
        advance_inputs(&mut inputs, options.warmup_frames + frame);
        let started = Instant::now();
        let rgb = engine
            .render(&inputs)
            .with_context(|| format!("{} measured frame {frame}", scenario.name))?;
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        if frame + 1 == options.measured_frames
            && let Some(rgb) = rgb
        {
            for &byte in rgb {
                checksum = checksum.wrapping_mul(31).wrapping_add(byte as u64);
                nonzero_bytes += usize::from(byte != 0);
            }
        }
    }
    if nonzero_bytes == 0 {
        bail!("{} produced an entirely black final frame", scenario.name);
    }

    let budget_ms = 1000.0 / options.fps_budget;
    let stats = timing_stats(&samples, budget_ms);
    Ok(ScenarioResult {
        name: scenario.name,
        pixels: scenario.pixels,
        spokes,
        pixels_per_spoke,
        layers: scenario.layers,
        effects: scenario.effects,
        dabs: scenario.dabs,
        samples: samples.len(),
        mean_ms: stats.mean,
        min_ms: stats.min,
        p50_ms: stats.p50,
        p95_ms: stats.p95,
        p99_ms: stats.p99,
        max_ms: stats.max,
        stddev_ms: stats.stddev,
        throughput_fps: 1000.0 / stats.mean,
        missed_budget_frames: stats.missed,
        missed_budget_percent: stats.missed as f64 * 100.0 / samples.len() as f64,
        checksum: format!("{checksum:#018x}"),
        nonzero_bytes,
    })
}

fn scenarios(options: &Options) -> Result<Vec<Scenario>> {
    if options.suite {
        if options.pixels != Options::default().pixels
            || options.layers != Options::default().layers
            || options.effects != 0
            || options.dabs != 0
        {
            bail!("--suite cannot be combined with --pixels/--layers/--effects/--dabs");
        }
        Ok(vec![
            Scenario {
                name: "installed-default".into(),
                pixels: 24_192,
                layers: 4,
                effects: 0,
                dabs: 0,
                video_only: false,
            },
            Scenario {
                name: "installed-heavy".into(),
                pixels: 24_192,
                layers: LayerKind::ALL.len(),
                effects: 16,
                dabs: 128,
                video_only: false,
            },
            Scenario {
                name: "headroom-70k-heavy".into(),
                pixels: 70_000,
                layers: LayerKind::ALL.len(),
                effects: 16,
                dabs: 128,
                video_only: false,
            },
            Scenario {
                name: "video-only".into(),
                pixels: 24_192,
                layers: 1,
                effects: 0,
                dabs: 0,
                video_only: true,
            },
        ])
    } else {
        validate_workload(
            options.pixels,
            options.layers,
            options.effects,
            options.dabs,
        )?;
        Ok(vec![Scenario {
            name: "custom".into(),
            pixels: options.pixels,
            layers: options.layers,
            effects: options.effects,
            dabs: options.dabs,
            video_only: false,
        }])
    }
}

fn make_inputs(scenario: &Scenario, spokes: u32, pixels: u32) -> FrameInputs {
    let cfg = AppConfig::default();
    let mut layers: Vec<_> = if scenario.video_only {
        vec![LayerCfg {
            kind: LayerKind::Video,
            hue_range: 1.0,
            param_a: 1.0,
            param_b: 0.0,
            param_c: 0.5,
            param_d: 0.5,
            ..Default::default()
        }
        .to_gpu(0.0)]
    } else {
        cfg.layers.iter().map(|layer| layer.to_gpu(0.0)).collect()
    };
    while layers.len() < scenario.layers {
        let kind = LayerKind::ALL[layers.len() % LayerKind::ALL.len()];
        layers.push(
            LayerCfg {
                kind,
                ..Default::default()
            }
            .to_gpu(0.0),
        );
    }
    layers.truncate(scenario.layers);
    let has_video = layers
        .iter()
        .any(|layer| layer.kind == LayerKind::Video.gpu_id());

    let effects: Vec<GpuEffect> = (0..scenario.effects)
        .map(|i| GpuEffect {
            kind: (i % 4) as u32,
            age: 0.2,
            duration: 1.5,
            angle: i as f32 * 0.37,
            radius: 0.25 + (i % 4) as f32 * 0.2,
            intensity: 1.0,
            hue: (i as f32 * 0.11) % 1.0,
            ..Default::default()
        })
        .collect();
    let dabs: Vec<GpuDab> = (0..scenario.dabs)
        .map(|i| GpuDab {
            kind: (i % 7) as u32,
            age: (i % 10) as f32 / 20.0,
            angle: i as f32 * 0.19,
            radius: 0.15 + (i % 17) as f32 / 22.0,
            hue: (i as f32 * 0.07) % 1.0,
            size: 0.12,
            intensity: 1.0,
            dir: i as f32 * 0.13,
            saturation: 0.85,
            brightness: 1.0,
            ..Default::default()
        })
        .collect();
    let mut scope = vec![0.0; SCOPE_FLOATS * MAX_AUDIO_SOURCES];
    for (i, value) in scope.iter_mut().enumerate() {
        *value = (i as f32 * 0.071).sin() * 0.7;
    }
    let mut audio = [AudioUniform::default(); MAX_AUDIO_SOURCES];
    for (i, slot) in audio.iter_mut().enumerate() {
        *slot = AudioUniform {
            level: 0.6,
            bass: 0.7,
            mid: 0.4,
            treble: 0.5,
            onset: 0.8,
            beat_phase: 0.3,
            bpm: 128.0 + i as f32,
            bass_att: 0.5,
            mid_att: 0.4,
            treble_att: 0.4,
            ..Default::default()
        };
    }
    FrameInputs {
        globals: Globals {
            spokes,
            pixels,
            layer_count: layers.len() as u32,
            effect_count: effects.len() as u32,
            dt: 1.0 / 60.0,
            master: 1.0,
            inner_over_outer: 8.0 / 25.0,
            dab_count: dabs.len() as u32,
            video_width: if has_video { BENCH_VIDEO_DIMENSION } else { 0 },
            video_height: if has_video { BENCH_VIDEO_DIMENSION } else { 0 },
            video_active: u32::from(has_video),
            ..Default::default()
        },
        audio,
        layers,
        effects,
        dabs,
        scope,
        video_upload: has_video.then(benchmark_video_frame),
    }
}

fn benchmark_video_frame() -> Vec<u8> {
    let mut rgba = Vec::with_capacity((BENCH_VIDEO_DIMENSION * BENCH_VIDEO_DIMENSION * 4) as usize);
    for y in 0..BENCH_VIDEO_DIMENSION {
        for x in 0..BENCH_VIDEO_DIMENSION {
            rgba.extend_from_slice(&[
                (x * 255 / (BENCH_VIDEO_DIMENSION - 1)) as u8,
                (y * 255 / (BENCH_VIDEO_DIMENSION - 1)) as u8,
                ((x ^ y) * 255 / (BENCH_VIDEO_DIMENSION - 1)) as u8,
                255,
            ]);
        }
    }
    rgba
}

fn advance_inputs(inputs: &mut FrameInputs, frame: u32) {
    // A live source uploads only on a new browser frame. Seed the GPU on the first
    // warmup frame, then benchmark steady-state sampling rather than PCIe traffic.
    if frame > 0 {
        inputs.video_upload = None;
    }
    let time = frame as f32 / 60.0;
    inputs.globals.time = time;
    for (i, layer) in inputs.layers.iter_mut().enumerate() {
        layer.phase = time * (0.7 + i as f32 * 0.03);
    }
    for effect in &mut inputs.effects {
        effect.age = (effect.age + 1.0 / 60.0) % effect.duration.max(0.001);
    }
    for dab in &mut inputs.dabs {
        dab.age = (dab.age + 1.0 / 120.0) % 0.95;
    }
    for (i, audio) in inputs.audio.iter_mut().enumerate() {
        audio.beat_phase = (time * audio.bpm / 60.0 + i as f32 * 0.17).fract();
    }
}

fn geometry_for(total_pixels: u32) -> (u32, u32) {
    let max_spokes = total_pixels.min(64);
    let spokes = (1..=max_spokes)
        .rev()
        .find(|candidate| total_pixels.is_multiple_of(*candidate))
        .unwrap_or(1);
    (spokes, total_pixels / spokes)
}

fn timing_stats(samples: &[f64], budget_ms: f64) -> TimingStats {
    assert!(!samples.is_empty());
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / sorted.len() as f64;
    TimingStats {
        mean,
        min: sorted[0],
        p50: percentile(&sorted, 0.50),
        p95: percentile(&sorted, 0.95),
        p99: percentile(&sorted, 0.99),
        max: *sorted.last().unwrap(),
        stddev: variance.sqrt(),
        missed: sorted.iter().filter(|&&v| v > budget_ms).count(),
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * percentile).round() as usize;
    sorted[index]
}

fn parse_options(args: &[String]) -> Result<Options> {
    let mut options = Options::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => options.json = true,
            "--suite" => options.suite = true,
            "--warmup" => options.warmup_frames = parse_next(args, &mut i, "--warmup")?,
            "--frames" => options.measured_frames = parse_next(args, &mut i, "--frames")?,
            "--fps-budget" => options.fps_budget = parse_next(args, &mut i, "--fps-budget")?,
            "--pixels" => options.pixels = parse_next(args, &mut i, "--pixels")?,
            "--layers" => options.layers = parse_next(args, &mut i, "--layers")?,
            "--effects" => options.effects = parse_next(args, &mut i, "--effects")?,
            "--dabs" => options.dabs = parse_next(args, &mut i, "--dabs")?,
            unknown => bail!("unknown argument `{unknown}` (try --help)"),
        }
        i += 1;
    }
    if options.warmup_frames == 0 {
        bail!("--warmup must be at least 1 so the ping-pong pipeline is primed");
    }
    if options.measured_frames == 0 {
        bail!("--frames must be at least 1");
    }
    if !options.fps_budget.is_finite() || options.fps_budget <= 0.0 {
        bail!("--fps-budget must be a positive finite number");
    }
    Ok(options)
}

fn revision() -> String {
    if let Ok(value) = std::env::var("EMPYREAN_BENCH_REVISION") {
        return value;
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn parse_next<T: std::str::FromStr>(args: &[String], index: &mut usize, flag: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    *index += 1;
    let value = args
        .get(*index)
        .with_context(|| format!("{flag} requires a value"))?;
    value
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid value for {flag}: `{value}` ({e})"))
}

fn validate_workload(pixels: u32, layers: usize, effects: usize, dabs: usize) -> Result<()> {
    if !(1..=MAX_BENCH_PIXELS).contains(&pixels) {
        bail!("--pixels must be between 1 and {MAX_BENCH_PIXELS}");
    }
    if layers == 0 || layers > MAX_LAYERS {
        bail!("--layers must be between 1 and {MAX_LAYERS}");
    }
    if effects > MAX_EFFECTS {
        bail!("--effects must be at most {MAX_EFFECTS}");
    }
    if dabs > MAX_DABS {
        bail!("--dabs must be at most {MAX_DABS}");
    }
    Ok(())
}

fn print_report(report: &Report) {
    println!(
        "\nengine benchmark (budget {:.2} ms @ {:.1} fps)",
        report.budget_ms, report.fps_budget
    );
    println!(
        "scenario                 pixels layers fx dabs   mean    p50    p95    p99    max  misses"
    );
    for result in &report.results {
        println!(
            "{:<24} {:>6} {:>6} {:>2} {:>4} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>4}/{:<4}",
            result.name,
            result.pixels,
            result.layers,
            result.effects,
            result.dabs,
            result.mean_ms,
            result.p50_ms,
            result.p95_ms,
            result.p99_ms,
            result.max_ms,
            result.missed_budget_frames,
            result.samples
        );
    }
    println!("engine benchmark OK");
}

fn print_help() {
    println!(
        "Engine correctness smoke test and GPU benchmark.\n\n\
Usage: engine-smoke [OPTIONS]\n\n\
  --suite              Run 24,192-pixel installed and 70k headroom scenarios\n\
  --pixels N           Total pixels for a custom scenario (max 500000)\n\
  --layers N           Active layers (1..24)\n\
  --effects N          Concurrent effects (0..32)\n\
  --dabs N             Concurrent drawing dabs (0..512)\n\
  --warmup N           Unmeasured warmup frames (default 30)\n\
  --frames N           Measured frames (default 120)\n\
  --fps-budget N       Deadline used for missed-frame telemetry (default 60)\n\
  --json               Print a versioned JSON report\n\
  -h, --help           Print this help\n\n\
Examples:\n\
  engine-smoke\n\
  engine-smoke --pixels 70000 --layers 20 --effects 16 --dabs 128\n\
  engine-smoke --suite --warmup 120 --frames 600 --json"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_custom_workload() {
        let parsed = parse_options(&args(&[
            "--pixels",
            "70000",
            "--layers",
            "19",
            "--effects",
            "16",
            "--dabs",
            "128",
            "--warmup",
            "10",
            "--frames",
            "20",
            "--fps-budget",
            "120",
            "--json",
        ]))
        .unwrap();
        assert_eq!(parsed.pixels, 70_000);
        assert_eq!(parsed.layers, 19);
        assert_eq!(parsed.effects, 16);
        assert_eq!(parsed.dabs, 128);
        assert_eq!(parsed.warmup_frames, 10);
        assert_eq!(parsed.measured_frames, 20);
        assert_eq!(parsed.fps_budget, 120.0);
        assert!(parsed.json);
    }

    #[test]
    fn caps_pixel_workloads_at_500k() {
        let options = parse_options(&args(&["--pixels", "500001"])).unwrap();
        assert!(scenarios(&options).is_err());
    }

    #[test]
    fn heavy_suite_exercises_the_video_layer() {
        let options = Options {
            suite: true,
            ..Default::default()
        };
        let heavy = scenarios(&options).unwrap().remove(1);
        assert_eq!(heavy.layers, LayerKind::ALL.len());

        let inputs = make_inputs(&heavy, 64, 378);
        assert_eq!(inputs.globals.video_active, 1);
        assert_eq!(inputs.globals.video_width, BENCH_VIDEO_DIMENSION);
        assert_eq!(inputs.video_upload.unwrap().len(), 96 * 96 * 4);
    }

    #[test]
    fn rejects_unprimed_measurement() {
        assert!(parse_options(&args(&["--warmup", "0"])).is_err());
    }

    #[test]
    fn geometry_preserves_exact_pixel_count() {
        for pixels in [24_192, 70_000, 500_000] {
            let (spokes, pixels_per_spoke) = geometry_for(pixels);
            assert!(spokes <= 64);
            assert_eq!(spokes * pixels_per_spoke, pixels);
        }
    }

    #[test]
    fn statistics_report_percentiles_and_deadlines() {
        let stats = timing_stats(&[1.0, 2.0, 3.0, 4.0, 20.0], 4.0);
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.p50, 3.0);
        assert_eq!(stats.p95, 20.0);
        assert_eq!(stats.p99, 20.0);
        assert_eq!(stats.max, 20.0);
        assert_eq!(stats.missed, 1);
    }
}
