//! Issue #123: a zero-blur inset `box-shadow` layer is painted as one
//! even-odd fill instead of Blitz's two layers around a per-pixel
//! blurred-rect hole. Since #125 hard layers (no blur, no radius, pure
//! translation) follow Chrome (CSS order, offset in device px), so they are
//! checked against Chrome's pixels (bevels, spread, offsets past the box,
//! translucent colours, borders, opacity); layers that are not hard must stay
//! pixel-identical to Blitz's layers, and partial repaint must equal full
//! repaint. The wall-clock check is `#[ignore]`d; see its doc comment.

mod chrome_profiles;

use std::time::Instant;

use chrome_profiles as cp;

use hx_native::*;

const W: f64 = 320.0;
const H: f64 = 120.0;

/// Win95 kit bevels plus every inset shape the adapter has to get right or
/// leave alone (`fixtures/inset123.html`).
const PAGE: &str = include_str!("fixtures/inset123.html");

fn engine() -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 10_000_000_000,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    e.declare_actions(&[]);
    e
}

fn viewport(scale: f64) -> Viewport {
    Viewport::with_scale((W * scale) as u32, (H * scale) as u32, scale)
}

fn full_paint(html: &str, scale: f64, fast: bool) -> Vec<u8> {
    set_inset_shadow_fast_path_for_test(fast);
    let mut e = engine();
    e.load_html(html).unwrap();
    let out = e.paint(viewport(scale)).unwrap();
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

/// Hard layers on whole device pixels match Chrome (every box Chrome does
/// not snap at that scale: `fixtures/bevel125_chrome.txt`). At 1.25 no box
/// of this page lands on whole device pixels, so there is nothing to check.
/// `fade` is left out: its `opacity` wrapper has no size and Blitz paints
/// nothing of it at all (box, background or shadow), which is not an inset
/// shadow matter.
#[test]
fn hard_layers_match_chrome() {
    let mut bad = Vec::new();
    for scale in [1.0, 1.5, 2.0] {
        assert!(!cp::profiles("inset123", scale).is_empty(), "fixture");
        let fast = full_paint(PAGE, scale, true);
        bad.extend(cp::mismatches(
            "inset123",
            scale,
            &[
                "bevel", "button", "field", "spread", "far", "half", "inside",
            ],
            &fast,
            (W * scale) as usize,
        ));
    }
    assert!(bad.is_empty(), "off Chrome:\n{}", bad.join("\n"));
}

/// Device rect (x, y, w, h) of a CSS box, grown by `pad` CSS px.
fn device_rect(b: (f64, f64, f64, f64), pad: f64, scale: f64) -> (usize, usize, usize, usize) {
    let x0 = ((b.0 - pad) * scale).floor() as usize;
    let y0 = ((b.1 - pad) * scale).floor() as usize;
    let x1 = ((b.0 + b.2 + pad) * scale).ceil() as usize;
    let y1 = ((b.1 + b.3 + pad) * scale).ceil() as usize;
    (x0, y0, x1 - x0, y1 - y0)
}

/// Layers that are not hard (radius, blur, rotation) and the transparent
/// first layer Blitz stops at stay pixel-identical to Blitz's layers.
#[test]
fn soft_layers_are_pixel_identical_to_blitz_layers() {
    // round, blur, clear, tilt (grown 2 CSS px for the rotation).
    let soft = [
        ((164.0, 50.0, 40.0, 30.0), 0.0),
        ((212.0, 50.0, 40.0, 30.0), 0.0),
        ((260.0, 50.0, 40.0, 30.0), 0.0),
        ((90.0, 88.0, 40.0, 24.0), 2.0),
    ];
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let _ = take_inset_shadow_counters_for_test();
        let fast = full_paint(PAGE, scale, true);
        let c = take_inset_shadow_counters_for_test();
        let layers = full_paint(PAGE, scale, false);
        let w = (W * scale) as usize;
        for (b, pad) in soft {
            let (x, y, rw, rh) = device_rect(b, pad, scale);
            for row in y..y + rh {
                let r = (row * w + x) * 4..(row * w + x + rw) * 4;
                assert_eq!(
                    first_diff(&fast[r.clone()], &layers[r], rw),
                    None,
                    "scale {scale} box {b:?} row {row}: (dx, _, fast, layers)"
                );
            }
        }
        assert!(c.hard > 0 && c.replayed > 0, "scale {scale}: {c:?}");
    }
}

/// Layers per class at scale 1, as (hard, fast, replayed): hard layers take
/// the #125 CSS-order path, the rest replay (or, in a run mixed with a
/// blurred layer, keep #123's one-fill path).
#[test]
fn hard_layers_take_the_css_order_path_and_the_rest_replay() {
    let count = |class: &str, style: &str| {
        let html = PAGE.split("</style>").next().unwrap().to_owned()
            + &format!(
                r#"</style></head><body><div class="b {class}" style="{style}"></div></body></html>"#
            );
        let _ = take_inset_shadow_counters_for_test();
        let _ = full_paint(&html, 1.0, true);
        let c = take_inset_shadow_counters_for_test();
        (c.hard, c.fast, c.replayed)
    };
    let at = "left:10px;top:10px;width:60px;height:30px";
    assert_eq!(count("bevel", at), (2, 0, 0));
    assert_eq!(count("button", at), (4, 0, 0));
    assert_eq!(count("field", at), (2, 0, 0));
    assert_eq!(count("spread", at), (1, 0, 0));
    assert_eq!(count("far", at), (2, 0, 0));
    assert_eq!(count("half", at), (2, 0, 0));
    assert_eq!(count("inside", at), (1, 0, 0));
    assert_eq!(count("round", at), (0, 0, 2), "a corner radius replays");
    assert_eq!(count("blur", at), (0, 0, 1), "a blur replays");
    // Blitz stops at a transparent layer (the black one is never drawn).
    assert_eq!(count("clear", at), (0, 0, 0));
    assert_eq!(
        count("bevel nudge", at),
        (2, 0, 0),
        "a box off whole pixels is still hard (anti-aliased like Chrome)"
    );
    assert_eq!(count("bevel tilt", at), (0, 0, 2), "a rotated box replays");
    let mixed = format!("{at};box-shadow:inset 1px 1px #fff,inset 0 0 4px #000");
    assert_eq!(
        count("", &mixed),
        (0, 1, 1),
        "a run with a blurred layer keeps Blitz's order"
    );
}

#[test]
fn partial_repaint_equals_full_repaint() {
    for scale in [1.0, 2.0] {
        let vp = viewport(scale);
        let mut e = engine();
        e.load_html(PAGE).unwrap();
        let full = e.paint(vp.clone()).unwrap().rgba.unwrap().pixels;
        // Rects that cut bevels, the spread ring and the far offsets.
        for (x, y, rw, rh) in [
            (0.0, 0.0, 7.0, 5.0),
            (100.0, 30.0, 40.0, 25.0),
            (150.0, 10.0, 70.0, 6.0),
            (42.0, 60.0, 5.0, 30.0),
            (60.0, 70.0, 60.0, 30.0),
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
                first_diff(&part, &full, (W * scale) as usize),
                None,
                "scale {scale} rect {x},{y} {rw}x{rh}"
            );
        }
    }
}

/// A Market-sized panel: four big bevelled boxes over 1280×656.
fn panel_page(shadow: &str) -> String {
    let boxes: String = [
        (8, 8, 620, 640),
        (636, 8, 636, 320),
        (636, 336, 636, 312),
        (16, 40, 604, 600),
    ]
    .iter()
    .map(|(x, y, w, h)| {
        format!(r#"<div class="b" style="left:{x}px;top:{y}px;width:{w}px;height:{h}px"></div>"#)
    })
    .collect();
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#008080}}.b{{position:absolute;box-sizing:border-box;background:#c0c0c0;box-shadow:{shadow}}}</style></head><body>{boxes}</body></html>"#
    )
}

fn best_full_paint_ns(html: &str, fast: bool, iters: usize) -> u64 {
    set_inset_shadow_fast_path_for_test(fast);
    let mut e = engine();
    e.load_html(html).unwrap();
    let best = (0..iters)
        .map(|n| {
            // A new size every time, so the engine repaints everything.
            let vp = Viewport::with_scale(1280 + n as u32, 656, 1.0);
            let t = Instant::now();
            let out = e.paint(vp).unwrap();
            assert!(!out.skipped);
            t.elapsed().as_nanos() as u64
        })
        .min()
        .unwrap();
    set_inset_shadow_fast_path_for_test(true);
    best
}

/// Load-independent: every bevel layer of the panel reaches the renderer as
/// one fill; none goes through the blurred-rect hole.
#[test]
fn a_bevelled_panel_paints_every_layer_as_one_fill() {
    let html = panel_page("inset 1px 1px #fff,inset -1px -1px #808080");
    let _ = take_inset_shadow_counters_for_test();
    let _ = best_full_paint_ns(&html, true, 1);
    let c = take_inset_shadow_counters_for_test();
    assert_eq!((c.hard, c.fast, c.replayed), (8, 0, 0), "{c:?}");
}

/// Wall-clock check: a full repaint of the bevelled panel costs at most twice
/// the same panel without shadows, and the layer path really is the slow one.
///
/// Ignored by default like the #67 timing check (wall clock flakes under
/// load); the counts in `a_bevelled_panel_paints_every_layer_as_one_fill`
/// guard the fast path itself:
///
/// ```text
/// cargo test --release -p hx-native --test inset_shadow_123 -- --ignored --nocapture
/// ```
#[test]
#[ignore = "wall-clock perf check, flaky under load; run with `cargo test -- --ignored`"]
fn a_bevelled_panel_costs_at_most_twice_a_plain_one() {
    let bevel = panel_page("inset 1px 1px #fff,inset -1px -1px #808080");
    let plain = panel_page("none");
    let fast = best_full_paint_ns(&bevel, true, 5);
    let none = best_full_paint_ns(&plain, true, 5);
    let layers = best_full_paint_ns(&bevel, false, 3);
    let ms = |ns: u64| ns as f64 / 1e6;
    eprintln!(
        "1280x656 panel, full repaint: fast {:.2} ms, no shadow {:.2} ms, layers {:.2} ms",
        ms(fast),
        ms(none),
        ms(layers)
    );
    assert!(
        fast <= 2 * none,
        "fast {:.2} ms vs none {:.2} ms",
        ms(fast),
        ms(none)
    );
    assert!(
        layers > 2 * fast,
        "layers {:.2} ms vs fast {:.2} ms",
        ms(layers),
        ms(fast)
    );
}
