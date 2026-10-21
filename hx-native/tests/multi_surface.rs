//! Issue #8 step 2: one document + buffer per HTML window.

use hx_native::*;

fn make_eng(budget: u64) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
        hidden_surface_budget_bytes: budget,
        fonts: Vec::new(),
    });
    e.declare_actions(&["act.do", "time.set_speed"]);
    e
}

fn view(w: u32, h: u32) -> Viewport {
    Viewport::with_scale(w, h, 1.0)
}

fn simple_html(title: &str, css_w: u32, css_h: u32) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: {css_w}px; height: {css_h}px; background: #2a3140; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; }}
h1 {{ font-size: 16px; padding: 10px; }}
</style></head><body>
<h1 id="title">{title}</h1>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##
    )
}

fn action_html(css_w: u32, css_h: u32) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: {css_w}px; height: {css_h}px; background: #2a3140; }}
#act {{ position: absolute; left: 8px; top: 8px; width: 120px; height: 40px; }}
</style></head><body>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##
    )
}

fn click_act(eng: &mut Engine, id: SurfaceId) -> RequestId {
    let _ = eng.input_surface(
        id,
        InputEvent::PointerMove {
            x: 40.0,
            y: 24.0,
            mods: Modifiers::default(),
        },
    );
    let _ = eng.input_surface(
        id,
        InputEvent::PointerDown {
            x: 40.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
    );
    match eng.input_surface(
        id,
        InputEvent::PointerUp {
            x: 40.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
    ) {
        InputOutcome::Action { request, .. } => request,
        other => panic!("expected Action on {id}, got {other:?}"),
    }
}

struct EmptySlices;
impl SliceSource for EmptySlices {
    fn version(&self, _name: &str) -> Option<u64> {
        None
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
}

struct EmptyFrags;
impl FragmentSource for EmptyFrags {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        format!("<!-- {section_id} -->")
    }
}

/// Eviction drops the document and buffer at once (estimated residual 0 B).
#[test]
fn eviction_frees_estimated_bytes_at_once() {
    // Mid window ~640x480 ≈ 1.17 MiB RGBA + ~1 MiB doc. Budget fits one hidden.
    let budget = estimate_surface_engine_bytes(640, 480, &simple_html("a", 640, 480)) + 1024;
    let mut eng = make_eng(budget);
    let a = SurfaceId(1);
    let b = SurfaceId(2);
    eng.open_surface(
        a,
        &simple_html("a", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    assert!(eng.surface_last_frame(a).is_some());
    assert!(eng.surface_info(a).unwrap().estimated_bytes > 0);

    eng.open_surface(
        b,
        &simple_html("b", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();

    // Opening B should have evicted A (LRU).
    let info_a = eng.surface_info(a).unwrap();
    assert!(info_a.evicted, "A should be evicted");
    assert_eq!(info_a.estimated_bytes, 0, "evicted residual must be 0 B");
    assert!(eng.surface_last_frame(a).is_none());
    assert!(eng.surface_memory_stats().eviction_count >= 1);
}

/// Pinned surfaces survive memory pressure.
#[test]
fn pinned_survives_memory_pressure() {
    let html = simple_html("pin", 640, 480);
    let one = estimate_surface_engine_bytes(640, 480, &html);
    let mut eng = make_eng(one + 1024);
    let pinned = SurfaceId(10);
    let filler = SurfaceId(11);
    eng.open_surface(
        pinned,
        &html,
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: true,
        },
    )
    .unwrap();
    eng.open_surface(
        filler,
        &simple_html("fill", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    // Force more pressure with another unpinned hidden.
    eng.open_surface(
        SurfaceId(12),
        &simple_html("more", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();

    let info = eng.surface_info(pinned).unwrap();
    assert!(!info.evicted, "pinned must survive");
    assert!(info.estimated_bytes > 0);
    assert!(eng.pinned_bytes() > 0);
}

/// Reopen after eviction is byte-identical to a fresh open.
#[test]
fn reopen_after_eviction_byte_identical() {
    let html = simple_html("twin", 320, 240);
    let one = estimate_surface_engine_bytes(320, 240, &html);
    let mut eng = make_eng(one + 512);

    let id = SurfaceId(1);
    eng.open_surface(
        id,
        &html,
        view(320, 240),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    // open_surface(visible:false) already ran the first raster; skipped paint is fine.
    let _ = eng.paint_surface(id).unwrap();
    let pixels_before = eng
        .surface_last_frame(id)
        .expect("pre-drawn buffer")
        .pixels
        .clone();

    // Evict by opening a second hidden surface over budget.
    eng.open_surface(
        SurfaceId(2),
        &simple_html("other", 320, 240),
        view(320, 240),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    assert!(eng.surface_info(id).unwrap().evicted);

    eng.set_visible(id, true).unwrap();
    let slices = EmptySlices;
    let mut frags = EmptyFrags;
    eng.sync_surface(id, &slices, &mut frags).unwrap();
    let rebuilt = eng.paint_surface(id).unwrap();
    let rebuilt_px = rebuilt.rgba.as_ref().unwrap().pixels.clone();

    // Fresh open of the same HTML on another engine (visible + paint).
    let mut fresh = make_eng(DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);
    fresh
        .open_surface(
            SurfaceId(99),
            &html,
            view(320, 240),
            SurfaceOpts {
                visible: true,
                pinned: false,
            },
        )
        .unwrap();
    fresh
        .sync_surface(SurfaceId(99), &slices, &mut frags)
        .unwrap();
    let fresh_out = fresh.paint_surface(SurfaceId(99)).unwrap();
    let fresh_px = fresh_out.rgba.as_ref().unwrap().pixels.clone();

    assert_eq!(
        rebuilt_px, fresh_px,
        "rebuilt-after-eviction must match fresh open"
    );
    // Pre-eviction pre-drawn buffer and fresh open should also match for static HTML.
    assert_eq!(pixels_before, fresh_px);
}

/// dirty_surfaces skips hidden, unpinned surfaces.
#[test]
fn dirty_surfaces_skips_hidden_unpinned() {
    let mut eng = make_eng(DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);
    let hidden = SurfaceId(1);
    let pinned_hidden = SurfaceId(2);
    let visible = SurfaceId(3);

    eng.open_surface(
        hidden,
        &simple_html("h", 200, 100),
        view(200, 100),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    eng.open_surface(
        pinned_hidden,
        &simple_html("ph", 200, 100),
        view(200, 100),
        SurfaceOpts {
            visible: false,
            pinned: true,
        },
    )
    .unwrap();
    eng.open_surface(
        visible,
        &simple_html("v", 200, 100),
        view(200, 100),
        SurfaceOpts {
            visible: true,
            pinned: false,
        },
    )
    .unwrap();

    // Mark all dirty via theme (forces full on next paint) — use sync empty is fine;
    // open already leaves visible + pinned_hidden needing upload.
    let dirty = eng.dirty_surfaces();
    assert!(
        !dirty.contains(&hidden),
        "hidden unpinned must never be reported: {dirty:?}"
    );
    assert!(
        dirty.contains(&visible) || dirty.contains(&pinned_hidden),
        "visible or pinned-hidden should be reportable: {dirty:?}"
    );
    assert!(!dirty.contains(&hidden));
}

/// Damage rects are in surface-local device coordinates.
#[test]
fn damage_is_surface_local() {
    let mut eng = make_eng(DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);
    let a = SurfaceId(1);
    let b = SurfaceId(2);
    eng.open_surface(
        a,
        &simple_html("a", 100, 80),
        view(100, 80),
        SurfaceOpts {
            visible: true,
            pinned: false,
        },
    )
    .unwrap();
    eng.open_surface(
        b,
        &simple_html("b", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: true,
            pinned: false,
        },
    )
    .unwrap();

    let out_a = eng.paint_surface(a).unwrap();
    let out_b = eng.paint_surface(b).unwrap();
    for r in &out_a.damage {
        assert!(r.x >= 0.0 && r.y >= 0.0);
        assert!(r.x + r.width <= 100.0 + 0.5);
        assert!(r.y + r.height <= 80.0 + 0.5);
    }
    for r in &out_b.damage {
        assert!(r.x >= 0.0 && r.y >= 0.0);
        assert!(r.x + r.width <= 640.0 + 0.5);
        assert!(r.y + r.height <= 480.0 + 0.5);
    }
    assert_eq!(out_a.rgba.as_ref().unwrap().width, 100);
    assert_eq!(out_a.rgba.as_ref().unwrap().height, 80);
    assert_eq!(out_b.rgba.as_ref().unwrap().width, 640);
    assert_eq!(out_b.rgba.as_ref().unwrap().height, 480);
}

/// First paint after set_visible(true) returns full-surface damage (host re-upload).
#[test]
fn full_damage_after_show() {
    let mut eng = make_eng(DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);
    let id = SurfaceId(1);
    eng.open_surface(
        id,
        &simple_html("win", 200, 150),
        view(200, 150),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    // Pre-drawn while hidden; paint while still hidden keeps buffer.
    let hidden_paint = eng.paint_surface(id).unwrap();
    assert!(
        !hidden_paint.skipped
            || hidden_paint.rgba.is_some()
            || eng.surface_last_frame(id).is_some()
    );

    eng.set_visible(id, true).unwrap();
    let shown = eng.paint_surface(id).unwrap();
    assert!(!shown.skipped);
    assert!(shown.rgba.is_some());
    assert_eq!(shown.damage.len(), 1);
    let d = &shown.damage[0];
    assert_eq!(d.x, 0.0);
    assert_eq!(d.y, 0.0);
    assert_eq!(d.width, 200.0);
    assert_eq!(d.height, 150.0);
    assert!(eng.last_paint_stats().full_host_upload);

    // Second paint with nothing dirty is skipped (no full upload again).
    let clean = eng.paint_surface(id).unwrap();
    assert!(clean.skipped);
    assert!(clean.damage.is_empty());
    assert!(!eng.last_paint_stats().full_host_upload);
}

/// Never evict a surface that still has an action waiting for complete().
#[test]
fn pending_action_blocks_eviction() {
    let html = action_html(640, 480);
    let one = estimate_surface_engine_bytes(640, 480, &html);
    let mut eng = make_eng(one + 1024);
    let pending_id = SurfaceId(1);
    eng.open_surface(
        pending_id,
        &html,
        view(640, 480),
        SurfaceOpts {
            visible: true,
            pinned: false,
        },
    )
    .unwrap();
    eng.paint_surface(pending_id).unwrap();
    let request = click_act(&mut eng, pending_id);

    // Hide so it becomes evictable by budget rules — except pending blocks it.
    eng.set_visible(pending_id, false).unwrap();
    assert!(
        !eng.surface_info(pending_id).unwrap().evicted,
        "pending surface must not evict on hide alone under budget"
    );

    // Open another large hidden surface to push over budget.
    eng.open_surface(
        SurfaceId(2),
        &simple_html("pressure", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    eng.open_surface(
        SurfaceId(3),
        &simple_html("pressure2", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();

    assert!(
        !eng.surface_info(pending_id).unwrap().evicted,
        "surface with pending Action must not be evicted"
    );

    eng.complete(request, Ok(()));
    // After complete, pressure from another open should allow eviction.
    eng.open_surface(
        SurfaceId(4),
        &simple_html("pressure3", 640, 480),
        view(640, 480),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    assert!(
        eng.surface_info(pending_id).unwrap().evicted
            || eng.surface_memory_stats().eviction_count >= 1,
        "after complete, pending surface may be evicted under pressure"
    );
}

/// Full-damage-after-show also holds after rebuild from eviction.
#[test]
fn full_damage_after_rebuild_from_eviction() {
    let html = simple_html("r", 320, 200);
    let one = estimate_surface_engine_bytes(320, 200, &html);
    let mut eng = make_eng(one + 256);
    let id = SurfaceId(1);
    eng.open_surface(
        id,
        &html,
        view(320, 200),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    eng.open_surface(
        SurfaceId(2),
        &simple_html("x", 320, 200),
        view(320, 200),
        SurfaceOpts {
            visible: false,
            pinned: false,
        },
    )
    .unwrap();
    assert!(eng.surface_info(id).unwrap().evicted);

    eng.set_visible(id, true).unwrap();
    let out = eng.paint_surface(id).unwrap();
    assert_eq!(out.damage.len(), 1);
    assert_eq!(out.damage[0].width, 320.0);
    assert_eq!(out.damage[0].height, 200.0);
    assert!(eng.last_paint_stats().full_host_upload);
}

/// close_surface drops buffer immediately.
#[test]
fn close_surface_drops_buffer() {
    let mut eng = make_eng(DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);
    let id = SurfaceId(1);
    eng.open_surface(
        id,
        &simple_html("c", 100, 100),
        view(100, 100),
        SurfaceOpts::default(),
    )
    .unwrap();
    eng.paint_surface(id).unwrap();
    assert!(eng.surface_last_frame(id).is_some());
    eng.close_surface(id).unwrap();
    assert!(eng.surface_info(id).is_none());
    assert!(eng.surface_last_frame(id).is_none());
}

/// Legacy single-document API still works on the default surface.
#[test]
fn legacy_default_surface_api() {
    let mut eng = make_eng(DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);
    eng.load_html(&simple_html("legacy", 400, 120)).unwrap();
    let slices = EmptySlices;
    let mut frags = EmptyFrags;
    eng.sync(&slices, &mut frags).unwrap();
    let out = eng.paint(Viewport::with_scale(400, 120, 1.0)).unwrap();
    assert!(!out.skipped);
    assert!(eng.last_frame().is_some());
}

/// RequestIds stay deterministic (monotonic) across surfaces.
#[test]
fn request_ids_deterministic_across_surfaces() {
    let mut eng = make_eng(DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);
    let a = SurfaceId(1);
    let b = SurfaceId(2);
    for id in [a, b] {
        eng.open_surface(
            id,
            &action_html(200, 120),
            view(200, 120),
            SurfaceOpts::default(),
        )
        .unwrap();
        eng.paint_surface(id).unwrap();
    }

    let r1 = click_act(&mut eng, a);
    let r2 = click_act(&mut eng, b);
    assert_eq!(r1.0, 1);
    assert_eq!(r2.0, 2);
}
