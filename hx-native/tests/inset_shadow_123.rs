//! Issue #123: a zero-blur inset `box-shadow` layer on whole device pixels
//! is painted as one even-odd fill instead of Blitz's two layers around a
//! per-pixel blurred-rect hole. The fast path must be pixel-identical to
//! Blitz's layers (bevels, spread, offsets past the box, translucent colours,
//! borders, opacity; scale 1, 1.5 and 2), shapes it cannot draw exactly must
//! fall back, and partial repaint must equal full repaint. The wall-clock
//! check is `#[ignore]`d; see its doc comment.

use std::time::Instant;

use hx_native::*;

const W: f64 = 320.0;
const H: f64 = 120.0;

/// Win95 kit bevels plus every inset shape the fast path has to get right or
/// leave alone.
const PAGE: &str = r#"<!DOCTYPE html><html><head><style>
html,body{margin:0;background:#008080}
.b{position:absolute;box-sizing:border-box;background:#c0c0c0}
.bevel{box-shadow:inset 1px 1px #fff,inset -1px -1px #808080}
.button{box-shadow:inset -1px -1px #0a0a0a,inset 1px 1px #fff,inset -2px -2px #808080,inset 2px 2px #dfdfdf}
.field{border:2px solid #000;padding:3px;box-shadow:inset 1px 1px #808080,inset -1px -1px #dfdfdf}
.spread{box-shadow:inset 0 0 0 3px #00f}
.far{box-shadow:inset 30px 0 #f00,inset 0 -40px #0f0}
.half{box-shadow:inset 2px 2px rgba(0,0,0,.5),inset -3px -1px rgba(255,255,255,.25)}
.fade{opacity:.6}
.round{border-radius:6px;box-shadow:inset 1px 1px #fff,inset -1px -1px #808080}
.blur{box-shadow:inset 0 0 4px #000}
.clear{box-shadow:inset 1px 1px transparent,inset 2px 2px #000}
.inside{box-shadow:inset 0 0 0 20px #f0f}
.tilt{transform:rotate(3deg)}
.nudge{transform:translate(0.5px,0.25px)}
</style></head><body>
<div class="b bevel" style="left:2px;top:2px;width:120px;height:40px"></div>
<div class="b button" style="left:130px;top:2px;width:75px;height:23px"></div>
<div class="b field" style="left:212px;top:2px;width:100px;height:21px"></div>
<div class="b spread" style="left:2px;top:50px;width:30px;height:30px"></div>
<div class="b far" style="left:40px;top:50px;width:20px;height:30px"></div>
<div class="b half" style="left:68px;top:50px;width:40px;height:30px"></div>
<div class="fade" style="position:absolute;left:116px;top:50px"><div class="b bevel" style="left:0;top:0;width:40px;height:30px"></div></div>
<div class="b round" style="left:164px;top:50px;width:40px;height:30px"></div>
<div class="b blur" style="left:212px;top:50px;width:40px;height:30px"></div>
<div class="b clear" style="left:260px;top:50px;width:40px;height:30px"></div>
<div class="b inside" style="left:2px;top:88px;width:30px;height:24px"></div>
<div class="b bevel nudge" style="left:40px;top:88px;width:40px;height:24px"></div>
<div class="b bevel tilt" style="left:90px;top:88px;width:40px;height:24px"></div>
</body></html>"#;

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

#[test]
fn fast_path_is_pixel_identical_to_blitz_layers() {
    for scale in [1.0, 1.5, 2.0] {
        let _ = take_inset_shadow_counters_for_test();
        let fast = full_paint(PAGE, scale, true);
        let c = take_inset_shadow_counters_for_test();
        let layers = full_paint(PAGE, scale, false);
        assert_eq!(
            first_diff(&fast, &layers, (W * scale) as usize),
            None,
            "scale {scale}: (x, y, fast, layers)"
        );
        assert!(c.fast > 0 && c.replayed > 0, "scale {scale}: {c:?}");
    }
}

/// Layers per class at scale 1: which take the fast path and which replay.
#[test]
fn whole_pixel_layers_take_the_fast_path_and_the_rest_replay() {
    let count = |class: &str, style: &str| {
        let html = PAGE.split("</style>").next().unwrap().to_owned()
            + &format!(
                r#"</style></head><body><div class="b {class}" style="{style}"></div></body></html>"#
            );
        let _ = take_inset_shadow_counters_for_test();
        let _ = full_paint(&html, 1.0, true);
        let c = take_inset_shadow_counters_for_test();
        (c.fast, c.replayed)
    };
    let at = "left:10px;top:10px;width:60px;height:30px";
    assert_eq!(count("bevel", at), (2, 0));
    assert_eq!(count("button", at), (4, 0));
    assert_eq!(count("field", at), (2, 0));
    assert_eq!(count("spread", at), (1, 0));
    assert_eq!(count("far", at), (2, 0));
    assert_eq!(count("half", at), (2, 0));
    assert_eq!(count("round", at), (0, 2), "a corner radius replays");
    assert_eq!(count("blur", at), (0, 1), "a blur replays");
    // Blitz stops at a transparent layer (the black one is never drawn).
    assert_eq!(count("clear", at), (0, 0));
    assert_eq!(
        count("bevel nudge", at),
        (0, 2),
        "a box off whole pixels replays"
    );
    assert_eq!(count("bevel tilt", at), (0, 2), "a rotated box replays");
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
    assert_eq!((c.fast, c.replayed), (8, 0), "{c:?}");
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
