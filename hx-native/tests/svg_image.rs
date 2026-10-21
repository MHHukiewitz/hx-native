//! Issue #23, round 2 (Mike, 2026-10-21): SVG `<image>` is allowed with a
//! `data:` href of PNG, JPEG, GIF or WebP, inside the 64 KiB per-URL cap and
//! the 4096x4096 pixel cap; everything else rejects the whole load or swap
//! with `EngineError::InvalidSvg`, and nothing is ever fetched. A corrupt
//! image is rejected (decision: reject, never paint the rest), because
//! anyrender_svg would otherwise draw a half-transparent red box over it.

use std::collections::HashMap;

use hx_native::*;

const RED_PNG: &[u8] = include_bytes!("fixtures/img/red16.png");
const GREEN_JPG: &[u8] = include_bytes!("fixtures/img/green16.jpg");
const BLUE_WEBP: &[u8] = include_bytes!("fixtures/img/blue16.webp");
const ANIM_GIF: &[u8] = include_bytes!("fixtures/img/anim16.gif");
const ANIM_WEBP: &[u8] = include_bytes!("fixtures/img/anim16.webp");
const NOISE_50K: &[u8] = include_bytes!("fixtures/img/noise_50k.png");
const NOISE_OVER: &[u8] = include_bytes!("fixtures/img/noise_over_cap.png");
const BOMB_PNG: &[u8] = include_bytes!("fixtures/img/bomb.png");
const BOMB_GIF: &[u8] = include_bytes!("fixtures/img/bomb.gif");
const BOMB_FRAME_GIF: &[u8] = include_bytes!("fixtures/img/bomb_frame.gif");
const BOMB_WEBP: &[u8] = include_bytes!("fixtures/img/bomb.webp");
const CORRUPT_PNG: &[u8] = include_bytes!("fixtures/img/corrupt.png");

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const YELLOW: [u8; 3] = [255, 255, 0];

fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

fn engine() -> Engine {
    Engine::new(EngineConfig {
        max_paint_ns: 200_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    })
}

fn vp(css_w: u32, css_h: u32, scale: f64) -> Viewport {
    Viewport::with_scale(
        (css_w as f64 * scale).round() as u32,
        (css_h as f64 * scale).round() as u32,
        scale,
    )
}

fn render(html: &str, scale: f64) -> RgbaImage {
    let mut e = engine();
    e.load_html(html).expect("load_html");
    e.paint(vp(32, 32, scale)).unwrap().rgba.unwrap()
}

/// `<image>` 16x16 at the origin of an inline 16x16 `<svg>`.
fn inline_image(attr: &str, href: &str) -> String {
    inline_svg(&format!(
        "<image {attr}=\"{href}\" width=\"16\" height=\"16\"/>"
    ))
}

fn inline_svg(body: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}svg{{display:block}}</style></head><body><svg id="ic" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="16" height="16">{body}</svg></body></html>"#
    )
}

/// The same `<image>` inside a `data:image/svg+xml` CSS background.
fn data_svg_image(href: &str) -> String {
    data_svg(&format!("<image href='{href}' width='16' height='16'/>"))
}

fn data_svg(body: &str) -> String {
    let svg = format!("<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' width='16' height='16'>{body}</svg>")
        .replace('%', "%25")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('#', "%23");
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}#d{{width:16px;height:16px;background:url("data:image/svg+xml,{svg}")}}</style></head><body><div id="d"></div></body></html>"#
    )
}

/// Every device pixel of the 16x16 CSS box, 1 CSS px in from each edge
/// (image smoothing may blend the outermost ring), within `tol` of `want`.
fn assert_fill(img: &RgbaImage, scale: f64, want: [u8; 3], tol: u8, what: &str) {
    let s = scale as u32;
    let mut bad = 0;
    for y in s..15 * s {
        for x in s..15 * s {
            let i = ((y * img.width + x) * 4) as usize;
            let px = &img.pixels[i..i + 3];
            if px.iter().zip(want).any(|(a, b)| a.abs_diff(b) > tol) {
                bad += 1;
            }
        }
    }
    assert_eq!(
        bad, 0,
        "{what} at scale {scale}: {bad} pixels differ from {want:?}"
    );
}

fn reject(html: &str) -> SvgRejection {
    match engine().load_html(html) {
        Err(EngineError::InvalidSvg { rejection, .. }) => rejection,
        other => panic!("expected InvalidSvg, got {other:?}"),
    }
}

#[test]
fn png_jpeg_and_webp_images_paint_inline_and_nested_at_scale_1_and_2() {
    for scale in [1.0, 2.0] {
        for (mime, bytes, want, tol, name) in [
            ("image/png", RED_PNG, RED, 0, "PNG"),
            ("image/jpeg", GREEN_JPG, GREEN, 3, "JPEG"),
            ("image/webp", BLUE_WEBP, BLUE, 0, "WebP"),
        ] {
            let u = uri(mime, bytes);
            assert_fill(
                &render(&inline_image("href", &u), scale),
                scale,
                want,
                tol,
                &format!("inline {name}"),
            );
            assert_fill(
                &render(&inline_image("xlink:href", &u), scale),
                scale,
                want,
                tol,
                &format!("inline xlink {name}"),
            );
            assert_fill(
                &render(&data_svg_image(&u), scale),
                scale,
                want,
                tol,
                &format!("data: SVG > data: {name}"),
            );
        }
    }
}

#[test]
fn scheme_and_mime_case_variants_paint() {
    let b64 = base64(RED_PNG);
    for href in [
        format!("DATA:IMAGE/PNG;BASE64,{b64}"),
        format!("Data:Image/Png;base64,{b64}"),
        format!(" data:image/png;base64,{b64} "),
    ] {
        assert_fill(
            &render(&inline_image("href", &href), 1.0),
            1.0,
            RED,
            0,
            &href[..20],
        );
        assert_fill(
            &render(&data_svg_image(&href), 1.0),
            1.0,
            RED,
            0,
            &href[..20],
        );
    }
}

#[test]
fn image_reached_through_use_is_checked_and_paints() {
    let ok = uri("image/png", RED_PNG);
    let body = format!(
        "<defs><image id='im' href='{ok}' width='16' height='16'/></defs><use href='#im'/>"
    );
    assert_fill(
        &render(&inline_svg(&body), 1.0),
        1.0,
        RED,
        0,
        "inline <use>",
    );
    assert_fill(&render(&data_svg(&body), 1.0), 1.0, RED, 0, "data: <use>");
    for bad in ["file:///etc/hostname", "a.png"] {
        let body = format!(
            "<defs><image id='im' href='{bad}' width='16' height='16'/></defs><use href='#im'/>"
        );
        assert!(
            matches!(reject(&inline_svg(&body)), SvgRejection::ImageHref { .. }),
            "inline <use> {bad}"
        );
        assert!(
            matches!(reject(&data_svg(&body)), SvgRejection::ImageHref { .. }),
            "data: <use> {bad}"
        );
    }
}

#[test]
fn animated_gif_and_webp_paint_their_first_frame() {
    for scale in [1.0, 2.0] {
        for (mime, bytes) in [("image/gif", ANIM_GIF), ("image/webp", ANIM_WEBP)] {
            let u = uri(mime, bytes);
            assert_fill(
                &render(&inline_image("href", &u), scale),
                scale,
                YELLOW,
                0,
                mime,
            );
            assert_fill(&render(&data_svg_image(&u), scale), scale, YELLOW, 0, mime);
        }
    }
}

#[test]
fn external_and_other_type_hrefs_are_refused() {
    let png = base64(RED_PNG);
    let svg = "data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22/%3E";
    for href in [
        "http://127.0.0.1:9/a.png".to_string(),
        "https://example.com/a.png".to_string(),
        "file:///etc/hostname".to_string(),
        "/etc/hostname".to_string(),
        "a.png".to_string(),
        "#frag".to_string(),
        svg.to_string(),
        format!("data:text/plain;base64,{png}"),
        format!("data:image/jpg;base64,{png}"),
        format!("data:image/bmp;base64,{png}"),
        format!("data:;base64,{png}"),
    ] {
        for attr in ["href", "xlink:href"] {
            assert!(
                matches!(
                    reject(&inline_image(attr, &href)),
                    SvgRejection::ImageHref { .. }
                ),
                "inline {attr}={href}"
            );
        }
        if !href.starts_with('#') {
            assert!(
                matches!(
                    reject(&data_svg_image(&href)),
                    SvgRejection::ImageHref { .. }
                ),
                "data: {href}"
            );
        }
    }
    // Two hrefs: both are checked.
    let both = inline_svg(&format!(
        "<image href='{}' xlink:href='a.png' width='16' height='16'/>",
        uri("image/png", RED_PNG)
    ));
    assert!(matches!(reject(&both), SvgRejection::ImageHref { .. }));
}

#[test]
fn byte_cap_is_on_the_decoded_image() {
    // 51 KiB decoded, about 69 KiB of base64 text: accepted (inline).
    assert!(uri("image/png", NOISE_50K).len() > DATA_SVG_MAX_BYTES);
    let mut e = engine();
    e.load_html(&inline_image("href", &uri("image/png", NOISE_50K)))
        .expect("51 KiB decoded is under the cap");
    // 66 KiB decoded: refused.
    assert!(NOISE_OVER.len() > DATA_SVG_MAX_BYTES);
    assert_eq!(
        reject(&inline_image("href", &uri("image/png", NOISE_OVER))),
        SvgRejection::TooLarge {
            bytes: NOISE_OVER.len(),
            limit: DATA_SVG_MAX_BYTES
        }
    );
    // Nested in a data: SVG the inner image is decoded and checked on its
    // own (the outer SVG's 64 KiB already bounds the inner text, so an
    // over-cap inner image can only come from an inline <svg>).
    let nested = data_svg_image(&uri("image/png", RED_PNG));
    engine().load_html(&nested).expect("nested PNG");
}

#[test]
fn decompression_bombs_are_refused_from_the_header() {
    for (mime, bytes, w, h) in [
        ("image/png", BOMB_PNG, 5000, 5000),
        ("image/gif", BOMB_GIF, 65535, 65535),
        ("image/gif", BOMB_FRAME_GIF, 8000, 8000),
        ("image/webp", BOMB_WEBP, 16384, 16384),
    ] {
        assert!(bytes.len() < 1024, "the bomb is tiny");
        let want = SvgRejection::ImageTooLarge {
            width: w,
            height: h,
            limit: DATA_IMAGE_MAX_PIXELS,
        };
        let u = uri(mime, bytes);
        assert_eq!(reject(&inline_image("href", &u)), want, "inline {mime}");
        assert_eq!(reject(&data_svg_image(&u)), want, "data: {mime}");
    }
}

#[test]
fn declared_type_must_match_the_bytes() {
    for (declared, bytes, actual) in [
        ("image/jpeg", RED_PNG, "image/png"),
        ("image/png", GREEN_JPG, "image/jpeg"),
        ("image/gif", BLUE_WEBP, "image/webp"),
        ("image/webp", ANIM_GIF, "image/gif"),
    ] {
        let want = SvgRejection::ImageTypeMismatch {
            declared: declared.into(),
            actual: actual.into(),
        };
        let u = uri(declared, bytes);
        assert_eq!(reject(&inline_image("href", &u)), want);
        assert_eq!(reject(&data_svg_image(&u)), want);
    }
    // SVG markup behind a raster type: not a raster at all.
    let u = uri("image/png", b"<svg xmlns='http://www.w3.org/2000/svg'/>");
    assert!(matches!(
        reject(&inline_image("href", &u)),
        SvgRejection::CorruptImage { .. }
    ));
}

#[test]
fn corrupt_images_reject_the_load_without_panicking() {
    let truncated = &RED_PNG[..RED_PNG.len() / 2];
    for (bytes, what) in [
        (CORRUPT_PNG, "garbage IDAT"),
        (truncated, "truncated"),
        (&RED_PNG[..20], "header only"),
        (&b"\x89PNG\r\n\x1a\nnot a chunk"[..], "bad IHDR"),
    ] {
        let u = uri("image/png", bytes);
        assert!(
            matches!(
                reject(&inline_image("href", &u)),
                SvgRejection::CorruptImage { .. }
            ),
            "inline {what}"
        );
        assert!(
            matches!(
                reject(&data_svg_image(&u)),
                SvgRejection::CorruptImage { .. }
            ),
            "data: {what}"
        );
    }
    let truncated_jpeg = &GREEN_JPG[..12];
    let u = uri("image/jpeg", truncated_jpeg);
    assert!(matches!(
        reject(&inline_image("href", &u)),
        SvgRejection::CorruptImage { .. }
    ));
}

// ---------------------------------------------------------------------------
// Swaps.

struct Slices(HashMap<String, u64>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
}

struct Frags(String);
impl FragmentSource for Frags {
    fn render(&mut self, _section_id: &str, _slices: &[&str]) -> String {
        self.0.clone()
    }
}

fn shell(translate: bool) -> String {
    let tf = if translate {
        "transform:translate(3px,2px);"
    } else {
        ""
    };
    format!(
        r##"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#c0c0c0}}
#other{{position:absolute;left:0;top:0;width:160px;height:20px;background:#000080}}
#sec{{position:absolute;left:20px;top:40px;width:120px;height:40px;{tf}}}
svg{{display:inline-block;margin-right:4px}}
</style></head><body><div id="other"></div>
<section id="sec-s" hx-trigger="slice:hud" hx-target="#sec" hx-swap="innerHTML"><div id="sec"></div></section>
</body></html>"##
    )
}

fn image_fragment() -> String {
    let img = |mime, b| {
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'><image href='{}' width='16' height='16'/></svg>",
            uri(mime, b)
        )
    };
    format!(
        "{}{}{}",
        img("image/png", RED_PNG),
        img("image/jpeg", GREEN_JPG),
        img("image/gif", ANIM_GIF)
    )
}

fn partial_equals_full(translate: bool, scale: f64) {
    let mut e = engine();
    e.load_html(&shell(translate)).unwrap();
    let mut slices = Slices(HashMap::from([("hud".to_string(), 1)]));
    e.sync(&slices, &mut Frags("<span id=\"f-empty\"></span>".into()))
        .unwrap();
    e.paint(vp(160, 100, scale)).unwrap();
    slices.0.insert("hud".into(), 2);
    e.sync(&slices, &mut Frags(image_fragment())).unwrap();
    let partial = e.paint(vp(160, 100, scale)).unwrap();
    assert!(
        !e.last_paint_stats().full_repaint,
        "image swap must not full-repaint"
    );
    for r in &partial.damage {
        assert!(
            r.y >= 36.0 * scale as f32,
            "damage {r:?} reaches the top strip"
        );
    }

    let mut f = engine();
    f.load_html(&shell(translate)).unwrap();
    f.sync(
        &Slices(HashMap::from([("hud".to_string(), 1)])),
        &mut Frags(image_fragment()),
    )
    .unwrap();
    f.mark_theme_or_stylesheet_changed();
    let full = f.paint(vp(160, 100, scale)).unwrap();
    let p = partial.rgba.as_ref().unwrap();
    assert_eq!(
        p.pixels,
        full.rgba.as_ref().unwrap().pixels,
        "translate={translate} scale={scale}"
    );
    let red = p.pixels.chunks(4).filter(|px| px[..3] == RED).count();
    assert!(
        red >= (14 * 14) as usize * (scale * scale) as usize,
        "the PNG painted ({red} red px)"
    );
}

#[test]
fn image_swap_partial_equals_full_scale_1() {
    partial_equals_full(false, 1.0);
}

#[test]
fn image_swap_partial_equals_full_scale_2() {
    partial_equals_full(false, 2.0);
}

#[test]
fn image_swap_partial_equals_full_translated_scale_1() {
    partial_equals_full(true, 1.0);
}

#[test]
fn image_swap_partial_equals_full_translated_scale_2() {
    partial_equals_full(true, 2.0);
}

#[test]
fn rejected_image_fragment_changes_nothing() {
    let mut e = engine();
    e.load_html(&shell(false)).unwrap();
    let mut slices = Slices(HashMap::from([("hud".to_string(), 1)]));
    e.sync(&slices, &mut Frags(image_fragment())).unwrap();
    let before = e.serialize_html().unwrap();
    let frame = e.paint(vp(160, 100, 1.0)).unwrap().rgba.unwrap().pixels;
    slices.0.insert("hud".into(), 2);
    for bad in [
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg'><image href='{}'/></svg>",
            uri("image/png", CORRUPT_PNG)
        ),
        "<svg xmlns='http://www.w3.org/2000/svg'><image href='http://127.0.0.1:9/a.png'/></svg>"
            .to_string(),
    ] {
        let err = e.sync(&slices, &mut Frags(bad.clone())).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidSvg { .. }),
            "{bad}: {err:?}"
        );
        assert_eq!(e.serialize_html().unwrap(), before);
        let now = e
            .paint(vp(160, 100, 1.0))
            .unwrap()
            .rgba
            .map(|r| r.pixels)
            .unwrap_or(frame.clone());
        assert_eq!(now, frame);
    }
}
