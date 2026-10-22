//! Issue #28: host-supplied fonts (`EngineConfig::fonts`, Option A).
//!
//! Test fonts: the bundled DejaVu Sans (regular) and DejaVu Sans Bold vendored
//! under `tests/fonts/` (same DejaVu licence). The tests register them under
//! host family names so the face actually used is visible in the pixels.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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

fn paint_with(fonts: Vec<FontFace>, body: &str) -> RgbaImage {
    let mut e = Engine::try_new(cfg(fonts)).expect("valid fonts");
    e.load_html(&page(body)).unwrap();
    let out = e.paint(vp()).unwrap();
    out.rgba
        .unwrap_or_else(|| e.last_frame().expect("frame").clone())
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

fn host_pair() -> Vec<FontFace> {
    vec![face("Host", 400, REGULAR), face("Host", 700, BOLD)]
}

/// (1) With the family registered at 400 and 700, `<b>x</b>` paints the bold
/// face: different glyph pixels from plain `x`, and more ink.
#[test]
fn bold_weight_paints_the_700_face() {
    let regular = paint_with(host_pair(), "<p style=\"font-family:Host\">x</p>");
    let bold = paint_with(host_pair(), "<p style=\"font-family:Host\"><b>x</b></p>");
    assert!(ink(&regular) > 0, "regular x must paint ink");
    assert!(
        diff(&regular, &bold) > 0,
        "<b>x</b> must paint different glyph pixels from x"
    );
    assert!(
        ink(&bold) > ink(&regular),
        "the 700 face must paint heavier strokes ({} vs {})",
        ink(&bold),
        ink(&regular)
    );
}

/// (2) A host family paints different pixels from the DejaVu-only engine, and
/// an unregistered family (and the generic) still paints bundled DejaVu.
#[test]
fn host_family_differs_and_unknown_family_falls_back_to_dejavu() {
    let fonts = || vec![face("Aardvark Host", 400, BOLD)];
    let text = "Hello";
    let host = paint_with(
        fonts(),
        &format!("<p style=\"font-family:'Aardvark Host'\">{text}</p>"),
    );
    let dejavu_only = paint_with(
        Vec::new(),
        &format!("<p style=\"font-family:'Aardvark Host'\">{text}</p>"),
    );
    assert!(
        diff(&host, &dejavu_only) > 0,
        "a registered host family must change the pixels"
    );

    // Fixed expectation first (Graph review of #30): comparing two engines
    // alone would pass when both lose the fallback (both paint nothing).
    let named_dejavu = paint_with(
        fonts(),
        &format!("<p style=\"font-family:'DejaVu Sans'\">{text}</p>"),
    );
    assert!(ink(&named_dejavu) > 0, "bundled DejaVu must paint ink");
    for family in ["'MS Sans Serif'", "sans-serif"] {
        let body = format!("<p style=\"font-family:{family}\">{text}</p>");
        let with_host = paint_with(fonts(), &body);
        assert!(ink(&with_host) > 0, "{family} must paint ink");
        assert_eq!(
            diff(&with_host, &named_dejavu),
            0,
            "{family} must paint the bundled DejaVu face"
        );
        let without = paint_with(Vec::new(), &body);
        assert_eq!(
            diff(&with_host, &without),
            0,
            "{family} must still paint bundled DejaVu when host fonts are registered"
        );
    }
}

fn invalid(fonts: Vec<FontFace>) -> EngineError {
    match Engine::try_new(cfg(fonts)) {
        Err(e) => e,
        Ok(_) => panic!("expected InvalidFont"),
    }
}

/// (3) Bad bytes, an empty family and a duplicate each give `InvalidFont`
/// at build time, with no panic, from `Engine::try_new`,
/// `PaintWorker::try_spawn`, and (deferred) `Engine::new` + `load_html`.
#[test]
fn invalid_fonts_fail_build_with_invalid_font() {
    let garbage: &'static [u8] = b"definitely not a font file";
    let bad_woff2: &'static [u8] = b"wOF2\0\0\0\0garbage-after-a-woff2-magic";
    let cases: Vec<(&str, Vec<FontFace>, usize)> = vec![
        ("garbage bytes", vec![face("Host", 400, garbage)], 0),
        ("bad woff2", vec![face("Host", 400, bad_woff2)], 0),
        ("empty bytes", vec![face("Host", 400, b"")], 0),
        ("empty family", vec![face("", 400, REGULAR)], 0),
        ("blank family", vec![face("   ", 400, REGULAR)], 0),
        ("weight 0", vec![face("Host", 0, REGULAR)], 0),
        (
            "duplicate (case-insensitive family)",
            vec![face("Host", 400, REGULAR), face("hOST", 400, BOLD)],
            1,
        ),
    ];
    for (what, fonts, bad_index) in cases {
        match invalid(fonts.clone()) {
            EngineError::InvalidFont { index, reason, .. } => {
                assert_eq!(index, bad_index, "{what}: wrong index");
                assert!(!reason.is_empty(), "{what}: reason must be set");
            }
            other => panic!("{what}: expected InvalidFont, got {other:?}"),
        }
        assert!(
            matches!(
                PaintWorker::try_spawn(cfg(fonts.clone())),
                Err(EngineError::InvalidFont { .. })
            ),
            "{what}: PaintWorker::try_spawn must refuse it"
        );
        let mut e = Engine::new(cfg(fonts));
        assert!(
            matches!(
                e.load_html(&page("<p>x</p>")),
                Err(EngineError::InvalidFont { .. })
            ),
            "{what}: Engine::new defers the error to load_html"
        );
    }

    // Same family at another weight or style is not a duplicate.
    let ok = vec![
        face("Host", 400, REGULAR),
        face("Host", 700, BOLD),
        FontFace::new("Host", 400, FontStyle::Italic, Arc::<[u8]>::from(REGULAR)),
    ];
    assert!(Engine::try_new(cfg(ok)).is_ok());
}

/// (4) Partial repaint equals a forced full repaint with host fonts, after a
/// section swap that re-renders host-font text.
#[test]
fn partial_repaint_equals_full_with_host_fonts() {
    let html = page(
        "<p id=\"keep\" style=\"font-family:Host\">Keep</p>\
         <section id=\"sec\" hx-trigger=\"slice:s\"><p style=\"font-family:Host\"><b>One</b></p></section>",
    );
    let run = |fonts: Vec<FontFace>| {
        let mut e = Engine::try_new(cfg(fonts)).unwrap();
        e.load_html(&html).unwrap();
        let _ = e.paint(vp());
        let mut p = SyncPacket::default();
        p.versions = HashMap::from([("s".into(), 1)]);
        p.fragments.insert(
            "sec".into(),
            "<p style=\"font-family:Host\"><b>Two</b></p>".into(),
        );
        e.sync(&p, &mut p.clone()).unwrap();
        p.versions.insert("s".into(), 2);
        p.fragments.insert(
            "sec".into(),
            "<p style=\"font-family:Host\"><b>Three</b></p>".into(),
        );
        e.sync(&p, &mut p.clone()).unwrap();
        let out = e.paint(vp()).unwrap();
        assert!(!out.damage.is_empty(), "the swap must damage something");
        let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
        e.mark_theme_or_stylesheet_changed();
        let full = e.paint(vp()).unwrap().rgba.expect("full frame");
        (partial, full)
    };
    let (partial, full) = run(host_pair());
    assert_eq!(diff(&partial, &full), 0, "partial must equal full");
    // The test faces are DejaVu, and since #56 the bundled fallback paints
    // DejaVu Sans Bold too, so a DejaVu-only engine paints the same pixels.
    // Prove the host 700 face is in the frames against the host family with
    // only its 400 face (`<b>` then paints regular).
    let (regular_only, _) = run(vec![face("Host", 400, REGULAR)]);
    assert!(
        diff(&partial, &regular_only) > 0,
        "the host 700 face must actually be in the compared frames"
    );
}

/// (5) `PaintWorker` paints the same pixels as the direct engine with host
/// fonts (so the fonts reach the worker's own engine).
#[test]
fn paint_worker_matches_direct_engine_with_host_fonts() {
    let body = "<p style=\"font-family:Host\">Plain <b>Bold</b></p>";
    let direct = paint_with(host_pair(), body);
    // Since #56 a DejaVu-only engine paints the same DejaVu faces (the guard
    // only held while mixed weights in an unknown family painted one face;
    // Graph review of #61). Prove the host 700 face reaches the frame against
    // the host family with only its 400 face.
    let regular_only = paint_with(vec![face("Host", 400, REGULAR)], body);
    assert!(diff(&direct, &regular_only) > 0);

    let worker = PaintWorker::try_spawn(cfg(host_pair())).expect("valid fonts");
    worker.load_html(&page(body)).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, vp()).unwrap();
    worker.flush_timeout(Duration::from_secs(10)).unwrap();
    let frame = worker
        .latest_frame(SurfaceId::DEFAULT, 0)
        .expect("worker frame");
    let img = frame.output.rgba.as_ref().expect("worker rgba");
    assert_eq!(
        diff(img, &direct),
        0,
        "worker pixels must match the direct engine"
    );
}

/// (6) Registration order is deterministic: two families registered in
/// either order still resolve each name to its own face.
#[test]
fn registration_order_does_not_change_which_face_a_family_gets() {
    let ab = || vec![face("Alpha", 400, REGULAR), face("Beta", 400, BOLD)];
    let ba = || vec![face("Beta", 400, BOLD), face("Alpha", 400, REGULAR)];
    let alpha = "<p style=\"font-family:Alpha\">Glyphs</p>";
    let beta = "<p style=\"font-family:Beta\">Glyphs</p>";
    let alpha_ab = paint_with(ab(), alpha);
    let alpha_ba = paint_with(ba(), alpha);
    let beta_ab = paint_with(ab(), beta);
    let beta_ba = paint_with(ba(), beta);
    assert_eq!(
        diff(&alpha_ab, &alpha_ba),
        0,
        "Alpha must not depend on order"
    );
    assert_eq!(diff(&beta_ab, &beta_ba), 0, "Beta must not depend on order");
    assert!(
        diff(&alpha_ab, &beta_ab) > 0,
        "Alpha and Beta must resolve to different faces"
    );
    assert!(
        ink(&beta_ab) > ink(&alpha_ab),
        "Beta (bold bytes) must paint heavier than Alpha (regular bytes)"
    );
}

/// Truncation with `text-overflow: ellipsis` shapes the cut text with the
/// same host face as the document (the ellipsis shaper gets the host fonts).
#[test]
fn ellipsis_truncation_uses_the_host_face() {
    let div = |overflow: &str| {
        format!(
            "<div style=\"margin:8px;width:150px;white-space:nowrap;overflow:hidden;\
             text-overflow:{overflow};font-family:Host;font-size:24px\">Truncated host text</div>"
        )
    };
    let fonts = || vec![face("Host", 400, BOLD)];
    let host_ellipsis = paint_with(fonts(), &div("ellipsis"));
    let host_clip = paint_with(fonts(), &div("clip"));
    let dejavu_ellipsis = paint_with(Vec::new(), &div("ellipsis"));
    assert!(
        diff(&host_ellipsis, &host_clip) > 0,
        "the ellipsis path must actually run"
    );
    assert!(
        diff(&host_ellipsis, &dejavu_ellipsis) > 0,
        "truncated text must be shaped with the host face, not bundled DejaVu"
    );
    assert!(
        ink(&host_ellipsis) > ink(&dejavu_ellipsis),
        "the bold host face must paint heavier truncated text"
    );
}

/// Crate-owner decision (Graph review of #30, bold added by #56): a host face
/// that ties with a bundled "DejaVu Sans" face, 400 or 700 normal, is
/// rejected (the bundled faces count as earlier entries in the duplicate
/// check); the same family at another weight or style is allowed.
#[test]
fn host_face_tying_with_bundled_dejavu_is_rejected() {
    for family in ["DejaVu Sans", "dejavu sans", "  DEJAVU SANS "] {
        for weight in [400, 700] {
            match invalid(vec![face(family, weight, BOLD)]) {
                EngineError::InvalidFont { index, reason, .. } => {
                    assert_eq!(index, 0);
                    assert!(reason.contains("bundled"), "{family:?} {weight}: {reason}");
                }
                other => panic!("{family:?} {weight}: expected InvalidFont, got {other:?}"),
            }
        }
    }
    assert!(Engine::try_new(cfg(vec![face("DejaVu Sans", 600, BOLD)])).is_ok());
    assert!(Engine::try_new(cfg(vec![FontFace::new(
        "DejaVu Sans",
        400,
        FontStyle::Italic,
        Arc::<[u8]>::from(REGULAR),
    )]))
    .is_ok());
}

/// Build a two-face TrueType collection (`ttcf`) whose faces both point at
/// one copy of `ttf`'s table directory (table offsets shifted past the
/// collection header).
fn two_face_ttc(ttf: &[u8]) -> Vec<u8> {
    ttc(ttf, 2)
}

/// A TrueType collection (`ttcf`) of `n` faces, all pointing at one copy of
/// `ttf`'s table directory.
fn ttc(ttf: &[u8], n: u32) -> Vec<u8> {
    let header: u32 = 4 + 4 + 4 + 4 * n;
    let mut body = ttf.to_vec();
    let num_tables = u16::from_be_bytes([body[4], body[5]]) as usize;
    for i in 0..num_tables {
        let at = 12 + 16 * i + 8;
        let off = u32::from_be_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
        body[at..at + 4].copy_from_slice(&(off + header).to_be_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    for _ in 0..n {
        out.extend_from_slice(&header.to_be_bytes());
    }
    out.extend_from_slice(&body);
    out
}

/// Only single-face files are accepted: a font collection gives InvalidFont.
#[test]
fn font_collections_are_rejected() {
    let ttc: Arc<[u8]> = Arc::from(two_face_ttc(REGULAR));
    match invalid(vec![FontFace::new("Host", 400, FontStyle::Normal, ttc)]) {
        EngineError::InvalidFont { reason, .. } => {
            assert!(reason.contains("collection"), "{reason}")
        }
        other => panic!("expected InvalidFont, got {other:?}"),
    }
}

/// A collection is refused by its `ttcf` tag even when it holds one face.
#[test]
fn one_face_collection_is_rejected() {
    let ttc: Arc<[u8]> = Arc::from(ttc(REGULAR, 1));
    match invalid(vec![FontFace::new("Host", 400, FontStyle::Normal, ttc)]) {
        EngineError::InvalidFont { reason, .. } => {
            assert!(reason.contains("collection"), "{reason}")
        }
        other => panic!("expected InvalidFont, got {other:?}"),
    }
}

/// The duplicate check lowercases per character like fontique, so a
/// word-final capital sigma (`Σ` -> `σ`, not `ς`) still matches.
#[test]
fn duplicate_check_lowercases_like_fontique() {
    match invalid(vec![face("ΟΔΟΣ", 400, REGULAR), face("οδοσ", 400, BOLD)]) {
        EngineError::InvalidFont { index, reason, .. } => {
            assert_eq!(index, 1);
            assert!(reason.contains("duplicate"), "{reason}");
        }
        other => panic!("expected InvalidFont, got {other:?}"),
    }
}

/// A WOFF2-wrapped collection: the `ttcf` tag only appears after WOFF2
/// decoding, so the collection check must run on the decoded bytes
/// (Graph review of #32, mutant M3 "ttcf on raw bytes"). Fixture: a
/// one-face TrueType collection of a DejaVu Sans subset, WOFF2-compressed
/// (`tests/fonts/README.md`).
#[test]
fn woff2_collection_is_rejected() {
    const WOFF2_TTC: &[u8] = include_bytes!("fonts/DejaVuSans-subset-collection1.woff2");
    assert_eq!(&WOFF2_TTC[..4], b"wOF2");
    assert_eq!(
        &WOFF2_TTC[4..8],
        b"ttcf",
        "WOFF2 flavor must be a collection"
    );
    match invalid(vec![FontFace::new(
        "Host",
        400,
        FontStyle::Normal,
        Arc::<[u8]>::from(WOFF2_TTC),
    )]) {
        EngineError::InvalidFont { index, reason, .. } => {
            assert_eq!(index, 0);
            assert!(reason.contains("collection"), "{reason}");
        }
        other => panic!("expected InvalidFont, got {other:?}"),
    }
}

/// Issue #56: with no host fonts, `font-weight:700` paints the bundled
/// DejaVu Sans Bold, not the regular face: in an unknown family (Mainnet's
/// "MS Sans Serif" stack today), in `sans-serif`, and in "DejaVu Sans".
#[test]
fn bold_without_host_fonts_paints_bundled_bold() {
    for fam in [
        "\"MS Sans Serif\",Arial,sans-serif",
        "sans-serif",
        "\"DejaVu Sans\"",
    ] {
        let regular = paint_with(vec![], &format!("<p style='font-family:{fam}'>x</p>"));
        let bold = paint_with(
            vec![],
            &format!("<p style='font-family:{fam};font-weight:700'>x</p>"),
        );
        let (r, b) = (ink(&regular), ink(&bold));
        println!("{fam}: ink 400 {r}, 700 {b}");
        assert!(r > 0, "{fam}: regular must paint");
        assert!(b > r, "{fam}: 700 must paint heavier than 400 ({b} vs {r})");
    }
    // The bundled bold is the same face a host registering DejaVu Sans Bold
    // under its own family gets: identical pixels.
    let bundled = paint_with(vec![], "<p style='font-weight:700'>x</p>");
    let host = paint_with(
        vec![face("Host", 700, BOLD)],
        "<p style='font-family:Host;font-weight:700'>x</p>",
    );
    assert_eq!(diff(&bundled, &host), 0, "bundled bold is DejaVu Sans Bold");
}

/// Graph review of #61: a paragraph that mixes 400 and 700 in a family list
/// with no generic and no registered family paints each run in its own
/// bundled face: the same pixels as DejaVu regular + bold registered as a
/// host family. Before the script fallback, parley's last-resort face (one
/// per paragraph, picked without regard to weight) painted every run.
#[test]
fn mixed_weights_in_an_unknown_family_paint_their_own_faces() {
    let real = || vec![face("Ref", 400, REGULAR), face("Ref", 700, BOLD)];
    for inner in [
        "Plain <b>Bold</b>",
        "<b>Bold</b> Plain",
        "<b>Exit</b> this <b>lease?</b>",
    ] {
        let want = paint_with(real(), &format!("<p style=\"font-family:Ref\">{inner}</p>"));
        for fam in ["Host", "'MS Sans Serif'", "'MS Sans Serif',Arial"] {
            let got = paint_with(
                Vec::new(),
                &format!("<p style=\"font-family:{fam}\">{inner}</p>"),
            );
            assert_eq!(diff(&want, &got), 0, "{fam}: {inner}");
        }
    }
}
