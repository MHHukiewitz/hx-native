//! Mainnet #781 (head 89b3edb7) Deals padlock buttons: an inline `<svg>` in
//! the HTML fragment (not a `data:` image). Markup verbatim from
//! `templates/hx_deals/items_talk.html` (open lock) and
//! `docs/design/mocks/target/28-deals-bargaining.html` (fixed, disabled
//! lock; its engine-written `data-hx-reason`, which loaded HTML may not
//! carry, is dropped). The `.lock` rules are carrier98.css lines 196-199, with the button
//! chrome reduced to a flat #c0c0c0 face so the icon pixels can be checked
//! exactly; the wrappers are flex so no line box shifts the button. Features: `viewBox`, `shape-rendering="crispEdges"`,
//! `fill="currentColor"`, integer `h`/`v` paths and one `fill="#c0c0c0"`
//! hole; nothing #52 refuses.

use std::collections::HashMap;

use hx_native::*;

const OPEN_LOCK: &str = r##"<button type="button" id="deals-lock-line_access" class="lock open" aria-label="Open: they may change this in a counter offer. Click to lock." hx-post="deals.item.lock.line_access" hx-trigger="click" hx-sync="drop" hx-indicator="#deals-ind"><svg width="12" height="12" viewBox="0 0 12 12" shape-rendering="crispEdges" fill="currentColor"><path d="M6 0h4v1H6zM5 1h1v3H5zM10 1h1v2h-1z"/><path d="M0 4h8v7H0z"/><path fill="#c0c0c0" d="M3 6h2v2H3z"/></svg></button>"##;

const FIXED_LOCK: &str = r##"<button type="button" id="deals-lock-customers_by_date" class="lock fixed" disabled aria-disabled="true" title="Locked: they added this. Change it by walking away or accepting." aria-label="Locked: they added this. Change it by walking away or accepting." hx-post="deals.item.lock.customers_by_date" hx-trigger="click" hx-sync="drop" hx-indicator="#deals-ind"><svg width="12" height="12" viewBox="0 0 12 12" shape-rendering="crispEdges" fill="currentColor"><path d="M3 1h4v1H3zM2 2h1v3H2zM7 2h1v3H7z"/><path d="M1 5h8v6H1z"/><path fill="#c0c0c0" d="M4 7h2v2H4z"/></svg></button>"##;

/// Ink rects (x0, y0, x1, y1) in the 12x12 viewBox, then the face-coloured hole.
const OPEN_INK: &[(u32, u32, u32, u32)] =
    &[(6, 0, 10, 1), (5, 1, 6, 4), (10, 1, 11, 3), (0, 4, 8, 11)];
const OPEN_HOLE: (u32, u32, u32, u32) = (3, 6, 5, 8);
const FIXED_INK: &[(u32, u32, u32, u32)] =
    &[(3, 1, 7, 2), (2, 2, 3, 5), (7, 2, 8, 5), (1, 5, 9, 11)];
const FIXED_HOLE: (u32, u32, u32, u32) = (4, 7, 6, 9);

const FACE: [u8; 3] = [0xc0, 0xc0, 0xc0];
const BLACK: [u8; 3] = [0, 0, 0];
const DIS_INK: [u8; 3] = [0x80, 0x80, 0x80];

const CSS: &str = "html,body{margin:0;background:#c0c0c0}
button{border:0;border-radius:0;background:#c0c0c0;box-shadow:none;padding:0;color:#000}
button:disabled{color:#808080}
.lock{display:inline-flex;align-items:center;justify-content:center;width:18px;height:16px;min-height:16px;min-width:0;padding:0;gap:0;color:#000}
.lock svg{display:block}
.lock.fixed{box-shadow:none;background:#c0c0c0}
.lock[disabled]{color:#808080}
#deals-ind{position:absolute;left:0;top:0;width:1px;height:1px}
#a{position:absolute;left:10px;top:10px;display:flex}
#b{position:absolute;left:40px;top:10px;display:flex}";

fn engine() -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 200_000_000,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    e.declare_actions(&[
        "deals.item.lock.line_access",
        "deals.item.lock.customers_by_date",
    ]);
    e
}

fn vp(scale: f64) -> Viewport {
    Viewport::with_scale((80.0 * scale) as u32, (40.0 * scale) as u32, scale)
}

fn page(body: &str) -> String {
    format!("<!DOCTYPE html><html><head><style>{CSS}</style></head><body><span id=\"deals-ind\"></span>{body}</body></html>")
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]
}

fn inside(r: &(u32, u32, u32, u32), x: u32, y: u32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

#[derive(Clone, Copy)]
struct Padlock {
    ink_rects: &'static [(u32, u32, u32, u32)],
    hole: (u32, u32, u32, u32),
    ink: [u8; 3],
    what: &'static str,
}

/// The 18x16 button at CSS (bx, by): its 12x12 icon sits centred at (+3, +2).
/// Every device pixel of the button must be ink where the path is, and the
/// face colour everywhere else (including the hole).
fn assert_padlock(img: &RgbaImage, scale: f64, (bx, by): (u32, u32), lock: &Padlock) {
    let Padlock {
        ink_rects,
        hole,
        ink,
        what,
    } = *lock;
    let s = scale as u32;
    let mut inked = 0;
    for dy in 0..16 * s {
        for dx in 0..18 * s {
            let (ix, iy) = (dx / s, dy / s);
            let in_icon = (3..15).contains(&ix) && (2..14).contains(&iy);
            let (vx, vy) = (ix.wrapping_sub(3), iy.wrapping_sub(2));
            let want =
                if in_icon && !inside(&hole, vx, vy) && ink_rects.iter().any(|r| inside(r, vx, vy))
                {
                    inked += 1;
                    ink
                } else {
                    FACE
                };
            let got = px(img, bx * s + dx, by * s + dy);
            assert_eq!(
                got, want,
                "{what} scale {scale}: device px ({dx},{dy}) of the button"
            );
        }
    }
    assert!(inked > 0);
}

fn check(img: &RgbaImage, scale: f64) {
    let open = Padlock {
        ink_rects: OPEN_INK,
        hole: OPEN_HOLE,
        ink: BLACK,
        what: "open padlock",
    };
    let fixed = Padlock {
        ink_rects: FIXED_INK,
        hole: FIXED_HOLE,
        ink: DIS_INK,
        what: "fixed disabled padlock (currentColor)",
    };
    assert_padlock(img, scale, (10, 10), &open);
    assert_padlock(img, scale, (40, 10), &fixed);
}

fn loaded(scale: f64) {
    let mut e = engine();
    e.load_html(&page(&format!(
        "<div id=\"a\">{OPEN_LOCK}</div><div id=\"b\">{FIXED_LOCK}</div>"
    )))
    .expect("load");
    let img = e.paint(vp(scale)).unwrap().rgba.unwrap();
    check(&img, scale);
}

#[test]
fn mainnet_781_padlocks_paint_on_load_scale_1() {
    loaded(1.0);
}

#[test]
fn mainnet_781_padlocks_paint_on_load_scale_2() {
    loaded(2.0);
}

struct Slices(HashMap<String, u64>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.get(name).copied()
    }
    /// As in Deals, the host greys the item it added; the engine then
    /// owns `disabled` (and writes `data-hx-reason`) on that button.
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![(
            "deals.item.lock.customers_by_date".into(),
            ActionState::Disabled {
                reason: "Locked: they added this. Change it by walking away or accepting.".into(),
            },
        )]
    }
}
struct Frags(String);
impl FragmentSource for Frags {
    fn render(&mut self, _section_id: &str, _slices: &[&str]) -> String {
        self.0.clone()
    }
}

fn shell() -> String {
    page(&format!(
        r##"<div id="a">{OPEN_LOCK}</div><section id="s" hx-trigger="slice:deals" hx-target="#b" hx-swap="innerHTML"><div id="b"></div></section>"##
    ))
}

/// Swap the second padlock in as a fragment (as Deals does after a click):
/// the partial repaint equals a forced full repaint of the same state.
fn swap_partial_equals_full(scale: f64) {
    let mut e = engine();
    e.load_html(&shell()).unwrap();
    let mut sl = Slices(HashMap::from([("deals".to_string(), 1)]));
    e.sync(&sl, &mut Frags("<span id=\"empty\"></span>".into()))
        .unwrap();
    e.paint(vp(scale)).unwrap();
    sl.0.insert("deals".into(), 2);
    e.sync(&sl, &mut Frags(FIXED_LOCK.into())).unwrap();
    let partial = e.paint(vp(scale)).unwrap();
    assert!(
        !e.last_paint_stats().full_repaint,
        "padlock swap must not full-repaint"
    );
    let p = partial.rgba.unwrap();
    check(&p, scale);
    let mut f = engine();
    f.load_html(&shell()).unwrap();
    f.sync(
        &Slices(HashMap::from([("deals".to_string(), 1)])),
        &mut Frags(FIXED_LOCK.into()),
    )
    .unwrap();
    f.mark_theme_or_stylesheet_changed();
    let full = f.paint(vp(scale)).unwrap().rgba.unwrap();
    assert_eq!(p.pixels, full.pixels, "partial == full at scale {scale}");
}

#[test]
fn mainnet_781_padlock_swap_partial_equals_full_scale_1() {
    swap_partial_equals_full(1.0);
}

#[test]
fn mainnet_781_padlock_swap_partial_equals_full_scale_2() {
    swap_partial_equals_full(2.0);
}
