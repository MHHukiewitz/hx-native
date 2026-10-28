//! #60 review, blocking item 4: a page with no `text-shadow` anywhere skips
//! the shadow table and the damage-pad walk, and a swap that brings a shadow
//! in (or takes it out again) switches the full path back on (or off) with
//! partial == full.

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
struct Frag(String);
impl FragmentSource for Frag {
    fn render(&mut self, _section: &str, _slices: &[&str]) -> String {
        self.0.clone()
    }
}

fn page() -> String {
    let mut rows = String::new();
    for i in 0..20 {
        rows.push_str(&format!("<p style='margin:0'>Row {i} <b>bold</b> text</p>"));
    }
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff;color:#000}}\
         body{{font-family:\"DejaVu Sans\";font-size:14px}}#sec{{position:absolute;left:200px;top:40px}}\
         </style></head><body>{rows}<section id=\"sec\" hx-trigger=\"slice:t\" hx-target=\"#sec\" \
         hx-swap=\"innerHTML\">a</section></body></html>"
    )
}

fn engine() -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 10_000_000_000,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    e.load_html(&page()).unwrap();
    e
}

fn reddish(p: &[u8]) -> bool {
    p[0] > 160 && p[1] < 90 && p[2] < 90
}

/// Full repaint of `#sec` = `html` on a fresh engine.
fn full_of(html: &str, view: &Viewport) -> RgbaImage {
    let mut e = engine();
    e.paint(view.clone()).unwrap();
    let mut s = Slices(HashMap::from([("t".to_string(), 2)]));
    e.sync(&s, &mut Frag(html.into())).unwrap();
    s.0.insert("t".into(), 3);
    let _ = s;
    e.mark_theme_or_stylesheet_changed();
    let out = e.paint(view.clone()).unwrap();
    out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone())
}

#[test]
fn no_shadow_page_takes_the_fast_path_until_a_swap_brings_one_in() {
    for scale in [1.0, 2.0] {
        let view = Viewport::with_scale((400.0 * scale) as u32, (300.0 * scale) as u32, scale);
        let mut e = engine();
        e.paint(view.clone()).unwrap();
        let mut slices = Slices(HashMap::from([("t".to_string(), 1)]));
        let steps = [
            ("w1", false),
            (
                "<b style=\"text-shadow:3px -9px #f00\">w2 shadowed</b>",
                true,
            ),
            ("w3", false),
        ];
        for (i, (html, shadowed)) in steps.into_iter().enumerate() {
            let _ = take_text_shadow_counters_for_test();
            slices.0.insert("t".into(), 2 + i as u64);
            e.sync(&slices, &mut Frag(html.into())).unwrap();
            let out = e.paint(view.clone()).unwrap();
            let c = take_text_shadow_counters_for_test();
            assert!(
                !e.last_paint_stats().full_repaint,
                "s{scale} {html}: partial"
            );
            if shadowed {
                assert_eq!(
                    (c.tables_skipped, c.pads_skipped),
                    (0, 0),
                    "s{scale} {html}: {c:?}"
                );
                assert!(
                    c.tables_built >= 1 && c.pads_walked >= 1,
                    "s{scale} {html}: {c:?}"
                );
            } else {
                assert_eq!(
                    (c.tables_built, c.pads_walked),
                    (0, 0),
                    "s{scale} {html}: {c:?}"
                );
                assert!(
                    c.tables_skipped >= 1 && c.pads_skipped >= 1,
                    "s{scale} {html}: {c:?}"
                );
            }
            let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
            let full = full_of(html, &view);
            let red = full.pixels.chunks_exact(4).filter(|p| reddish(p)).count();
            assert_eq!(red > 0, shadowed, "s{scale} {html}: red {red}");
            let diff = partial
                .pixels
                .chunks_exact(4)
                .zip(full.pixels.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(diff, 0, "s{scale} {html}: partial vs full");
        }
    }
}
