//! Issue #23, round 4 (Graph's #52 HOLD caps, 2026-10-21): each SVG
//! `<image>` at most 256x256 decoded pixels with no side over 4096, and at
//! most 512x512 decoded `<image>` pixels per load or swap, counting every
//! reference. anyrender_svg decodes every `<image>` on every paint, and
//! anyrender_vello_cpu panics on a side over 65535.

use std::collections::HashMap;

use hx_native::*;

const RED_256: &[u8] = include_bytes!("fixtures/img/red256.png");
const RED_16X4096: &[u8] = include_bytes!("fixtures/img/red16x4096.png");
const RED_1X4097: &[u8] = include_bytes!("fixtures/img/red1x4097.png");
const RED_257X256: &[u8] = include_bytes!("fixtures/img/red257x256.png");
const NS: &str = "http://www.w3.org/2000/svg";

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

fn reject(html: &str) -> SvgRejection {
    match engine().load_html(html) {
        Err(EngineError::InvalidSvg { rejection, .. }) => rejection,
        other => panic!("expected InvalidSvg, got {other:?}"),
    }
}

fn images(href: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("<image href='{href}' x='{}' width='4' height='4'/>", i * 4))
        .collect()
}

fn inline_page(body: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}svg{{display:block}}</style></head><body>{body}</body></html>"#
    )
}

fn svg(inner: &str) -> String {
    format!("<svg xmlns='{NS}' width='32' height='32'>{inner}</svg>")
}

fn data_div(svg: &str) -> String {
    format!(
        r#"<div style="width:32px;height:32px;background-image:url(&quot;data:image/svg+xml;base64,{}&quot;)"></div>"#,
        base64(svg.as_bytes())
    )
}

fn budget(n: u64) -> SvgRejection {
    SvgRejection::ImagePixelBudget {
        pixels: n * 256 * 256,
        limit: DATA_IMAGE_MAX_TOTAL_PIXELS,
    }
}

#[test]
fn the_caps_are_the_agreed_numbers() {
    assert_eq!(DATA_IMAGE_MAX_PIXELS, 256 * 256);
    assert_eq!(DATA_IMAGE_MAX_SIDE, 4096);
    assert_eq!(DATA_IMAGE_MAX_TOTAL_PIXELS, 512 * 512);
}

#[test]
fn a_256_square_image_paints_and_257_or_a_long_side_is_refused() {
    let u = uri("image/png", RED_256);
    let html = inline_page(&svg(&format!("<image href='{u}' width='32' height='32'/>")));
    let img = render(&html, 1.0);
    let red = img
        .pixels
        .chunks(4)
        .filter(|p| p[..3] == [255, 0, 0])
        .count();
    assert!(red >= 30 * 30, "256x256 image paints ({red} red px)");
    for (bytes, w, h) in [(RED_257X256, 257, 256), (RED_1X4097, 1, 4097)] {
        let u = uri("image/png", bytes);
        let want = SvgRejection::ImageTooLarge {
            width: w,
            height: h,
            limit: DATA_IMAGE_MAX_PIXELS,
        };
        let s = svg(&format!("<image href='{u}' width='8' height='8'/>"));
        assert_eq!(reject(&inline_page(&s)), want, "inline {w}x{h}");
        assert_eq!(reject(&inline_page(&data_div(&s))), want, "data: {w}x{h}");
    }
    // A side up to 4096 is fine while the area is within the cap.
    let u = uri("image/png", RED_16X4096);
    let html = inline_page(&svg(&format!(
        "<image href='{u}' width='32' height='32' preserveAspectRatio='none'/>"
    )));
    let img = render(&html, 1.0);
    assert!(img.pixels.chunks(4).any(|p| p[..3] == [255, 0, 0]));
}

#[test]
fn four_images_at_the_cap_load_and_a_fifth_reference_is_refused() {
    let u = uri("image/png", RED_256);
    // Four in one inline SVG, or spread over inline and data: SVGs: allowed.
    assert!(engine()
        .load_html(&inline_page(&svg(&images(&u, 4))))
        .is_ok());
    let mixed = format!("{}{}", svg(&images(&u, 2)), data_div(&svg(&images(&u, 2))));
    assert!(engine().load_html(&inline_page(&mixed)).is_ok());
    // A fifth reference, even to the same bytes, in any of those places.
    assert_eq!(reject(&inline_page(&svg(&images(&u, 5)))), budget(5));
    assert_eq!(
        reject(&inline_page(&data_div(&svg(&images(&u, 5))))),
        budget(5)
    );
    let mixed = format!("{}{}", svg(&images(&u, 3)), data_div(&svg(&images(&u, 2))));
    assert_eq!(reject(&inline_page(&mixed)), budget(5));
    // One data: SVG used by two elements counts twice.
    let tile = data_div(&svg(&images(&u, 3)));
    assert_eq!(reject(&inline_page(&format!("{tile}{tile}"))), budget(6));
}

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

#[test]
fn the_budget_is_per_swap_and_a_refused_swap_changes_nothing() {
    let u = uri("image/png", RED_256);
    let shell = inline_page(&format!(
        r##"{}<section id="s" hx-trigger="slice:hud" hx-target="#t" hx-swap="innerHTML"><div id="t"></div></section>"##,
        svg(&images(&u, 4))
    ));
    let mut e = engine();
    e.load_html(&shell).unwrap();
    let mut sl = Slices(HashMap::from([("hud".to_string(), 1)]));
    // A swap has its own budget: four more on top of the shell's four.
    e.sync(&sl, &mut Frags(svg(&images(&u, 4)))).unwrap();
    let before = e.serialize_html().unwrap();
    let frame = e.paint(vp(64, 64, 1.0)).unwrap().rgba.unwrap().pixels;
    sl.0.insert("hud".into(), 2);
    let err = e.sync(&sl, &mut Frags(svg(&images(&u, 5)))).unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidSvg { rejection, .. } if *rejection == budget(5)),
        "{err:?}"
    );
    assert_eq!(e.serialize_html().unwrap(), before);
    let now = e
        .paint(vp(64, 64, 1.0))
        .unwrap()
        .rgba
        .map(|r| r.pixels)
        .unwrap_or(frame.clone());
    assert_eq!(now, frame);
}
