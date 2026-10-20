//! Issue #12: single-line `text-overflow: ellipsis`.

use std::time::{Duration, Instant};

use hx_native::*;

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    }
}

fn gmode_why_page(reason: &str) -> String {
    // Mirrors Mainnet `#gmode-why`: fixed 200px, nowrap, overflow hidden, ellipsis.
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 320px; height: 80px; background: #1a1f2a; color: #9aa3b5;
  font-family: "DejaVu Sans", sans-serif; font-size: 13px; }}
#gmode-why {{
  position: absolute; left: 8px; top: 24px;
  width: 200px; height: 20px; line-height: 20px;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}}
</style></head><body>
<div id="gmode-why">{reason}</div>
</body></html>"##
    )
}

fn gmode_why_with_button(reason: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 320px; height: 80px; background: #1a1f2a; color: #9aa3b5;
  font-family: "DejaVu Sans", sans-serif; font-size: 13px; }}
#gmode-why {{
  position: absolute; left: 8px; top: 24px;
  width: 200px; height: 20px; line-height: 20px;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}}
#act {{ position: absolute; left: 220px; top: 20px; width: 80px; height: 28px;
  background: #2d3648; color: #e8ecf4; border: 1px solid #556; }}
#act:hover {{ background: #3d7ea6; }}
</style></head><body>
<div id="gmode-why">{reason}</div>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##
    )
}

fn plain_page(body_inner: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 320px; height: 80px; background: #1a1f2a; color: #9aa3b5;
  font-family: "DejaVu Sans", sans-serif; font-size: 13px; }}
#label {{
  position: absolute; left: 8px; top: 24px;
  width: 200px; height: 20px; line-height: 20px;
  white-space: nowrap; overflow: hidden;
}}
</style></head><body>
<div id="label">{body_inner}</div>
</body></html>"##
    )
}

fn short_reason() -> &'static str {
    "ok"
}

fn long_reason() -> &'static str {
    "Disabled: not enough credit on the active settlement rail for this order size"
}

fn vp() -> Viewport {
    Viewport::new(320, 80)
}

fn boot(html: &str) -> Engine {
    let mut eng = Engine::new(cfg());
    eng.declare_actions(&["act.do"]);
    eng.load_html(html).expect("load");
    eng
}

/// Body background `#1a1f2a` sums to 99; real DejaVu glyphs sit well above that.
const TEXT_LUMA_MIN: u16 = 160;

fn is_text_pixel(px: &[u8]) -> bool {
    px[3] >= 8 && px[0] as u16 + px[1] as u16 + px[2] as u16 > TEXT_LUMA_MIN
}

fn rightmost_ink_x(img: &RgbaImage, box_x0: u32, box_x1: u32, y0: u32, y1: u32) -> Option<u32> {
    let mut max_x = None;
    for y in y0..y1.min(img.height) {
        for x in box_x0..box_x1.min(img.width) {
            let i = ((y * img.width + x) * 4) as usize;
            if is_text_pixel(&img.pixels[i..i + 4]) {
                max_x = Some(max_x.map_or(x, |m: u32| m.max(x)));
            }
        }
    }
    max_x
}

fn opaque_text_pixel_count(img: &RgbaImage) -> usize {
    img.pixels
        .chunks_exact(4)
        .filter(|px| is_text_pixel(px))
        .count()
}

#[test]
fn long_text_gets_ellipsis_and_stays_inside_box() {
    let reason = long_reason();
    let mut eng = boot(&gmode_why_page(reason));
    let out = eng.paint(vp()).expect("paint");
    assert!(!out.skipped);
    let img = out.rgba.expect("rgba");

    let box_x0 = 8u32;
    let box_x1 = 208u32;
    let y0 = 24u32;
    let y1 = 44u32;
    let right = rightmost_ink_x(&img, box_x0, box_x1 + 40, y0, y1)
        .expect("expected visible text pixels in #gmode-why");
    assert!(
        right < box_x1,
        "text ink leaked past content box: rightmost={right} box_x1={box_x1}"
    );
    assert!(
        opaque_text_pixel_count(&img) > 20,
        "expected painted text (and ellipsis), got almost no ink"
    );

    let truncated = eng
        .test_ellipsis_truncated("gmode-why")
        .expect("truncated cache");
    assert!(
        truncated.ends_with('\u{2026}'),
        "truncated paint string must end with ellipsis: {truncated:?}"
    );

    // Full reason string must still be in the serialized DOM after paint.
    let html = eng.serialize_html().expect("serialize");
    assert!(
        html.contains(reason),
        "original text missing after paint restore"
    );
    assert!(
        !html.contains('\u{2026}'),
        "ellipsis must not remain in the DOM after paint"
    );
}

#[test]
fn short_text_unchanged() {
    let mut eng = boot(&gmode_why_page(short_reason()));
    let out = eng.paint(vp()).expect("paint");
    let img = out.rgba.expect("rgba");
    let right = rightmost_ink_x(&img, 8, 208, 24, 44).expect("short text should paint");
    assert!(
        right < 8 + 40,
        "short text unexpectedly wide: right={right}"
    );
    assert!(eng.test_ellipsis_truncated("gmode-why").is_none());
    let html = eng.serialize_html().expect("serialize");
    assert!(html.contains(">ok<") || html.contains(">ok</"));
    assert!(!html.contains('\u{2026}'));
}

#[test]
fn partial_and_full_repaint_byte_identical_with_ellipsis() {
    let html = gmode_why_with_button(long_reason());

    let mut eng = boot(&html);
    let first = eng.paint(vp()).unwrap();
    let base = first.rgba.unwrap().clone();

    let _ = eng.input(InputEvent::PointerMove {
        x: 250.0,
        y: 30.0,
        mods: Modifiers::default(),
    });
    let partial = eng.paint(vp()).unwrap();
    assert!(!partial.skipped);
    assert!(
        !partial.damage.is_empty(),
        "hover should produce damage rects"
    );
    let partial_img = partial.rgba.unwrap();

    let mut eng_full = boot(&html);
    let _ = eng_full.input(InputEvent::PointerMove {
        x: 250.0,
        y: 30.0,
        mods: Modifiers::default(),
    });
    let full_img = eng_full.paint(vp()).unwrap().rgba.unwrap();

    assert_eq!(
        partial_img.pixels, full_img.pixels,
        "partial hover paint must be byte-identical to full paint with same hover"
    );

    for y in 24..44 {
        for x in 8..208 {
            let i = ((y * base.width + x) * 4) as usize;
            assert_eq!(
                &partial_img.pixels[i..i + 4],
                &base.pixels[i..i + 4],
                "ellipsis band changed on button hover at ({x},{y})"
            );
        }
    }
}

#[test]
fn worker_matches_sync_engine_with_ellipsis() {
    let html = gmode_why_page(long_reason());

    let mut sync = boot(&html);
    sync.resize_surface(SurfaceId::DEFAULT, vp()).unwrap();
    let sync_img = sync.paint(vp()).unwrap().rgba.unwrap();

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(&html).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, vp()).unwrap();
    worker.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let frame = loop {
        if let Some(view) = worker.latest_frame(SurfaceId::DEFAULT, 0) {
            if view.output.rgba.is_some() {
                break view;
            }
        }
        assert!(Instant::now() < deadline, "worker frame timeout");
        std::thread::sleep(Duration::from_millis(5));
    };
    let worker_img = frame.output.rgba.as_ref().expect("worker rgba");
    assert_eq!(
        sync_img.pixels, worker_img.pixels,
        "PaintWorker frame must match sync Engine for ellipsis page"
    );
}

#[test]
fn idle_frames_do_no_resolve_with_or_without_ellipsis() {
    // Static document: after the first paint, N idle paints must not resolve
    // (skipped clean frames). resolve_count must match a page without ellipsis.
    const N: u64 = 8;

    let mut with_e = boot(&gmode_why_page(long_reason()));
    let _ = with_e.paint(vp()).unwrap();
    let after_first_e = with_e.resolve_count();
    // Hit-test must not force a resolve (DOM + original layout already restored).
    let _ = with_e.input(InputEvent::PointerMove {
        x: 20.0,
        y: 30.0,
        mods: Modifiers::default(),
    });
    // Pointer move marks paint dirty when hover changes — paint once more to
    // settle, then measure truly idle skips.
    let _ = with_e.paint(vp()).unwrap();
    let settled_e = with_e.resolve_count();
    for _ in 0..N {
        let out = with_e.paint(vp()).unwrap();
        assert!(out.skipped, "idle frame must skip raster");
    }
    assert_eq!(
        with_e.resolve_count(),
        settled_e,
        "idle skips must not resolve (ellipsis page)"
    );
    assert!(
        after_first_e >= 1,
        "first paint must have resolved at least once"
    );

    let mut without = boot(&plain_page(long_reason()));
    let _ = without.paint(vp()).unwrap();
    let _ = without.input(InputEvent::PointerMove {
        x: 20.0,
        y: 30.0,
        mods: Modifiers::default(),
    });
    let _ = without.paint(vp()).unwrap();
    let settled_plain = without.resolve_count();
    for _ in 0..N {
        let out = without.paint(vp()).unwrap();
        assert!(out.skipped, "idle frame must skip raster");
    }
    assert_eq!(
        without.resolve_count(),
        settled_plain,
        "idle skips must not resolve (plain page)"
    );

    // Idle skip cost is the same class of work: zero resolves after settle.
    assert_eq!(
        with_e.resolve_count() - settled_e,
        without.resolve_count() - settled_plain
    );
}

#[test]
fn cluster_boundaries_keep_combining_mark_and_zwj() {
    // Place combining mark and ZWJ near where a 200px / 13px cut lands (~20–28 chars).
    let combining = "e\u{0301}"; // é as base + combining acute
    let zwj = "\u{1F468}\u{200D}\u{1F469}"; // man + ZWJ + woman
    let reason = format!(
        "{}{}{}{} more credit needed for settlement overflow overflow",
        "A".repeat(18),
        combining,
        "B".repeat(4),
        zwj,
    );
    let mut eng = boot(&gmode_why_page(&reason));
    let _ = eng.paint(vp()).unwrap();
    let truncated = eng
        .test_ellipsis_truncated("gmode-why")
        .expect("should truncate");
    assert!(
        truncated.ends_with('\u{2026}'),
        "expected ellipsis marker: {truncated:?}"
    );

    let body = truncated.trim_end_matches('\u{2026}');
    assert!(
        reason.starts_with(body),
        "truncated body must be a prefix of the original:\nbody={body:?}\norig={reason:?}"
    );

    // Cluster-safe: the next scalar after the cut must not be a combining mark or ZWJ
    // (those continue a cluster that should have been kept wholly or dropped wholly).
    if let Some(next) = reason[body.len()..].chars().next() {
        assert_ne!(
            next, '\u{0301}',
            "cut before combining mark (scalar split): {truncated:?}"
        );
        assert_ne!(
            next, '\u{200D}',
            "cut before ZWJ (scalar split): {truncated:?}"
        );
    }
    assert!(
        !body.ends_with('\u{200D}'),
        "must not end on a bare ZWJ: {truncated:?}"
    );
    // If the combining sequence appears, it must be complete (not `e` alone at the cut).
    if body.ends_with('e') {
        let next = reason[body.len()..].chars().next();
        assert_ne!(
            next,
            Some('\u{0301}'),
            "cut between base and combining mark: {truncated:?}"
        );
    }
    // If a ZWJ family was started, the full sequence must be present.
    if let Some(i) = body.find('\u{1F468}') {
        let rest = &body[i..];
        assert!(
            rest.starts_with(zwj),
            "ZWJ emoji must not be split: {truncated:?}"
        );
    }

    let html = eng.serialize_html().unwrap();
    assert!(html.contains(&reason), "DOM keeps full original text");
}

#[test]
fn paint_error_restores_dom_and_layout() {
    let reason = long_reason();
    let mut eng = boot(&gmode_why_page(reason));
    let first = eng.paint(vp()).unwrap();
    assert!(!first.skipped);
    assert!(eng.test_ellipsis_truncated("gmode-why").is_some());
    assert_eq!(eng.test_element_text("gmode-why").as_deref(), Some(reason));
    // After a successful paint, Drop restored the full-text layout.
    assert_eq!(
        eng.test_inline_layout_text("gmode-why").as_deref(),
        Some(reason),
        "restored inline_layout_data.text must be the full original"
    );

    eng.mark_theme_or_stylesheet_changed();
    eng.test_fail_next_paint();
    let err = eng.paint(vp());
    assert!(err.is_err(), "expected forced paint error, got {err:?}");

    assert_eq!(
        eng.test_element_text("gmode-why").as_deref(),
        Some(reason),
        "DOM text must be restored after paint error"
    );
    assert_eq!(
        eng.test_inline_layout_text("gmode-why").as_deref(),
        Some(reason),
        "inline_layout_data.text must be the full original after paint error \
         (fails if Drop restore is removed)"
    );
    // Next paint must still detect overflow and apply ellipsis.
    eng.mark_theme_or_stylesheet_changed();
    let out = eng.paint(vp()).unwrap();
    assert!(!out.skipped);
    let truncated = eng
        .test_ellipsis_truncated("gmode-why")
        .expect("ellipsis still applied after error recovery");
    assert!(truncated.ends_with('\u{2026}'));
    assert_eq!(
        eng.test_inline_layout_text("gmode-why").as_deref(),
        Some(reason)
    );
}

#[test]
fn paint_panic_restores_dom_and_layout() {
    let reason = long_reason();
    let mut eng = boot(&gmode_why_page(reason));
    let _ = eng.paint(vp()).unwrap();
    assert!(eng.test_ellipsis_truncated("gmode-why").is_some());
    assert_eq!(
        eng.test_inline_layout_text("gmode-why").as_deref(),
        Some(reason)
    );

    eng.mark_theme_or_stylesheet_changed();
    eng.test_panic_next_paint();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = eng.paint(vp());
    }));
    assert!(panicked.is_err(), "expected panic inside paint");

    assert_eq!(
        eng.test_element_text("gmode-why").as_deref(),
        Some(reason),
        "DOM text must be restored after panic"
    );
    assert_eq!(
        eng.test_inline_layout_text("gmode-why").as_deref(),
        Some(reason),
        "inline_layout_data.text must be the full original after panic \
         (fails if Drop restore is removed)"
    );
    eng.mark_theme_or_stylesheet_changed();
    let out = eng.paint(vp()).unwrap();
    assert!(!out.skipped);
    assert!(
        eng.test_ellipsis_truncated("gmode-why")
            .is_some_and(|t| t.ends_with('\u{2026}')),
        "ellipsis still applied after panic recovery"
    );
    assert_eq!(
        eng.test_inline_layout_text("gmode-why").as_deref(),
        Some(reason)
    );
}

#[test]
fn flex_ellipsis_hover_matches_without_ellipsis() {
    // Content-sized flex item (`flex: 0 1 auto` + max-width) with ellipsis, then a
    // hover target. Probe just inside the *full* text's right edge — if a cache miss
    // left truncated geometry, that point would miss #title (or shift #act left).
    fn page(ellipsis: bool) -> String {
        let overflow = if ellipsis {
            "white-space: nowrap; overflow: hidden; text-overflow: ellipsis;"
        } else {
            "white-space: nowrap; overflow: hidden;"
        };
        format!(
            r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 400px; height: 80px; background: #1a1f2a; color: #9aa3b5;
  font-family: "DejaVu Sans", sans-serif; font-size: 13px; }}
#row {{
  display: flex; flex-direction: row; align-items: center;
  position: absolute; left: 8px; top: 20px; height: 28px;
}}
#title {{
  flex: 0 1 auto; max-width: 200px; height: 20px; line-height: 20px;
  {overflow}
}}
#act {{
  flex: 0 0 auto; width: 72px; height: 28px; margin-left: 4px;
  background: #2d3648; color: #e8ecf4; border: 1px solid #556;
}}
#act:hover {{ background: #3d7ea6; }}
</style></head><body>
<div id="row">
  <div id="title">Disabled: not enough credit on the active settlement rail for this order size</div>
  <button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</div>
</body></html>"##
        )
    }

    let view = Viewport::new(400, 80);
    let mut with_e = boot(&page(true));
    let mut without = boot(&page(false));
    let _ = with_e.paint(view.clone()).unwrap();
    let _ = without.paint(view.clone()).unwrap();
    assert!(
        with_e.test_ellipsis_truncated("title").is_some(),
        "flex title should truncate"
    );

    // Just inside the full #title content box (max-width 200 at x=8 → edge ~208).
    // Truncated geometry would shrink the hit target and miss here.
    let edge_x = 200.0;
    let tx = with_e.input(InputEvent::PointerMove {
        x: edge_x,
        y: 34.0,
        mods: Modifiers::default(),
    });
    let to = without.input(InputEvent::PointerMove {
        x: edge_x,
        y: 34.0,
        mods: Modifiers::default(),
    });
    assert!(
        matches!(tx, InputOutcome::Hover { id: Some(ref id) } if id == "title"),
        "ellipsis title hit at full edge: {tx:?}"
    );
    assert!(
        matches!(to, InputOutcome::Hover { id: Some(ref id) } if id == "title"),
        "plain title hit at full edge: {to:?}"
    );

    // Button sits after the 200px title (+ margin); both pages must hit #act.
    let hx = with_e.input(InputEvent::PointerMove {
        x: 230.0,
        y: 34.0,
        mods: Modifiers::default(),
    });
    let ho = without.input(InputEvent::PointerMove {
        x: 230.0,
        y: 34.0,
        mods: Modifiers::default(),
    });
    assert!(
        matches!(hx, InputOutcome::Hover { id: Some(ref id) } if id == "act"),
        "ellipsis page button hover: {hx:?}"
    );
    assert!(
        matches!(ho, InputOutcome::Hover { id: Some(ref id) } if id == "act"),
        "plain page button hover: {ho:?}"
    );
}

#[test]
fn ellipsis_cache_miss_does_not_exceed_style_layout_budget() {
    // Direct parley reshape must not spend a document style/layout pass.
    // Budget remains at most 2 passes per paint (resolve + optional hover follow-up).
    let mut eng = boot(&gmode_why_page(long_reason()));
    let before = eng.style_layout_pass_count();
    let out = eng.paint(vp()).unwrap();
    assert!(!out.skipped);
    let after_miss = eng.style_layout_pass_count();
    let delta_miss = after_miss - before;
    assert!(
        (1..=2).contains(&delta_miss),
        "cache-miss paint must use 1–2 document passes, got {delta_miss}"
    );
    assert!(
        eng.test_ellipsis_truncated("gmode-why").is_some(),
        "miss must still truncate"
    );

    eng.mark_theme_or_stylesheet_changed();
    let before_hit = eng.style_layout_pass_count();
    let out2 = eng.paint(vp()).unwrap();
    assert!(!out2.skipped);
    let delta_hit = eng.style_layout_pass_count() - before_hit;
    assert!(
        (1..=2).contains(&delta_hit),
        "cache-hit paint must use 1–2 document passes, got {delta_hit}"
    );
}

/// Graph font probe: ellipsis-on paint must be byte-identical to Blitz painting the
/// same truncated string with ellipsis off (same box / styles).
#[test]
fn ellipsis_paint_matches_blitz_truncated_string() {
    fn page(extra: &str, overflow: &str, text: &str) -> String {
        format!(
            r##"<!DOCTYPE html><html><head><meta charset="utf-8"/><style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 320px; height: 80px; background: #1a1f2a; color: #9aa3b5;
  font-family: "DejaVu Sans", sans-serif; font-size: 13px; }}
#t {{ position: absolute; left: 8px; top: 24px; width: 200px; height: 20px;
  line-height: 20px; white-space: nowrap; overflow: hidden; {overflow} {extra} }}
</style></head><body><div id="t">{text}</div></body></html>"##
        )
    }
    fn paint_img(html: &str) -> (Engine, RgbaImage) {
        let mut e = boot(html);
        let img = e.paint(vp()).unwrap().rgba.unwrap();
        (e, img)
    }
    fn band_diff(a: &RgbaImage, b: &RgbaImage) -> usize {
        a.pixels
            .chunks_exact(4)
            .zip(b.pixels.chunks_exact(4))
            .filter(|(p, q)| p != q)
            .count()
    }

    let text = long_reason();
    // Cases Graph required (plus the Mainnet default). Zero tolerance: byte-identical.
    let cases = [
        ("default", ""),
        ("line-height 12px", "line-height: 12px;"),
        ("letter-spacing 1px", "letter-spacing: 1px;"),
        ("word-spacing 4px", "word-spacing: 4px;"),
        ("italic", "font-style: italic;"),
    ];
    for (name, extra) in cases {
        let (e, with_ellipsis) = paint_img(&page(extra, "text-overflow: ellipsis;", text));
        let truncated = e
            .test_ellipsis_truncated("t")
            .unwrap_or_else(|| panic!("{name}: expected truncation"));
        assert!(
            truncated.ends_with('\u{2026}'),
            "{name}: truncated={truncated:?}"
        );
        let (_e2, blitz_truncated) = paint_img(&page(extra, "", &truncated));
        let diff = band_diff(&with_ellipsis, &blitz_truncated);
        assert_eq!(
            diff, 0,
            "{name}: ellipsis-on paint must be byte-identical to Blitz painting \
             the truncated string (diff_pixels={diff}, truncated={truncated:?})"
        );
    }
}
