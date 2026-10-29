//! #52 (Graph's HOLD at 122ee963), follow-up to `svg_swap_context_52.rs`:
//! a swap fragment is validated in the context the swap parses it in, with
//! the target's real `<svg>` ancestry, so the #23 SVG gate runs on it as SVG
//! for every swap style the dialect has (`innerHTML`, `outerHTML`; every
//! other `hx-swap` value is refused at load). No SVG path may load a file:
//! an `<image>` (or any element) href that is an absolute path, a relative
//! path or a `file://` URL is refused, the previous DOM stays, and nothing
//! from disk is painted.
use std::collections::HashMap;

use hx_native::*;

struct Slices(HashMap<String, u64>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}

struct Frags(String);
impl FragmentSource for Frags {
    fn render(&mut self, _section: &str, _slices: &[&str]) -> String {
        self.0.clone()
    }
}

const SWAPS: [&str; 2] = ["innerHTML", "outerHTML"];
const RECT: &str = "<rect width=\"16\" height=\"16\" fill=\"#000\"/>";

fn png_abs() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/img/red16.png").to_string()
}

/// `body` holds an element `#b` (the swap target); a section swaps into it.
fn shell(body: &str, swap: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}</style></head>\
         <body>{body}\
         <section id=\"s\" hx-trigger=\"slice:x\" hx-target=\"#b\" hx-swap=\"{swap}\"></section>\
         </body></html>"
    )
}

fn svg_body(inner_of_b: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">\
         <g id=\"b\">{inner_of_b}</g></svg>"
    )
}

/// The fragment `inner` as the swap renders it: as is for `innerHTML`,
/// wrapped in a fresh `<g id="b">` for `outerHTML`.
fn frag(swap: &str, inner: &str) -> String {
    if swap == "outerHTML" {
        format!("<g id=\"b\">{inner}</g>")
    } else {
        inner.to_string()
    }
}

struct Run {
    engine: Engine,
    slices: Slices,
}

impl Run {
    /// Load `body`, then a first sync that swaps `first` into `#b`.
    fn new(body: &str, swap: &str, first: &str) -> Run {
        let mut engine = Engine::new(EngineConfig {
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        engine.load_html(&shell(body, swap)).unwrap();
        let slices = Slices(HashMap::from([("x".to_string(), 1)]));
        engine.sync(&slices, &mut Frags(first.to_string())).unwrap();
        Run { engine, slices }
    }

    fn swap(&mut self, html: &str) -> Result<(), EngineError> {
        let v = self.slices.0["x"] + 1;
        self.slices.0.insert("x".into(), v);
        self.engine
            .sync(&self.slices, &mut Frags(html.to_string()))
            .map(|_| ())
    }

    fn pixels(&mut self, want: [u8; 3]) -> usize {
        // A paint with nothing new returns no image; read the last frame.
        self.engine
            .paint(Viewport::with_scale(16, 16, 1.0))
            .unwrap();
        let img = self.engine.last_frame().expect("a frame was painted");
        img.pixels
            .chunks(4)
            .filter(|p| p[..3].iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 8))
            .count()
    }
}

fn want_invalid_svg(r: Result<(), EngineError>, what: &str) {
    assert!(
        matches!(r, Err(EngineError::InvalidSvg { .. })),
        "{what}: want InvalidSvg, got {r:?}"
    );
}

/// Every href that is not `data:` names a file: absolute, relative and
/// `file://`, as `href` and `xlink:href`, on `<image>` and on other elements.
fn file_hrefs() -> Vec<String> {
    let abs = png_abs();
    let mut out = Vec::new();
    for href in [
        abs.clone(),
        "tests/fixtures/img/red16.png".to_string(),
        "red16.png".to_string(),
        format!("file://{abs}"),
    ] {
        out.push(format!(
            "<image href=\"{href}\" width=\"16\" height=\"16\"/>"
        ));
        out.push(format!(
            "<image xlink:href=\"{href}\" width=\"16\" height=\"16\"/>"
        ));
        out.push(format!("<a href=\"{href}\">{RECT}</a>"));
    }
    out
}

#[test]
fn every_swap_style_into_svg_refuses_forbidden_content() {
    let mut cases: Vec<String> = vec![
        "<text x=\"2\" y=\"12\">HHH</text>".into(),
        "<tspan>H</tspan>".into(),
        "<marker id=\"m\"><rect width=\"2\" height=\"2\"/></marker>".into(),
        "<mask id=\"k\"></mask>".into(),
        "<pattern id=\"p\"></pattern>".into(),
        "<use href=\"#b\"/>".into(),
        "<path d=\"M0 5L16 5\" stroke=\"#000\" stroke-dasharray=\"2\"/>".into(),
        "<path d=\"M0 5L16 5\" stroke=\"#000\" style=\"stroke-dashoffset:1\"/>".into(),
        "<style>path{stroke-dasharray:1}</style>".into(),
        // Nested inside an allowed element of the fragment.
        "<g><g><text>H</text></g></g>".into(),
    ];
    cases.extend(file_hrefs());
    for swap in SWAPS {
        for inner in &cases {
            let mut run = Run::new(&svg_body(""), swap, &frag(swap, ""));
            want_invalid_svg(run.swap(&frag(swap, inner)), &format!("{swap} {inner}"));
        }
    }
}

#[test]
fn a_refused_file_image_leaves_the_dom_and_paints_nothing_from_disk() {
    for swap in SWAPS {
        for img in file_hrefs() {
            let mut run = Run::new(&svg_body(""), swap, &frag(swap, RECT));
            let before = run.engine.serialize_html().unwrap();
            assert!(run.pixels([0, 0, 0]) > 200, "{swap}: the first rect paints");
            want_invalid_svg(run.swap(&frag(swap, &img)), &format!("{swap} {img}"));
            assert_eq!(
                run.engine.serialize_html().unwrap(),
                before,
                "{swap} {img}: the refused swap changed the DOM"
            );
            assert_eq!(
                run.pixels([255, 0, 0]),
                0,
                "{swap} {img}: red from disk painted"
            );
            assert!(
                run.pixels([0, 0, 0]) > 200,
                "{swap} {img}: old rect still paints"
            );
        }
    }
}

#[test]
fn benign_swaps_into_svg_apply_and_paint() {
    for swap in SWAPS {
        let mut run = Run::new(&svg_body(""), swap, &frag(swap, ""));
        assert_eq!(run.pixels([0, 0, 0]), 0);
        run.swap(&frag(swap, RECT)).unwrap();
        assert!(run.pixels([0, 0, 0]) > 200, "{swap}: swapped rect paints");
    }
}

#[test]
fn a_target_deep_in_svg_or_the_svg_itself_is_gated() {
    let text = "<text>H</text>";
    // Target nested several groups down.
    let deep = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">\
                <g><g><g id=\"b\"></g></g></g></svg>";
    for swap in SWAPS {
        let mut run = Run::new(deep, swap, &frag(swap, ""));
        want_invalid_svg(run.swap(&frag(swap, text)), &format!("deep {swap}"));
    }
    // innerHTML straight into the <svg> element.
    let svg_target =
        "<svg id=\"b\" xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"></svg>";
    let mut run = Run::new(svg_target, "innerHTML", "");
    want_invalid_svg(run.swap(text), "innerHTML into <svg>");
    // outerHTML replacing the <svg> itself: the context is HTML, and the
    // fragment's own <svg> carries the gate.
    let replace = |inner: &str| {
        format!(
            "<svg id=\"b\" xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">{inner}</svg>"
        )
    };
    let mut run = Run::new(svg_target, "outerHTML", &replace(""));
    want_invalid_svg(run.swap(&replace(text)), "outerHTML of <svg>");
    let img = format!("<image href=\"{}\" width=\"16\" height=\"16\"/>", png_abs());
    want_invalid_svg(run.swap(&replace(&img)), "outerHTML of <svg>, file image");
    run.swap(&replace(RECT)).unwrap();
}

#[test]
fn a_swap_into_a_clip_path_uses_the_clip_path_rules() {
    let body = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">\
                <defs><clipPath id=\"b\"></clipPath></defs></svg>";
    let mut run = Run::new(body, "innerHTML", "");
    // `clip-path` is refused only inside a <clipPath> (no nesting fan-out).
    want_invalid_svg(
        run.swap("<rect width=\"4\" height=\"4\" clip-path=\"url(#b)\"/>"),
        "clip-path inside clipPath",
    );
    run.swap("<rect width=\"4\" height=\"4\"/>").unwrap();
}

#[test]
fn svg_depth_counts_the_target_ancestry() {
    // `#b` sits at SVG depth SVG_MAX_DEPTH - 1 (the <svg> is depth 1); a
    // fragment two elements deep pushes its innermost element past the cap.
    let open: String = "<g>".repeat(SVG_MAX_DEPTH - 3);
    let close: String = "</g>".repeat(SVG_MAX_DEPTH - 3);
    let body = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">{open}<g id=\"b\"></g>{close}</svg>"
    );
    let mut run = Run::new(&body, "innerHTML", "");
    run.swap("<rect width=\"1\" height=\"1\"/>").unwrap();
    let r = run.swap("<g><rect width=\"1\" height=\"1\"/></g>");
    assert!(
        matches!(
            r,
            Err(EngineError::InvalidSvg {
                rejection: SvgRejection::TooDeep { .. },
                ..
            })
        ),
        "want TooDeep, got {r:?}"
    );
}

#[test]
fn html_targets_keep_html_parsing() {
    // Outside <svg>, `<text>` is an unknown HTML element, not SVG text.
    let mut run = Run::new("<div id=\"b\"></div>", "innerHTML", "");
    run.swap("<text>plain</text>").unwrap();
    // A fragment that brings its own <svg> is still gated.
    want_invalid_svg(
        run.swap("<svg xmlns=\"http://www.w3.org/2000/svg\"><text>H</text></svg>"),
        "own <svg> in an HTML target",
    );
}

#[test]
fn other_swap_styles_are_refused_at_load() {
    for swap in [
        "beforeend",
        "afterbegin",
        "beforebegin",
        "afterend",
        "delete",
        "none",
    ] {
        let r = Engine::new(EngineConfig::default()).load_html(&shell(&svg_body(""), swap));
        assert!(
            matches!(r, Err(EngineError::UnsupportedSwap { .. })),
            "{swap}: want UnsupportedSwap, got {r:?}"
        );
    }
}
