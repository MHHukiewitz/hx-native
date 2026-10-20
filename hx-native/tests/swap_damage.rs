//! Issue #20: sync_surface swap damage must match a forced full repaint when
//! the panel is centered with `transform: translate(-50%, -50%)` (Mainnet Pause).
//!
//! `absolute_position` ignores CSS transforms; damage must use the shared
//! transform-aware box helper. At scales 1 / 1.25 / 2, partial ≡ full. A
//! leftover pending full-surface rect must force `full_repaint`.

#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hx_native::*;

fn boot(html: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(&["menu.exit", "menu.cancel"]);
    eng.load_html(html).expect("load");
    eng
}

struct MemSlices {
    versions: HashMap<String, u64>,
}

impl MemSlices {
    fn new() -> Self {
        let mut versions = HashMap::new();
        versions.insert("pause".into(), 1);
        Self { versions }
    }
    fn bump(&mut self, name: &str) {
        *self.versions.entry(name.into()).or_insert(0) += 1;
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

struct SpyFrags {
    bodies: HashMap<String, String>,
    calls: Arc<Mutex<usize>>,
}

impl SpyFrags {
    fn new() -> Self {
        Self {
            bodies: HashMap::new(),
            calls: Arc::new(Mutex::new(0)),
        }
    }
    fn set(&mut self, section: &str, html: &str) {
        self.bodies.insert(section.into(), html.into());
    }
}

impl FragmentSource for SpyFrags {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        *self.calls.lock().unwrap() += 1;
        self.bodies
            .get(section_id)
            .cloned()
            .unwrap_or_else(|| format!("<!-- {section_id} -->"))
    }
}

/// Mainnet Pause shape: panel at left/top 50% with `translate(-50%, -50%)`.
/// Without transform-aware boxes, damage lands at the untransformed spot
/// (~640,400) while pixels change around (~480,236).
fn pause_page() -> String {
    r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  width: 1280px; height: 800px;
  background: #0e1218; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden;
}
#pause-panel {
  position: absolute; left: 50%; top: 50%; width: 320px; height: 328px;
  transform: translate(-50%, -50%);
  background: #2a3140; border: 2px solid #667;
}
#pause-title {
  position: absolute; left: 16px; top: 16px; width: 288px; height: 28px;
  font-size: 18px;
}
#pause-exit {
  position: absolute; left: 16px; top: 60px; width: 288px; height: 240px;
  overflow: hidden;
}
#exit-row {
  position: absolute; left: 0; top: 0; width: 160px; height: 36px; line-height: 36px;
  background: #c04040; padding: 0 12px; color: #fff;
}
#exit-confirm {
  position: absolute; left: 40px; top: 80px; width: 200px;
}
#exit-q {
  display: block; width: 200px; height: 40px; line-height: 40px;
  background: #1a2030; padding: 0 8px; margin-bottom: 12px;
}
#exit-yes, #exit-no {
  display: inline-block; width: 90px; height: 36px; line-height: 36px;
  text-align: center; margin-right: 8px; background: #3d7ea6; color: #fff;
  border: 1px solid #556;
}
#exit-no { background: #5a4040; }
</style></head><body>
<div id="pause-panel">
  <div id="pause-title">Paused</div>
  <section id="pause-exit" hx-trigger="slice:pause" hx-target="#pause-exit" hx-swap="innerHTML">
    <div id="exit-row">Exit game…</div>
  </section>
</div>
</body></html>"##
        .to_string()
}

/// Ancestor carries `translate(150px, 100px)`; swap target is inside it.
fn transformed_ancestor_page() -> String {
    r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* { box-sizing: border-box; margin: 0; padding: 0; }
body { width: 800px; height: 600px; background: #111; color: #eee;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; }
#outer {
  position: absolute; left: 40px; top: 40px; width: 400px; height: 300px;
  transform: translate(150px, 100px);
  background: #222;
}
#sec { position: absolute; left: 20px; top: 20px; width: 200px; height: 120px; }
#old { width: 180px; height: 40px; background: #c04040; }
#new { width: 120px; height: 80px; background: #40c040; margin-top: 20px; }
</style></head><body>
<div id="outer">
  <section id="sec" hx-trigger="slice:pause" hx-target="#sec" hx-swap="innerHTML">
    <div id="old">old</div>
  </section>
</div>
</body></html>"##
        .to_string()
}

fn exit_row_html() -> String {
    r#"<div id="exit-row">Exit game…</div>"#.into()
}

fn exit_confirm_html() -> String {
    r#"<div id="exit-confirm">
  <span id="exit-q">Really exit?</span>
  <button id="exit-yes" hx-post="menu.exit" hx-trigger="click">Yes</button>
  <button id="exit-no" hx-post="menu.cancel" hx-trigger="click">No</button>
</div>"#
        .into()
}

fn vp_css(css_w: u32, css_h: u32, scale: f64) -> Viewport {
    let w = ((css_w as f64) * scale).round() as u32;
    let h = ((css_h as f64) * scale).round() as u32;
    Viewport::with_scale(w, h, scale)
}

fn pixel_diffs(a: &RgbaImage, b: &RgbaImage) -> u32 {
    let mut diffs = 0u32;
    for (pa, pb) in a.pixels.chunks_exact(4).zip(b.pixels.chunks_exact(4)) {
        if pa != pb {
            diffs += 1;
        }
    }
    diffs
}

fn paint_swap_partial(
    html: &str,
    section: &str,
    before: &str,
    after: &str,
    scale: f64,
    css_w: u32,
    css_h: u32,
) -> (PaintOutput, PaintStats) {
    let mut eng = boot(html);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set(section, before);
    eng.sync(&slices, &mut frags).unwrap();
    let view = vp_css(css_w, css_h, scale);
    let _ = eng.paint(view.clone()).unwrap();

    slices.bump("pause");
    frags.set(section, after);
    eng.sync(&slices, &mut frags).unwrap();
    let out = eng.paint(view).unwrap();
    let stats = eng.last_paint_stats();
    (out, stats)
}

fn paint_swap_forced_full(
    html: &str,
    section: &str,
    after: &str,
    scale: f64,
    css_w: u32,
    css_h: u32,
) -> PaintOutput {
    let mut eng = boot(html);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set(section, after);
    slices.bump("pause");
    eng.sync(&slices, &mut frags).unwrap();
    eng.mark_theme_or_stylesheet_changed();
    let view = vp_css(css_w, css_h, scale);
    eng.paint(view).unwrap()
}

fn assert_pause_swap_partial_matches_full(scale: f64) {
    let (partial, stats) = paint_swap_partial(
        &pause_page(),
        "pause-exit",
        &exit_row_html(),
        &exit_confirm_html(),
        scale,
        1280,
        800,
    );
    let full = paint_swap_forced_full(
        &pause_page(),
        "pause-exit",
        &exit_confirm_html(),
        scale,
        1280,
        800,
    );

    assert!(
        !partial.skipped,
        "swap paint must not skip at scale={scale}"
    );
    let a = partial.rgba.as_ref().unwrap();
    let b = full.rgba.as_ref().unwrap();
    let diffs = pixel_diffs(a, b);
    eprintln!(
        "pause transform scale={scale} diffs={diffs} full_repaint={} damage={:?} stats={:?}",
        stats.full_repaint, partial.damage, stats
    );
    assert_eq!(
        diffs, 0,
        "Pause translate(-50%,-50%) swap partial must match full at scale={scale}; \
         diffs={diffs} damage={:?}",
        partial.damage
    );
}

#[test]
fn swap_inner_section_partial_matches_full_scale_1() {
    assert_pause_swap_partial_matches_full(1.0);
}

#[test]
fn swap_inner_section_partial_matches_full_scale_2() {
    assert_pause_swap_partial_matches_full(2.0);
}

#[test]
fn swap_inner_section_partial_matches_full_scale_1_25() {
    assert_pause_swap_partial_matches_full(1.25);
}

#[test]
fn swap_under_transformed_ancestor_partial_matches_full() {
    let scale = 1.0;
    let (partial, stats) = paint_swap_partial(
        &transformed_ancestor_page(),
        "sec",
        r#"<div id="old">old</div>"#,
        r#"<div id="new">new</div>"#,
        scale,
        800,
        600,
    );
    let full = paint_swap_forced_full(
        &transformed_ancestor_page(),
        "sec",
        r#"<div id="new">new</div>"#,
        scale,
        800,
        600,
    );
    let diffs = pixel_diffs(partial.rgba.as_ref().unwrap(), full.rgba.as_ref().unwrap());
    eprintln!(
        "ancestor translate(150px,100px) diffs={diffs} damage={:?} full_repaint={}",
        partial.damage, stats.full_repaint
    );
    assert_eq!(
        diffs, 0,
        "transformed-ancestor swap must be byte-identical; diffs={diffs} damage={:?}",
        partial.damage
    );
}

/// A leftover pending full-surface rect must force `full_repaint=true`.
/// Fails if the full-surface rule in `compute_plan` is dropped.
#[test]
fn pending_full_surface_rect_forces_full_repaint() {
    let mut eng = boot(&pause_page());
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("pause-exit", &exit_row_html());
    eng.sync(&slices, &mut frags).unwrap();
    let view = vp_css(1280, 800, 2.0);
    let _ = eng.paint(view.clone()).unwrap();

    eng.test_push_pending_damage(Rect {
        x: 0.0,
        y: 0.0,
        width: view.width as f32,
        height: view.height as f32,
    });

    slices.bump("pause");
    frags.set("pause-exit", &exit_confirm_html());
    eng.sync(&slices, &mut frags).unwrap();
    let out = eng.paint(view.clone()).unwrap();
    let stats = eng.last_paint_stats();

    assert!(
        stats.full_repaint,
        "pending full-surface rect must force full_repaint; stats={stats:?} damage={:?}",
        out.damage
    );
    // Must be the explicit pending-full rule, not the area-fraction fallback
    // (without `|| pending_full` the area fallback can still set full_repaint).
    assert!(
        !stats.full_repaint_area_fallback,
        "pending full-surface must not rely on area fallback; stats={stats:?}"
    );
    assert_eq!(out.damage.len(), 1, "expected one full-surface damage rect");
    assert_eq!(out.damage[0].x, 0.0);
    assert_eq!(out.damage[0].y, 0.0);
    assert_eq!(out.damage[0].width, view.width as f32);
    assert_eq!(out.damage[0].height, view.height as f32);

    // With full_repaint forced, frame must still match a fresh full paint.
    let full = paint_swap_forced_full(
        &pause_page(),
        "pause-exit",
        &exit_confirm_html(),
        2.0,
        1280,
        800,
    );
    assert_eq!(
        pixel_diffs(out.rgba.as_ref().unwrap(), full.rgba.as_ref().unwrap()),
        0,
        "pending-full path must still be byte-identical to forced full"
    );
}

#[test]
fn transformed_panel_box_matches_visual_center() {
    let mut eng = boot(&pause_page());
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("pause-exit", &exit_row_html());
    eng.sync(&slices, &mut frags).unwrap();
    let _ = eng.paint(vp_css(1280, 800, 1.0)).unwrap();
    let (x, y, w, h) = eng.test_element_box("pause-panel").expect("panel box");
    // Visual top-left after translate(-50%,-50%) from (640,400) size 320×328.
    assert!(
        (x - 480.0).abs() < 2.0 && (y - 236.0).abs() < 2.0,
        "panel origin should be ~(480,236), got ({x},{y})"
    );
    assert!(
        (w - 320.0).abs() < 2.0 && (h - 328.0).abs() < 2.0,
        "panel size should be ~320×328, got ({w}×{h})"
    );
}

/// After a swap, a forced paint error must not drop pending/bounds: the next
/// paint (after another change) must match a full repaint (0 px). Without
/// `force_full_repaint` on rasterize failure the cached frame stays on the
/// pre-swap ink (~20,240 px wrong vs confirm at scale 1).
#[test]
fn paint_error_after_swap_recovers_with_full_repaint() {
    let mut eng = boot(&pause_page());
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("pause-exit", &exit_row_html());
    eng.sync(&slices, &mut frags).unwrap();
    let view = vp_css(1280, 800, 1.0);
    let _ = eng.paint(view.clone()).unwrap();

    // Swap to confirm, then force the paint that would apply it to error.
    slices.bump("pause");
    frags.set("pause-exit", &exit_confirm_html());
    eng.sync(&slices, &mut frags).unwrap();
    eng.test_fail_next_paint();
    let err = eng.paint(view.clone());
    assert!(err.is_err(), "expected forced paint error, got {err:?}");

    // One more change that does not cover the stale exit-row ink: a small
    // pending rect in the corner. Without force_full_repaint the next paint
    // only patches that corner and leaves ~20k wrong pixels in the panel.
    eng.test_push_pending_damage(Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    });
    let recovered = eng.paint(view.clone()).unwrap();
    assert!(!recovered.skipped);

    let expected = paint_swap_forced_full(
        &pause_page(),
        "pause-exit",
        &exit_confirm_html(),
        1.0,
        1280,
        800,
    );
    let diffs = pixel_diffs(
        recovered.rgba.as_ref().unwrap(),
        expected.rgba.as_ref().unwrap(),
    );
    eprintln!(
        "paint-error recovery diffs={diffs} full_repaint={} damage={:?}",
        eng.last_paint_stats().full_repaint,
        recovered.damage
    );
    assert_eq!(
        diffs, 0,
        "after paint error, next frame must match full repaint; diffs={diffs} damage={:?}",
        recovered.damage
    );
    assert!(
        eng.last_paint_stats().full_repaint,
        "paint-error recovery must force full_repaint; stats={:?}",
        eng.last_paint_stats()
    );
}

/// Out-of-bounds pending rects are clamped on push (single pending list).
#[test]
fn pending_damage_clamps_out_of_bounds_on_push() {
    let mut eng = boot(&pause_page());
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("pause-exit", &exit_row_html());
    eng.sync(&slices, &mut frags).unwrap();
    let view = vp_css(1280, 800, 1.0);
    let _ = eng.paint(view.clone()).unwrap();

    // Negative origin + oversized extent → clamp to surface; must not panic
    // and the next paint's damage must stay inside the surface.
    eng.test_push_pending_damage(Rect {
        x: -80.0,
        y: -40.0,
        width: 200.0,
        height: 100.0,
    });
    let out = eng.paint(view.clone()).unwrap();
    assert!(!out.skipped);
    for r in &out.damage {
        assert!(
            r.x >= 0.0 && r.y >= 0.0,
            "damage origin must be clamped: {r:?}"
        );
        assert!(
            r.x + r.width <= view.width as f32 + 0.5 && r.y + r.height <= view.height as f32 + 0.5,
            "damage extent must be clamped to surface: {r:?}"
        );
    }
}
