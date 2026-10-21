//! Graph review of hx-native #30 (issue #28): pins the two rules the
//! `host_fonts.rs` suite leaves to relative comparisons, so mutants M9
//! (bundled DejaVu not mapped to the generic families) and M10 (host faces
//! registered before bundled DejaVu) are caught.
//!
//! - M9: `host_fonts.rs` (2) compares `sans-serif` with host fonts against
//!   `sans-serif` without; under M9 both engines lose the generic mapping, so
//!   the two frames still match. Here `sans-serif` must paint ink and the
//!   exact pixels of the named bundled family `"DejaVu Sans"`.
//! - M10: order relative to the bundled face is only observable when a host
//!   face lands in the bundled family itself. A host entry named
//!   "DejaVu Sans" at 400 normal (here carrying Bold bytes) must not replace
//!   the bundled regular face for `sans-serif` or for `"DejaVu Sans"`.

use std::sync::Arc;

use hx_native::*;

const REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
const BOLD: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans-Bold.ttf");

fn vp() -> Viewport {
    Viewport {
        width: 320,
        height: 120,
        scale: 1.0,
        holes: vec![],
    }
}

fn cfg(fonts: Vec<FontFace>) -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        fonts,
        ..EngineConfig::default()
    }
}

fn face(family: &str, weight: u16, bytes: &'static [u8]) -> FontFace {
    FontFace::new(family, weight, FontStyle::Normal, Arc::<[u8]>::from(bytes))
}

fn page(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff;color:#000}}\
         p{{margin:8px;font-size:32px}}</style></head><body>{body}</body></html>"
    )
}

/// `Engine::try_new` result for `fonts`, or `None` when the config is
/// rejected (a crate that reserves the bundled face may refuse it).
fn try_paint(fonts: Vec<FontFace>, body: &str) -> Option<RgbaImage> {
    let mut e = Engine::try_new(cfg(fonts)).ok()?;
    e.load_html(&page(body)).unwrap();
    let out = e.paint(vp()).unwrap();
    Some(
        out.rgba
            .unwrap_or_else(|| e.last_frame().expect("frame").clone()),
    )
}

fn paint(fonts: Vec<FontFace>, body: &str) -> RgbaImage {
    try_paint(fonts, body).expect("valid fonts")
}

fn ink(img: &RgbaImage) -> usize {
    img.pixels
        .chunks_exact(4)
        .filter(|p| p[0] < 128 && p[1] < 128 && p[2] < 128)
        .count()
}

fn diff(a: &RgbaImage, b: &RgbaImage) -> usize {
    a.pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count()
}

const TEXT: &str = "Hello Wg";

/// Kills M9: every generic family resolves to bundled DejaVu — it paints
/// ink, and the same pixels as naming the bundled family — with and without
/// host fonts registered.
#[test]
fn generic_families_resolve_to_bundled_dejavu() {
    for fonts in [Vec::new(), vec![face("Host", 400, BOLD)]] {
        let named = paint(
            fonts.clone(),
            &format!("<p style=\"font-family:'DejaVu Sans'\">{TEXT}</p>"),
        );
        assert!(ink(&named) > 0, "named bundled family must paint ink");
        for generic in ["sans-serif", "serif", "monospace", "system-ui"] {
            let img = paint(
                fonts.clone(),
                &format!("<p style=\"font-family:{generic}\">{TEXT}</p>"),
            );
            assert!(ink(&img) > 0, "{generic} must paint ink");
            assert_eq!(
                diff(&img, &named),
                0,
                "{generic} must paint bundled DejaVu (host fonts: {})",
                fonts.len()
            );
        }
    }
}

/// Kills M10: a host entry that lands in the bundled family at the bundled
/// face's own weight/style must not take over the generic fallback. Either
/// the crate refuses it (InvalidFont, reserved bundled face) or the bundled
/// face, registered first, keeps answering. Bold bytes make a takeover
/// visible.
#[test]
fn host_face_in_the_bundled_family_does_not_replace_the_fallback() {
    let reference = paint(
        Vec::new(),
        &format!("<p style=\"font-family:sans-serif\">{TEXT}</p>"),
    );
    for family in ["DejaVu Sans", "dejavu sans"] {
        for css in ["sans-serif", "'DejaVu Sans'"] {
            let body = format!("<p style=\"font-family:{css}\">{TEXT}</p>");
            match try_paint(vec![face(family, 400, BOLD)], &body) {
                None => {} // refused up front: also satisfies the rule
                Some(img) => assert_eq!(
                    diff(&img, &reference),
                    0,
                    "host face {family:?}@400 must not replace bundled DejaVu for {css}"
                ),
            }
        }
    }
}

/// Issue #56 (replaces the #30 "extend the bundled family at 700" test): the
/// bundled family already has a real 700 face, so `<b>` in `sans-serif`
/// with no host fonts paints exactly DejaVu Sans Bold (the pixels of that
/// face registered under a host name), and a host "DejaVu Sans" 700 is
/// refused because the bundled bold would shadow it.
#[test]
fn bundled_family_bold_is_the_real_bold_face() {
    let body = format!("<p style=\"font-family:sans-serif\"><b>{TEXT}</b></p>");
    let bundled = paint(Vec::new(), &body);
    let host_named = paint(
        vec![face("Bold Host", 700, BOLD)],
        &format!("<p style=\"font-family:'Bold Host'\"><b>{TEXT}</b></p>"),
    );
    assert_eq!(
        diff(&bundled, &host_named),
        0,
        "sans-serif <b> must paint the bundled DejaVu Sans Bold"
    );
    let regular = paint(
        Vec::new(),
        &format!("<p style=\"font-family:sans-serif\">{TEXT}</p>"),
    );
    assert!(
        ink(&bundled) > ink(&regular),
        "bold is heavier than regular"
    );
    assert!(
        try_paint(vec![face("DejaVu Sans", 700, BOLD)], &body).is_none(),
        "a host DejaVu Sans 700 ties with the bundled bold and is refused"
    );
    let _ = REGULAR;
}
