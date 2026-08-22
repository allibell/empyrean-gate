//! Mapping between pixel indices, physical polar positions, and sACN universes.
//!
//! Pixel indexing convention everywhere in this app: `idx = spoke * pixels_per_spoke + i`
//! where `i = 0` is the OUTER end of the spoke (strings are fed from outside) and the
//! last pixel is at the inner radius.

use crate::config::{GeometryConfig, OutputConfig};

/// Polar position of a pixel. Angle in radians, radius in feet.
pub fn pixel_polar(geo: &GeometryConfig, spoke: u32, i: u32) -> (f32, f32) {
    let angle = spoke as f32 / geo.spokes as f32 * std::f32::consts::TAU;
    let t = if geo.pixels_per_spoke > 1 {
        i as f32 / (geo.pixels_per_spoke - 1) as f32
    } else {
        0.0
    };
    let radius = geo.outer_radius_ft + (geo.inner_radius_ft - geo.outer_radius_ft) * t;
    (angle, radius)
}

/// Universes needed per spoke (each spoke starts on a fresh universe boundary).
pub fn universes_per_spoke(geo: &GeometryConfig, out: &OutputConfig) -> u32 {
    let ppu = out.pixels_per_universe.max(1) as u32;
    geo.pixels_per_spoke.div_ceil(ppu)
}

pub fn total_universes(geo: &GeometryConfig, out: &OutputConfig) -> u32 {
    universes_per_spoke(geo, out).saturating_mul(geo.spokes)
}

/// The unicast destination (controller IP) for a given spoke, if configured.
pub fn controller_for_spoke(out: &OutputConfig, spoke: u32) -> Option<&str> {
    let idx = (spoke / out.strings_per_controller.max(1)) as usize;
    out.controllers.get(idx).map(|s| s.as_str()).filter(|s| !s.is_empty())
}

/// Implied physical strip length vs. the configured radii, for the settings UI to
/// display ("350 px at 60/m = 5.83 m = 19.1 ft; outer->inner span is 17.0 ft").
pub fn implied_strip_ft(geo: &GeometryConfig) -> (f32, f32) {
    let strip_m = geo.pixels_per_spoke as f32 / geo.leds_per_meter.max(1.0);
    let strip_ft = strip_m * 3.28084;
    let span_ft = geo.outer_radius_ft - geo.inner_radius_ft;
    (strip_ft, span_ft)
}
