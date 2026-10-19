//! A8 release-profile paint timings for the 800×120 time-strip fixture.
//!
//! Run: `cargo test -p hx-native --release --test a8_release_timing -- --nocapture`
//!
//! Reports median / p95 nanoseconds for:
//! - full dirty paint (after load+sync)
//! - paint after a `time` slice swap
//! - clean frame (must skip raster)
//!
//! These numbers are the ones to compare against FRAME_BUDGET (ideal ≤ slice of
//! 16.6 ms). The debug `a8_paint_budget` test only guards a generous ceiling.

use std::collections::HashMap;

use hx_native::*;

const FIXTURE: &str = include_str!("fixtures/time_strip.html");
const SAMPLES: usize = 31;

fn view() -> Viewport {
    Viewport {
        width: 800,
        height: 120,
        holes: vec![],
    }
}

struct MemSlices {
    versions: HashMap<String, u64>,
}

impl MemSlices {
    fn new() -> Self {
        let mut versions = HashMap::new();
        versions.insert("time".into(), 1);
        versions.insert("actions".into(), 1);
        Self { versions }
    }
    fn bump_time(&mut self) {
        *self.versions.entry("time".into()).or_insert(0) += 1;
    }
}

impl SliceSource for MemSlices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
}

struct Frags {
    body: String,
}

impl FragmentSource for Frags {
    fn render(&mut self, _section_id: &str, _slices: &[&str]) -> String {
        self.body.clone()
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p * (sorted.len() as f64 - 1.0)).round() as usize).min(sorted.len() - 1);
    sorted[idx]
}

fn stats(mut samples: Vec<u64>) -> (u64, u64, u64) {
    samples.sort_unstable();
    let median = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let max = *samples.last().unwrap_or(&0);
    (median, p95, max)
}

fn boot() -> (Engine, MemSlices, Frags) {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 16_600_000, // honest ideal frame; we report, not hard-fail vs FRAME_BUDGET here
        pending_timeout_frames: 120,
    });
    eng.declare_actions(&[
        "time.pause",
        "time.set_speed",
        "time.pause_menu",
        "time.set_speed.bulk",
        "time.context",
    ]);
    eng.load_html(FIXTURE).unwrap();
    let slices = MemSlices::new();
    let frags = Frags {
        body: r#"<span id="time-label">DATE 2026-10-19 · 1x</span>"#.into(),
    };
    (eng, slices, frags)
}

#[test]
fn a8_release_paint_timings() {
    if cfg!(debug_assertions) {
        eprintln!("a8_release_timing: skipping measurement in debug profile; run with --release");
        return;
    }

    let mut full = Vec::with_capacity(SAMPLES);
    let mut after_swap = Vec::with_capacity(SAMPLES);
    let mut clean = Vec::with_capacity(SAMPLES);

    for i in 0..SAMPLES {
        let (mut eng, mut slices, mut frags) = boot();
        eng.sync(&slices, &mut frags).unwrap();

        let out = eng.paint(view()).unwrap();
        assert!(!out.skipped);
        full.push(eng.last_timings().paint_ns);

        frags.body = format!(r#"<span id="time-label">DATE tick-{i}</span>"#);
        slices.bump_time();
        eng.sync(&slices, &mut frags).unwrap();
        let out = eng.paint(view()).unwrap();
        assert!(!out.skipped, "swap must dirty paint");
        after_swap.push(eng.last_timings().paint_ns);

        let out = eng.paint(view()).unwrap();
        assert!(out.skipped, "clean frame must skip raster");
        assert!(eng.last_timings().paint_skipped);
        clean.push(eng.last_timings().paint_ns);
    }

    let (f_med, f_p95, f_max) = stats(full);
    let (s_med, s_p95, s_max) = stats(after_swap);
    let (c_med, c_p95, c_max) = stats(clean);

    eprintln!("A8 release timings (800x120 time-strip, n={SAMPLES}):");
    eprintln!("  full paint after sync:     median={f_med} ns  p95={f_p95} ns  max={f_max} ns");
    eprintln!("  paint after time swap:     median={s_med} ns  p95={s_p95} ns  max={s_max} ns");
    eprintln!("  clean/skipped frame:       median={c_med} ns  p95={c_p95} ns  max={c_max} ns");
    eprintln!(
        "  FRAME_BUDGET context: ideal frame 16.6 ms; floor 25 ms (40 FPS). Chrome paint should be a small slice."
    );

    // Clean skip must stay far below a millisecond.
    assert!(
        c_p95 < 500_000,
        "clean-frame p95 must be <<1ms, got {c_p95} ns"
    );
    // Dirty paints must do real work (sanity).
    assert!(f_med > 100_000, "full paint median suspiciously low");
    assert!(s_med > 100_000, "swap paint median suspiciously low");
}
