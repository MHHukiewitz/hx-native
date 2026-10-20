//! Issue #20: sync_surface swap damage must match a forced full repaint.
//!
//! Repro shape (Mainnet Pause exit confirm): an inner section swaps from a
//! compact row to a larger question+buttons block at a different size. Partial
//! damage must be byte-identical to a full repaint at scale 1.0, 2.0, and 1.25.
//! A leftover pending full-surface rect must force full_repaint.

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

/// Pause-like panel: fixed chrome, inner `#pause-exit` swapped by slice:pause.
/// Old fragment is a single row at the top; new fragment is a question + two
/// buttons that sit lower and wider — old and new boxes differ in size and
/// position (like Mainnet exit confirm).
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
  position: absolute; left: 480px; top: 236px; width: 320px; height: 328px;
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

fn paint_swap_partial(scale: f64) -> (PaintOutput, PaintStats) {
    let mut eng = boot(&pause_page());
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("pause-exit", &exit_row_html());
    eng.sync(&slices, &mut frags).unwrap();
    let view = vp_css(1280, 800, scale);
    let _ = eng.paint(view.clone()).unwrap();

    slices.bump("pause");
    frags.set("pause-exit", &exit_confirm_html());
    eng.sync(&slices, &mut frags).unwrap();
    let out = eng.paint(view).unwrap();
    let stats = eng.last_paint_stats();
    (out, stats)
}

fn paint_swap_forced_full(scale: f64) -> PaintOutput {
    let mut eng = boot(&pause_page());
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("pause-exit", &exit_confirm_html());
    slices.bump("pause");
    eng.sync(&slices, &mut frags).unwrap();
    eng.mark_theme_or_stylesheet_changed();
    let view = vp_css(1280, 800, scale);
    eng.paint(view).unwrap()
}

fn assert_partial_matches_full(scale: f64) {
    let (partial, stats) = paint_swap_partial(scale);
    let full = paint_swap_forced_full(scale);

    assert!(
        !partial.skipped,
        "swap paint must not skip at scale={scale}"
    );
    assert!(
        partial.rgba.is_some(),
        "partial must return rgba at scale={scale}"
    );
    eprintln!(
        "scale={scale} full_repaint={} damage={:?} stats={:?}",
        stats.full_repaint, partial.damage, stats
    );

    let a = partial.rgba.as_ref().unwrap();
    let b = full.rgba.as_ref().unwrap();
    if a.pixels != b.pixels {
        let mut min_x = a.width;
        let mut min_y = a.height;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        let mut diffs = 0u32;
        for y in 0..a.height {
            for x in 0..a.width {
                let i = ((y * a.width + x) * 4) as usize;
                if a.pixels[i..i + 4] != b.pixels[i..i + 4] {
                    diffs += 1;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        panic!(
            "swap partial must be byte-identical to forced full at scale={scale}; \
             diffs={diffs} mismatch_bbox=({min_x},{min_y})-({max_x},{max_y}) \
             mismatch_size={}x{} damage={:?}",
            max_x.saturating_sub(min_x) + 1,
            max_y.saturating_sub(min_y) + 1,
            partial.damage
        );
    }
}

#[test]
fn swap_inner_section_partial_matches_full_scale_1() {
    assert_partial_matches_full(1.0);
}

#[test]
fn swap_inner_section_partial_matches_full_scale_2() {
    assert_partial_matches_full(2.0);
}

#[test]
fn swap_inner_section_partial_matches_full_scale_1_25() {
    assert_partial_matches_full(1.25);
}

/// A leftover pending full-surface rect (e.g. from a fold) must force
/// `full_repaint=true` on the next plan — never dropped in favour of a local rect.
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
    assert_eq!(out.damage.len(), 1, "expected one full-surface damage rect");
    assert_eq!(out.damage[0].x, 0.0);
    assert_eq!(out.damage[0].y, 0.0);
    assert_eq!(out.damage[0].width, view.width as f32);
    assert_eq!(out.damage[0].height, view.height as f32);
}

/// Same-size colour swap: NodeId reuse can hide content-only changes if damage
/// only looks at own-box geometry.
#[test]
fn swap_same_box_recolour_partial_matches_full_scale_2() {
    let html = r##"<!DOCTYPE html>
<html><head><meta charset="utf-8"/>
<style>
* { margin:0; padding:0; box-sizing:border-box; }
body { width:400px; height:200px; background:#111; }
#sec { position:absolute; left:40px; top:40px; width:120px; height:60px; }
#cell { width:120px; height:60px; }
</style></head><body>
<section id="sec" hx-trigger="slice:pause" hx-target="#sec" hx-swap="innerHTML">
  <div id="cell" style="background:#c00"></div>
</section>
</body></html>"##;
    let mut eng = boot(html);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("sec", r#"<div id="cell" style="background:#c00"></div>"#);
    eng.sync(&slices, &mut frags).unwrap();
    let view = vp_css(400, 200, 2.0);
    let _ = eng.paint(view.clone()).unwrap();

    slices.bump("pause");
    frags.set("sec", r#"<div id="cell" style="background:#0c0"></div>"#);
    eng.sync(&slices, &mut frags).unwrap();
    let partial = eng.paint(view.clone()).unwrap();

    let mut eng_f = boot(html);
    let mut s_f = MemSlices::new();
    let mut f_f = SpyFrags::new();
    f_f.set("sec", r#"<div id="cell" style="background:#0c0"></div>"#);
    s_f.bump("pause");
    eng_f.sync(&s_f, &mut f_f).unwrap();
    eng_f.mark_theme_or_stylesheet_changed();
    let full = eng_f.paint(view).unwrap();

    eprintln!(
        "recolour damage={:?} full_repaint={}",
        partial.damage,
        eng.last_paint_stats().full_repaint
    );
    assert_eq!(
        partial.rgba.as_ref().unwrap().pixels,
        full.rgba.as_ref().unwrap().pixels,
        "same-box recolour must be byte-identical; damage={:?}",
        partial.damage
    );
}
