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

    // Full-repaint reference with the same hover state.
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

    // Ellipsis band must match the pre-hover base (button is outside the band).
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
