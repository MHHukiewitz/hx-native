//! Mainnet uploads only `PaintOutput::damage` to its texture (G1Texture).
//! After a swap that reflows content below it, the host's screen must equal
//! a full repaint of the same DOM, so a click on what is drawn hits what is
//! drawn (Mainnet #811).
use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff;font:14px \"DejaVu Sans\"}\
#sec{width:200px}.ops{line-height:20px}\
.spin{display:inline-block;position:relative;width:80px;height:24px;vertical-align:top;background:#ddd}\
#n{position:absolute;left:0;top:0;width:56px;height:24px;font-size:14px}\
.ud{position:absolute;left:58px;top:0;width:20px;height:24px}\
.ud i{display:block;height:12px;font-style:normal;font-size:9px;line-height:12px}\
.kidadd{height:24px}#add{height:22px;font-size:12px}</style>";
const SHORT: &str = "1 of 2 free";
const LONG: &str = "1 of 2 free · meeting in about 2 weeks";

fn note(t: &str) -> String {
    format!("<span id=\"line\">{t}</span>")
}
fn page(t: &str) -> String {
    format!("<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
<div class=\"ops\" id=\"ops\" hx-trigger=\"slice:ops\">{}</div>\
<div class=\"ed\"><span class=\"spin\"><input id=\"n\" type=\"number\" name=\"n\" value=\"10\" min=\"0\" max=\"100\" step=\"1\" hx-post=\"deal.n\" hx-trigger=\"change\" />\
<span class=\"ud\"><i id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"n\">▲</i><i id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"n\">▼</i></span></span></div>\
<div class=\"kidadd\"><button type=\"button\" id=\"add\" hx-post=\"deal.add\" hx-trigger=\"click\">+ Add to Line access ▼</button></div>\
</form></body></html>", note(t))
}
struct Sl(u64);
impl SliceSource for Sl {
    fn version(&self, n: &str) -> Option<u64> {
        match n {
            "actions" => Some(1),
            "ops" => Some(self.0),
            _ => None,
        }
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}
struct Fr(&'static str);
impl FragmentSource for Fr {
    fn render(&mut self, _: &str, _: &[&str]) -> String {
        note(self.0)
    }
}

const W: u32 = 240;
const H: u32 = 160;

/// Engine + a host texture that only takes the damage rects (Mainnet G1).
struct Host {
    e: Engine,
    screen: Option<RgbaImage>,
    ver: u64,
}
impl Host {
    fn new(t: &'static str) -> Self {
        let mut e = Engine::new(EngineConfig {
            pending_timeout_frames: 120,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        e.declare_actions(&["deal.n", "deal.add"]);
        e.load_html(&page(t)).unwrap();
        let mut h = Host {
            e,
            screen: None,
            ver: 1,
        };
        h.e.sync(&Sl(1), &mut Fr(t)).unwrap();
        h.paint();
        h
    }
    fn paint(&mut self) {
        let out = self.e.paint(Viewport::with_scale(W, H, 1.0)).unwrap();
        let Some(rgba) = out.rgba else { return };
        match &mut self.screen {
            None => self.screen = Some(rgba),
            Some(s) => {
                for r in &out.damage {
                    let (x0, y0) = (r.x.max(0.0) as usize, r.y.max(0.0) as usize);
                    let x1 = ((r.x + r.width).ceil() as usize).min(W as usize);
                    let y1 = ((r.y + r.height).ceil() as usize).min(H as usize);
                    for y in y0..y1 {
                        let a = (y * W as usize + x0) * 4;
                        let b = (y * W as usize + x1) * 4;
                        s.pixels[a..b].copy_from_slice(&rgba.pixels[a..b]);
                    }
                }
            }
        }
    }
    fn swap(&mut self, t: &'static str) {
        self.ver += 1;
        self.e.sync(&Sl(self.ver), &mut Fr(t)).unwrap();
    }
}

fn diff_rows(a: &RgbaImage, b: &RgbaImage) -> Option<(usize, usize, usize)> {
    let w = a.width as usize;
    let (mut lo, mut hi, mut n) = (usize::MAX, 0, 0);
    for (i, (pa, pb)) in a.pixels.chunks(4).zip(b.pixels.chunks(4)).enumerate() {
        if pa != pb {
            lo = lo.min(i / w);
            hi = hi.max(i / w);
            n += 1;
        }
    }
    (n > 0).then_some((lo, hi, n))
}

fn check(from: &'static str, to: &'static str) {
    let mut h = Host::new(from);
    h.swap(to);
    h.paint();
    let full = Host::new(to);
    let truth = full.e.last_frame().unwrap();
    let engine_frame = h.e.last_frame().unwrap();
    assert_eq!(
        diff_rows(engine_frame, truth),
        None,
        "{from:?} -> {to:?}: engine's partial frame != full repaint (rows lo,hi,count)"
    );
    assert_eq!(
        diff_rows(h.screen.as_ref().unwrap(), truth),
        None,
        "{from:?} -> {to:?}: host texture from damage rects != full repaint (rows lo,hi,count)"
    );
}

#[test]
fn grow_note_screen_equals_full_repaint() {
    check(SHORT, LONG)
}
#[test]
fn shrink_note_screen_equals_full_repaint() {
    check(LONG, SHORT)
}

/// #88: the release resolves the swapped DOM before the paint. That paint's
/// partial frame (and the host's damage-only texture) must still equal a
/// full repaint.
fn check_release_between(from: &'static str, to: &'static str) {
    let mut h = Host::new(from);
    let (x, y, w, bh) = h.e.test_element_box("add").unwrap();
    let at = (x + w / 2.0, y + bh / 2.0);
    let m = Modifiers::default();
    h.e.input(InputEvent::PointerDown {
        x: at.0,
        y: at.1,
        button: PointerButton::Primary,
        mods: m,
    });
    h.swap(to);
    h.e.input(InputEvent::PointerUp {
        x: at.0,
        y: at.1,
        button: PointerButton::Primary,
        mods: m,
    });
    h.e.input(InputEvent::PointerLeave);
    h.paint();
    let full = Host::new(to);
    let truth = full.e.last_frame().unwrap();
    assert_eq!(
        diff_rows(h.e.last_frame().unwrap(), truth),
        None,
        "{from:?} -> {to:?}: engine frame != full repaint"
    );
    assert_eq!(
        diff_rows(h.screen.as_ref().unwrap(), truth),
        None,
        "{from:?} -> {to:?}: damage-only texture != full repaint"
    );
}

#[test]
fn release_between_swap_and_paint_grow_keeps_partial_equal_to_full() {
    check_release_between(SHORT, LONG)
}
#[test]
fn release_between_swap_and_paint_shrink_keeps_partial_equal_to_full() {
    check_release_between(LONG, SHORT)
}
