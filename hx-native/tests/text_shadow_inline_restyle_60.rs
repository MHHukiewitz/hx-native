//! #60 review (Graph, comment 6332259690), probes P7, P10 and P15: an inline
//! element without a box of its own (an `<a hx-post>` inside a `<p>` or an
//! anonymous block) gains, loses or changes a `text-shadow` when greying
//! flips `aria-disabled`. Its runs are laid out by the inline root, so the
//! damage must cover the root's (shadowed) text, and partial must equal full.

use std::collections::HashMap;

use hx_native::*;

fn cfg(mode: PaintMode) -> EngineConfig {
    EngineConfig {
        max_paint_ns: 10_000_000_000,
        pending_timeout_frames: 5,
        paint_mode: mode,
        ..EngineConfig::default()
    }
}

struct Slices {
    v: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
}
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.v.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
}
struct NoFrags;
impl FragmentSource for NoFrags {
    fn render(&mut self, _section: &str, _slices: &[&str]) -> String {
        String::new()
    }
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    [
        img.pixels[i],
        img.pixels[i + 1],
        img.pixels[i + 2],
        img.pixels[i + 3],
    ]
}

fn reddish(p: [u8; 4]) -> bool {
    p[0] > 160 && p[1] < 90 && p[2] < 90
}

fn diffs(a: &RgbaImage, b: &RgbaImage, tol: u8) -> usize {
    a.pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > tol))
        .count()
}

fn page(css: &str, markup: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#c0c0c0;color:#000}}\
         body{{font-family:\"DejaVu Sans\";font-size:20px}}{css}</style></head><body>{markup}</body></html>"
    )
}

const IN_P: &str = "<p style=\"position:absolute;left:80px;top:40px;margin:0\">Text <a id=g hx-post=\"x.go\" hx-trigger=\"click\">Confirm</a> tail</p>";
const IN_ANON: &str = "<div style=\"position:absolute;left:80px;top:40px\"><div>x</div>Text <a id=g hx-post=\"x.go\" hx-trigger=\"click\">Confirm</a> tail</div>";
const IN_B_IN_P: &str = "<p style=\"position:absolute;left:80px;top:40px;margin:0\">Text <b>bold <a id=g hx-post=\"x.go\" hx-trigger=\"click\">Confirm</a></b> tail</p>";
const BLOCK: &str = "<button id=g hx-post=\"x.go\" hx-trigger=\"click\" style=\"position:absolute;left:80px;top:40px\">Confirm</button>";
const MIXED_BUTTON: &str = "<button id=g hx-post=\"x.go\" hx-trigger=\"click\" style=\"position:absolute;left:80px;top:40px\">Con<div>firm</div>ok</button>";

struct Flip {
    first: RgbaImage,
    partial: RgbaImage,
    full: RgbaImage,
    was_partial: bool,
    damage: Vec<Rect>,
}

/// Paint with `x.go` in state `from`, flip it to `to`, paint (partial),
/// then force a full repaint of the same state.
fn flip(html: &str, mode: PaintMode, scale: f64, to_disabled: bool) -> Flip {
    let view = Viewport::with_scale((400.0 * scale) as u32, (200.0 * scale) as u32, scale);
    let dis = ActionState::Disabled {
        reason: "no".into(),
    };
    let (from, to) = if to_disabled {
        (ActionState::Enabled, dis)
    } else {
        (dis, ActionState::Enabled)
    };
    let mut e = Engine::new(cfg(mode));
    e.declare_actions(&["x.go"]);
    e.load_html(html).unwrap();
    let mut s = Slices {
        v: HashMap::from([("actions".to_string(), 1)]),
        actions: vec![("x.go".into(), from)],
    };
    e.sync(&s, &mut NoFrags).unwrap();
    let first = e.paint(view.clone()).unwrap().rgba.unwrap();
    s.v.insert("actions".into(), 2);
    s.actions = vec![("x.go".into(), to)];
    e.sync(&s, &mut NoFrags).unwrap();
    let out = e.paint(view.clone()).unwrap();
    let was_partial = !e.last_paint_stats().full_repaint;
    let damage = out.damage.clone();
    let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
    e.mark_theme_or_stylesheet_changed();
    let f = e.paint(view).unwrap();
    let full = f.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
    Flip {
        first,
        partial,
        full,
        was_partial,
        damage,
    }
}

/// P7: the kit's greyed rule (colour and a large shadow) on a block button,
/// an inline `<a>` in a paragraph, an inline in an anonymous block and a
/// button with mixed content, both directions, Exact at scale 1 and 2.
///
/// Exact only: Shifted partial paints are wrong on main for other reasons
/// (#63: culling against the unshifted clip, brush shifted twice).
#[test]
fn greying_that_adds_or_removes_a_shadow_repaints_like_a_full_paint() {
    let css = "[aria-disabled=true]{color:#808080;text-shadow:-40px 30px #f00}";
    let mut bad = vec![];
    for (label, m) in [
        ("block", BLOCK),
        ("inline-in-p", IN_P),
        ("inline-in-anon", IN_ANON),
        ("mixed-button", MIXED_BUTTON),
    ] {
        for scale in [1.0, 2.0] {
            for to_disabled in [true, false] {
                let r = flip(&page(css, m), PaintMode::Exact, scale, to_disabled);
                let d = diffs(&r.partial, &r.full, 0);
                let red = r
                    .full
                    .pixels
                    .chunks_exact(4)
                    .filter(|p| reddish([p[0], p[1], p[2], p[3]]))
                    .count();
                if d != 0 || !r.was_partial || (to_disabled && red == 0) {
                    bad.push(format!(
                        "{label} s{scale} to_disabled={to_disabled}: {d} px differ, partial {}, red {red}, damage {:?}",
                        r.was_partial, r.damage
                    ));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// P10: an inline `<a>` greyed with a colour-only change, a large shadow and
/// a 1 px shadow, in a paragraph, in an anonymous block and nested in an
/// inline `<b>` (whose own box does not paint the text either).
#[test]
fn inline_link_greying_colour_or_shadow_repaints_like_a_full_paint() {
    let mut bad = vec![];
    for (label, css) in [
        ("colour-only", "[aria-disabled=true]{color:#f00}"),
        (
            "shadow",
            "[aria-disabled=true]{text-shadow:-40px 30px #f00}",
        ),
        (
            "shadow-small",
            "[aria-disabled=true]{text-shadow:1px 1px #f00}",
        ),
        (
            "colour+shadow",
            "[aria-disabled=true]{color:#808080;text-shadow:1px 1px #f00}",
        ),
    ] {
        for (ml, m) in [
            ("in-p", IN_P),
            ("in-anon", IN_ANON),
            ("in-b-in-p", IN_B_IN_P),
        ] {
            for scale in [1.0, 2.0] {
                let r = flip(&page(css, m), PaintMode::Exact, scale, true);
                let d = diffs(&r.partial, &r.full, 0);
                if d != 0 || !r.was_partial {
                    bad.push(format!(
                        "{label} {ml} s{scale}: {d} px differ, partial {}, damage {:?}",
                        r.was_partial, r.damage
                    ));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// P15: every pixel the flip changes lies inside the reported damage (the
/// 1 px shadow under "Confirm", x 128–204 at scale 1), and the damage is
/// still local rather than the whole surface.
#[test]
fn inline_link_shadow_damage_covers_the_shadowed_text() {
    let css = "[aria-disabled=true]{text-shadow:1px 1px #f00}";
    for scale in [1.0, 2.0] {
        let r = flip(&page(css, IN_P), PaintMode::Exact, scale, true);
        assert!(r.was_partial, "s{scale}");
        let inside = |x: u32, y: u32| {
            r.damage.iter().any(|d| {
                (x as f32) >= d.x
                    && (x as f32) < d.x + d.width
                    && (y as f32) >= d.y
                    && (y as f32) < d.y + d.height
            })
        };
        let mut changed = 0;
        let mut outside = vec![];
        for y in 0..r.full.height {
            for x in 0..r.full.width {
                if px(&r.first, x, y) != px(&r.full, x, y) {
                    changed += 1;
                    if !inside(x, y) && outside.len() < 5 {
                        outside.push((x, y));
                    }
                }
            }
        }
        assert!(changed > 0, "s{scale}: the shadow must change pixels");
        assert!(
            outside.is_empty(),
            "s{scale}: changed px outside damage {:?}: {outside:?}",
            r.damage
        );
        let area: f32 = r.damage.iter().map(|d| d.width * d.height).sum();
        let surface = (r.full.width * r.full.height) as f32;
        assert!(
            area < surface / 4.0,
            "s{scale}: damage stays local: {:?}",
            r.damage
        );
        assert_eq!(diffs(&r.partial, &r.full, 0), 0, "s{scale}");
    }
}
