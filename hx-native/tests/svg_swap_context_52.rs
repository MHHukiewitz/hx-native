//! Graph review of #52 at 122ee963: a swap fragment is validated as its own
//! document, but it is applied in the target's context. A target inside an
//! inline `<svg>` turns the fragment into SVG content, so the #23 gate must
//! run on it as SVG: forbidden elements, forbidden properties and non-`data:`
//! hrefs are refused, and the previous DOM stays in place.
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

fn shell(swap: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}</style></head>\
         <body><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"30\"><g id=\"b\"></g></svg>\
         <section id=\"s\" hx-trigger=\"slice:x\" hx-target=\"#b\" hx-swap=\"{swap}\"><span>s</span></section>\
         </body></html>"
    )
}

/// The second sync's result: the swap of `frag` into `#b` with `swap`.
fn swap_into_svg(swap: &str, frag: &str) -> Result<(), EngineError> {
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.load_html(&shell(swap)).unwrap();
    let mut sl = Slices(HashMap::from([("x".to_string(), 1)]));
    let first = if swap == "innerHTML" {
        String::new()
    } else {
        "<g id=\"b\"></g>".to_string()
    };
    e.sync(&sl, &mut Frags(first)).unwrap();
    sl.0.insert("x".into(), 2);
    e.sync(&sl, &mut Frags(frag.to_string())).map(|_| ())
}

#[test]
fn swap_into_inline_svg_runs_the_svg_gate() {
    let png = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/img/red16.png");
    let image = format!("<image href=\"{png}\" width=\"16\" height=\"16\"/>");
    let text = "<text x=\"2\" y=\"20\">HHH</text>";
    let dash = "<path d=\"M0 5L90 5\" stroke=\"#000\" stroke-dasharray=\"2\"/>";
    let marker = "<marker id=\"m\"><rect width=\"2\" height=\"2\"/></marker>";
    let cases = [
        ("innerHTML", text.to_string()),
        ("innerHTML", image.clone()),
        ("innerHTML", dash.to_string()),
        ("innerHTML", marker.to_string()),
        ("outerHTML", format!("<g id=\"b\">{text}</g>")),
        ("outerHTML", format!("<g id=\"b\">{image}</g>")),
    ];
    for (swap, frag) in cases {
        let r = swap_into_svg(swap, &frag);
        assert!(
            matches!(r, Err(EngineError::InvalidSvg { .. })),
            "{swap} {frag}: want InvalidSvg, got {r:?}"
        );
    }
}

#[test]
fn benign_svg_swap_still_applies() {
    let rect = "<rect width=\"10\" height=\"10\" fill=\"#000\"/>";
    swap_into_svg("innerHTML", rect).unwrap();
    swap_into_svg("outerHTML", &format!("<g id=\"b\">{rect}</g>")).unwrap();
}
