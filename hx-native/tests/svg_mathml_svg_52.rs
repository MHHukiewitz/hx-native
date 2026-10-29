//! Graph review of #52 at f63de6e. Blitz renders every element whose local
//! name is `svg` as an inline SVG image, whatever its namespace, and hands
//! its subtree to usvg by local name. Inside MathML (outside the text
//! integration points) the HTML parser makes `<svg>` a MathML element, so
//! the #23 gate, which only looked for SVG-namespace ancestors, skipped it
//! and an `<image>` there read a file from disk and painted it. The gate must
//! treat such an `<svg>` as inline SVG, on load and on every swap style.
//!
//! The last test pins the outerHTML parse context of the swap check: a
//! `<foreignObject>` target is replaced in its parent `<g>`, so the fragment
//! is SVG there and an `<image>` must pass the SVG `<image>` rules.
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

const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">";

fn png() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/img/red16.png")
}

fn image() -> String {
    format!("<image href=\"{}\" width=\"16\" height=\"16\"/>", png())
}

fn page(body: &str, swap: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}</style></head>\
         <body>{body}\
         <section id=\"s\" hx-trigger=\"slice:x\" hx-target=\"#b\" hx-swap=\"{swap}\"></section>\
         </body></html>"
    )
}

fn red_px(e: &mut Engine) -> usize {
    let out = e.paint(Viewport::with_scale(64, 64, 1.0)).unwrap();
    let img = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
    img.pixels
        .chunks(4)
        .filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60)
        .count()
}

/// Load `body`, run a first sync with `first`, then swap `frag` into `#b`.
/// Returns the second sync's result and the red pixels painted after it.
fn swap(body: &str, swap: &str, first: &str, frag: &str) -> (Result<(), EngineError>, usize) {
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.load_html(&page(body, swap)).unwrap();
    let mut sl = Slices(HashMap::from([("x".to_string(), 1)]));
    e.sync(&sl, &mut Frags(first.to_string())).unwrap();
    red_px(&mut e);
    sl.0.insert("x".into(), 2);
    let r = e.sync(&sl, &mut Frags(frag.to_string())).map(|_| ());
    (r, red_px(&mut e))
}

fn is_invalid_svg<T: std::fmt::Debug>(r: &Result<T, EngineError>) -> bool {
    matches!(r, Err(EngineError::InvalidSvg { .. }))
}

#[test]
fn an_svg_inside_mathml_is_gated_on_load() {
    let cases = [
        format!("<math>{SVG}{}</svg></math>", image()),
        format!("<math><mrow>{SVG}<text>HHH</text></svg></mrow></math>"),
        format!(
            "<math><annotation-xml encoding=\"text/html\"><a>{SVG}{}</a></annotation-xml></math>",
            image()
        ),
    ];
    for body in cases {
        let mut e = Engine::new(EngineConfig::default());
        let r = e.load_html(&page(&format!("{body}<div id=\"b\"></div>"), "innerHTML"));
        assert!(is_invalid_svg(&r), "{body}: want InvalidSvg, got {r:?}");
    }
}

#[test]
fn a_swap_that_makes_an_svg_inside_mathml_is_gated() {
    let frag = format!("{SVG}{}</svg>", image());
    let cases = [
        (
            "<math><mrow id=\"b\"></mrow></math>",
            "innerHTML",
            "",
            frag.clone(),
        ),
        (
            "<math><mrow><mrow id=\"b\"></mrow></mrow></math>",
            "outerHTML",
            "<mrow id=\"b\"></mrow>",
            format!("<mrow id=\"b\">{frag}</mrow>"),
        ),
        (
            "<math><annotation-xml id=\"b\" encoding=\"text/html\"></annotation-xml></math>",
            "innerHTML",
            "",
            format!("<a>{frag}</a>"),
        ),
    ];
    for (body, style, first, frag) in cases {
        let (r, red) = swap(body, style, first, &frag);
        assert!(
            is_invalid_svg(&r),
            "{body} {style}: want InvalidSvg, got {r:?}"
        );
        assert_eq!(red, 0, "{body} {style}: painted a file from disk");
    }
}

#[test]
fn plain_mathml_still_loads_and_swaps() {
    let mut e = Engine::new(EngineConfig::default());
    e.load_html(&page(
        "<math><mi>x</mi></math><div id=\"b\"></div>",
        "innerHTML",
    ))
    .unwrap();
    let (r, _) = swap(
        "<math><mrow id=\"b\"></mrow></math>",
        "innerHTML",
        "",
        "<mi>y</mi><mo>+</mo>",
    );
    r.unwrap();
}

#[test]
fn an_outer_swap_of_a_foreign_object_is_checked_in_its_svg_parent() {
    let body = format!(
        "{SVG}<g><foreignObject id=\"b\" width=\"16\" height=\"16\"></foreignObject></g></svg>"
    );
    let first = "<foreignObject id=\"b\" width=\"16\" height=\"16\"></foreignObject>";
    // In the `<g>` this is an SVG `<image>`, and its href is not an allowed
    // raster. Parsed in the `<foreignObject>` it would be an HTML `<img>`.
    let (r, _) = swap(
        &body,
        "outerHTML",
        first,
        "<image href=\"data:text/plain,x\"/>",
    );
    assert!(is_invalid_svg(&r), "want InvalidSvg, got {r:?}");
}
