//! Mainnet #811 shape: the ▼ click keeps focus in the box (Ruling 4), its
//! `change` makes the host redraw the whole section (same-id focused box),
//! the redraw's meeting line wraps and pushes the box down, and the next
//! click on the painted ▼ must step it again, not hit the row below.
use hx_native::*;
use std::cell::RefCell;
use std::rc::Rc;

const STYLE: &str = "<style>html,body{margin:0;background:#fff;font:14px \"DejaVu Sans\"}\
#sec{width:200px}.ops{line-height:20px}\
.spin{display:inline-block;position:relative;width:80px;height:24px;vertical-align:top}\
#n{position:absolute;left:0;top:0;width:56px;height:24px;font-size:14px}\
.ud{position:absolute;left:58px;top:0;width:20px;height:24px}\
.ud i{display:block;height:12px;font-style:normal;font-size:9px;line-height:12px}\
.kidadd{height:24px}#add{height:22px;font-size:12px}</style>";

const SHORT: &str = "1 of 2 free";
const LONG: &str = "1 of 2 free · meeting in about 2 weeks";

fn section(note: &str, value: i32) -> String {
    format!("<div class=\"ops\"><span id=\"line\">{note}</span></div>\
<div class=\"ed\"><span class=\"spin\"><input id=\"n\" type=\"number\" name=\"n\" value=\"{value}\" min=\"0\" max=\"100\" step=\"1\" hx-post=\"deal.n\" hx-trigger=\"change\" hx-sync=\"drop\" />\
<span class=\"ud\"><i id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"n\">▲</i><i id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"n\">▼</i></span></span></div>\
<div class=\"kidadd\"><button type=\"button\" id=\"add\" hx-post=\"deal.add\" hx-trigger=\"click\">+ Add to Line access ▼</button></div>")
}

fn page(note: &str, value: i32) -> String {
    format!("<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\"><div id=\"sec\" hx-trigger=\"slice:proposal\">{}</div></form></body></html>", section(note, value))
}

/// Game: the note grows once the value goes below 10 (Game's meeting estimate).
#[derive(Clone)]
struct Game {
    value: i32,
    version: u64,
}
impl Game {
    fn note(&self) -> &'static str {
        if self.value < 10 {
            LONG
        } else {
            SHORT
        }
    }
}
struct Sl(Rc<RefCell<Game>>);
impl SliceSource for Sl {
    fn version(&self, n: &str) -> Option<u64> {
        match n {
            "actions" => Some(1),
            "proposal" => Some(self.0.borrow().version),
            _ => None,
        }
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}
struct Fr(Rc<RefCell<Game>>);
impl FragmentSource for Fr {
    fn render(&mut self, _id: &str, _s: &[&str]) -> String {
        let g = self.0.borrow();
        section(g.note(), g.value)
    }
}

struct Host {
    e: Engine,
    game: Rc<RefCell<Game>>,
    adds: usize,
}
const VP: (u32, u32) = (240, 160);

impl Host {
    fn new(value: i32) -> Self {
        let game = Rc::new(RefCell::new(Game { value, version: 1 }));
        let mut e = Engine::new(EngineConfig {
            pending_timeout_frames: 120,
            paint_mode: PaintMode::Exact,
            key_releases: true,
            ..Default::default()
        });
        e.declare_actions(&["deal.n", "deal.add"]);
        e.load_html(&page(game.borrow().note(), value)).unwrap();
        let mut h = Host { e, game, adds: 0 };
        h.frame(&[]);
        h
    }
    /// Mainnet `step`: input (route + apply), sync, resize, paint.
    fn frame(&mut self, events: &[InputEvent]) {
        for ev in events {
            let mut outs = vec![self.e.input(ev.clone())];
            outs.extend(self.e.take_outcomes());
            for o in outs {
                if let InputOutcome::Action {
                    request,
                    name,
                    args,
                    ..
                } = o
                {
                    if name == "deal.add" {
                        self.adds += 1;
                    }
                    if name == "deal.n" {
                        let v: i32 = args
                            .iter()
                            .find(|(k, _)| k == "n")
                            .unwrap()
                            .1
                            .parse()
                            .unwrap();
                        let mut g = self.game.borrow_mut();
                        g.value = v;
                        g.version += 1;
                    }
                    self.e.complete(request, Ok(()));
                }
            }
        }
        self.e
            .sync(&Sl(self.game.clone()), &mut Fr(self.game.clone()))
            .unwrap();
        self.e.paint(Viewport::with_scale(VP.0, VP.1, 1.0)).unwrap();
    }
    fn click_frames(&mut self, (x, y): (f32, f32)) {
        let m = Modifiers::default();
        self.frame(&[InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        }]);
        self.frame(&[InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        }]);
    }
    fn centre(&self, id: &str) -> (f32, f32) {
        let (x, y, w, h) = self.e.test_element_box(id).unwrap();
        (x + w / 2.0, y + h / 2.0)
    }
    fn value(&self) -> String {
        self.e.attr("n", "value").unwrap_or_default()
    }
}

/// ▼ position in a full repaint of the long-note page.
fn painted_dn_long() -> (f32, f32) {
    Host::new(9).centre("dn")
}

#[test]
fn fixture_the_wrap_moves_the_arrow_down() {
    let short = Host::new(10).centre("dn");
    let long = painted_dn_long();
    eprintln!("short ▼ {short:?}, long ▼ {long:?}");
    assert!(long.1 > short.1 + 10.0);
}

#[test]
fn focused_box_redrawn_by_its_own_change_then_click_on_the_painted_arrow() {
    let mut h = Host::new(10);
    let first = h.centre("dn");
    h.click_frames(first); // 10 -> 9: focus stays in the box; redraw wraps the note.
    assert_eq!(h.value(), "9");
    assert_eq!(
        h.e.focus_state().focused_id.as_deref(),
        Some("n"),
        "Ruling 4: focus stays in the box"
    );
    let target = painted_dn_long();
    assert_eq!(
        h.centre("dn"),
        target,
        "after the paint, live layout == full-repaint layout"
    );
    h.click_frames(target);
    assert_eq!(
        h.adds, 0,
        "click on the painted ▼ reached + Add to Line access"
    );
    assert_eq!(h.value(), "8", "second ▼ click steps 9 -> 8");
}

#[test]
fn same_but_down_and_up_in_one_frame() {
    let mut h = Host::new(10);
    let m = Modifiers::default();
    let c = h.centre("dn");
    let ev = |(x, y): (f32, f32)| {
        [
            InputEvent::PointerDown {
                x,
                y,
                button: PointerButton::Primary,
                mods: m,
            },
            InputEvent::PointerUp {
                x,
                y,
                button: PointerButton::Primary,
                mods: m,
            },
        ]
    };
    h.frame(&ev(c));
    assert_eq!(h.value(), "9");
    let target = painted_dn_long();
    h.frame(&ev(target));
    assert_eq!(
        h.adds, 0,
        "click on the painted ▼ reached + Add to Line access"
    );
    assert_eq!(h.value(), "8");
}
