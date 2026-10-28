//! Issue #125: hard inset `box-shadow` layers (no blur, no radius, pure
//! translation) paint in CSS order (first shadow on top) with their offset
//! in device px, as Chrome does. Blitz paints them in list order with the
//! offset in CSS px, so Win95 bevels came out with the wrong colours on top
//! and half as thick at scale 2.
//!
//! Expectations are Chrome's own pixels (`fixtures/bevel125_chrome.txt`);
//! typed fields and soft layers (blur, radius, rotation) must keep the
//! pixels they had before, and partial repaint must equal full repaint.

mod chrome_profiles;

use chrome_profiles as cp;
use hx_native::*;

const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

/// Raised panel, sunken box, button and window frame (Win95 kit bevels).
const PAGE: &str = include_str!("fixtures/bevel125.html");
const PAGE_W: f64 = 200.0;
const PAGE_H: f64 = 120.0;

fn engine(mode: PaintMode) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 10_000_000_000,
        paint_mode: mode,
        ..EngineConfig::default()
    });
    e.declare_actions(&[]);
    e
}

fn viewport(w: f64, h: f64, scale: f64) -> Viewport {
    Viewport::with_scale((w * scale) as u32, (h * scale) as u32, scale)
}

/// Full paint with the inset adapter on (`fast`) or off (Blitz's layers).
fn paint(html: &str, w: f64, h: f64, scale: f64, fast: bool) -> Vec<u8> {
    set_inset_shadow_fast_path_for_test(fast);
    let mut e = engine(PaintMode::Exact);
    e.load_html(html).unwrap();
    let out = e.paint(viewport(w, h, scale)).unwrap();
    set_inset_shadow_fast_path_for_test(true);
    out.rgba.unwrap().pixels
}

fn first_diff(a: &[u8], b: &[u8], w: usize) -> Option<(usize, usize, [u8; 4], [u8; 4])> {
    a.chunks(4)
        .zip(b.chunks(4))
        .enumerate()
        .find_map(|(i, (p, q))| {
            (p != q).then(|| {
                (
                    i % w,
                    i / w,
                    [p[0], p[1], p[2], p[3]],
                    [q[0], q[1], q[2], q[3]],
                )
            })
        })
}

#[test]
fn win95_bevels_match_chrome_at_every_scale() {
    let mut bad = Vec::new();
    for scale in SCALES {
        assert_eq!(cp::profiles("bevel125", scale).len(), 4 * 8, "fixture");
        let _ = take_inset_shadow_counters_for_test();
        let frame = paint(PAGE, PAGE_W, PAGE_H, scale, true);
        let c = take_inset_shadow_counters_for_test();
        assert_eq!((c.hard, c.fast, c.replayed), (16, 0, 0), "scale {scale}");
        bad.extend(cp::mismatches(
            "bevel125",
            scale,
            &[],
            &frame,
            (PAGE_W * scale) as usize,
        ));
    }
    assert!(bad.is_empty(), "off Chrome:\n{}", bad.join("\n"));
}

/// The bevel width along every edge equals Chrome's: 2, 3, 3 and 4 device
/// px at 1, 1.25, 1.5 and 2 (it was 2, 2, 2 and 2). Prints the table.
#[test]
fn bevel_edge_widths_match_chrome() {
    let mut bad = Vec::new();
    for scale in SCALES {
        let frame = paint(PAGE, PAGE_W, PAGE_H, scale, true);
        for p in cp::profiles("bevel125", scale) {
            if !["top", "bottom", "left", "right"].contains(&p.edge) {
                continue;
            }
            let hx = cp::width(&cp::sample(&p, &frame, (PAGE_W * scale) as usize));
            let chrome = cp::width(&p.colours);
            eprintln!("{} {} @{scale}: hx {hx} chrome {chrome}", p.name, p.edge);
            if hx != chrome || chrome != (2.0 * scale).ceil() as usize {
                bad.push(format!(
                    "{} {} @{scale}: hx {hx} chrome {chrome}",
                    p.name, p.edge
                ));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Typed fields keep the pixels they had: their hard bevels are left to the
/// field replay (`fields.rs`) and Blitz's layers, never painted twice. (A
/// `<textarea>` is not a typed field there, so its bevel now takes the #125
/// path like any other box.)
#[test]
fn typed_field_bevels_are_unchanged() {
    const FIELDS: &str = r#"<!DOCTYPE html><html><head><style>
html,body{margin:0;background:#c0c0c0}
.f{position:absolute;box-sizing:border-box;border:none;background:#fff;padding:2px 3px;font:11px "DejaVu Sans",sans-serif;
box-shadow:inset -1px -1px #fff,inset 1px 1px #808080,inset -2px -2px #dfdfdf,inset 2px 2px #0a0a0a}
.box{position:absolute;left:4px;top:60px;width:150px;height:40px;overflow:auto}
</style></head><body>
<input class="f" type="text" value="abc" style="left:8px;top:8px;width:96px;height:20px">
<input class="f" type="number" value="12" style="left:112px;top:8.5px;width:61px;height:21px">
<div class="box"><div style="height:30px"></div><input class="f" type="text" style="position:relative;left:2px;width:100px;height:20px"></div>
</body></html>"#;
    for scale in SCALES {
        let _ = take_inset_shadow_counters_for_test();
        let now = paint(FIELDS, 180.0, 110.0, scale, true);
        let c = take_inset_shadow_counters_for_test();
        let before = paint(FIELDS, 180.0, 110.0, scale, false);
        assert_eq!(
            first_diff(&now, &before, (180.0 * scale) as usize),
            None,
            "scale {scale}: (x, y, now, before)"
        );
        assert_eq!(
            c.hard, 0,
            "scale {scale}: a field bevel took the #125 path: {c:?}"
        );
        assert_eq!(c.fast + c.replayed, 12, "scale {scale}: {c:?}");
    }
}

/// Layers that are not hard (blur, corner radius, rotation, a run mixing
/// hard and blurred layers) keep Blitz's pixels.
#[test]
fn soft_layers_are_unchanged() {
    const SOFT: &str = r#"<!DOCTYPE html><html><head><style>
html,body{margin:0;background:#008080}
.b{position:absolute;box-sizing:border-box;background:#c0c0c0;width:40px;height:30px}
</style></head><body>
<div class="b" style="left:4px;top:4px;box-shadow:inset 0 0 4px #000"></div>
<div class="b" style="left:52px;top:4px;border-radius:6px;box-shadow:inset 1px 1px #fff,inset -1px -1px #808080"></div>
<div class="b" style="left:100px;top:4px;transform:rotate(3deg);box-shadow:inset 1px 1px #fff,inset -1px -1px #808080"></div>
<div class="b" style="left:4px;top:44px;box-shadow:inset 1px 1px #fff,inset 0 0 4px #000,inset -1px -1px #808080"></div>
<div class="b" style="left:52px;top:44px;box-shadow:inset 2px 2px 3px rgba(0,0,0,.5),inset -1px -1px #fff"></div>
<div class="b" style="left:100px;top:44px;border-radius:50%;box-shadow:inset 0 0 0 3px #00f"></div>
</body></html>"#;
    for scale in SCALES {
        let _ = take_inset_shadow_counters_for_test();
        let now = paint(SOFT, 150.0, 80.0, scale, true);
        let c = take_inset_shadow_counters_for_test();
        let before = paint(SOFT, 150.0, 80.0, scale, false);
        assert_eq!(
            first_diff(&now, &before, (150.0 * scale) as usize),
            None,
            "scale {scale}: (x, y, now, before)"
        );
        assert_eq!(c.hard, 0, "scale {scale}: {c:?}");
        assert_eq!(c.fast + c.replayed, 11, "scale {scale}: {c:?}");
    }
}

#[test]
fn partial_repaint_equals_full_repaint() {
    // Exact mode, as #123; Shifted may differ by 1/255 by design.
    let mode = PaintMode::Exact;
    {
        for scale in [1.0, 2.0] {
            let vp = viewport(PAGE_W, PAGE_H, scale);
            let mut e = engine(mode);
            e.load_html(PAGE).unwrap();
            let full = e.paint(vp.clone()).unwrap().rgba.unwrap().pixels;
            // Rects that cut the bevels across edges and corners.
            for (x, y, rw, rh) in [
                (6.0, 6.0, 5.0, 5.0),
                (60.0, 20.0, 20.0, 30.0),
                (130.0, 30.0, 70.0, 3.0),
                (6.0, 100.0, 125.0, 9.0),
                (126.0, 50.0, 3.0, 60.0),
            ] {
                let s = scale as f32;
                e.test_push_pending_damage(Rect {
                    x: x * s + 1.0,
                    y: y * s + 1.0,
                    width: rw * s,
                    height: rh * s,
                });
                let out = e.paint(vp.clone()).unwrap();
                assert!(!out.skipped);
                assert!(!e.last_paint_stats().full_repaint, "must be a partial");
                let part = out.rgba.unwrap().pixels;
                assert_eq!(
                    first_diff(&part, &full, (PAGE_W * scale) as usize),
                    None,
                    "{mode:?} scale {scale} rect {x},{y} {rw}x{rh}"
                );
            }
        }
    }
}
