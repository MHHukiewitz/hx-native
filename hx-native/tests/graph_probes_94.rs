//! Graph's review probes for hx-native PR #94 (issue #89, Designer's
//! `hx-sync` row in Mainnet HTML_CONTRACT at master 188feffd, with the two
//! rulings added at 5983ff17).
//!
//! Reviewed at 6387042. Every probe pins behaviour the review checked.
//! `checkbox_catch_up_compares_presence` and the two radio probes fail at
//! 6387042 (a held untick and a held radio pick are lost) and pass with
//! Graph's fix.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use hx_native::*;

fn vp() -> Viewport {
    Viewport::with_scale(320, 240, 1.0)
}

const STYLE: &str = "<style>*{box-sizing:border-box}html,body{margin:0;background:#fff;\
font:12px \"DejaVu Sans\"}input,select,button{display:block;width:160px;height:20px;margin:2px}\
.spin{display:block;position:relative}.spin i{position:absolute;left:200px;width:20px;height:10px;\
font-style:normal;font-size:8px}.up{top:2px}.dn{top:12px}</style>";

/// The game's side: the HTML each section renders, slice versions, and the
/// `actions` slice.
#[derive(Default)]
struct Game {
    html: HashMap<String, String>,
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
}

struct Slices(Rc<RefCell<Game>>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.borrow().versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.0.borrow().actions.clone()
    }
}

struct Frags(Rc<RefCell<Game>>);
impl FragmentSource for Frags {
    fn render(&mut self, s: &str, _slices: &[&str]) -> String {
        self.0.borrow().html.get(s).cloned().unwrap_or_default()
    }
}

struct H {
    e: Engine,
    g: Rc<RefCell<Game>>,
}

fn fee_box(id: &str, name: &str, value: &str, post: bool) -> String {
    let post = if post {
        format!(" hx-post=\"deal.{name}\" hx-trigger=\"change\"")
    } else {
        String::new()
    };
    format!(
        "<span class=\"spin\"><input id=\"{id}\" type=\"number\" name=\"{name}\" value=\"{value}\" \
         min=\"0\" max=\"100.00\" step=\"0.01\"{post} />\
         <i id=\"{id}-up\" class=\"up\" data-hx-step=\"up\" data-hx-step-for=\"{id}\">U</i>\
         <i id=\"{id}-dn\" class=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"{id}\">D</i></span>"
    )
}

impl H {
    /// `body` holds `<section id=S hx-trigger="slice:S" …>` placeholders;
    /// `sections` gives each one's first render.
    fn new(actions: &[&str], body: &str, sections: &[(&str, String)], cfg: EngineConfig) -> H {
        let g = Rc::new(RefCell::new(Game::default()));
        {
            let mut gm = g.borrow_mut();
            for (s, html) in sections {
                gm.html.insert(s.to_string(), html.clone());
                gm.versions.insert(s.to_string(), 1);
            }
            gm.versions.insert("actions".into(), 1);
        }
        let mut e = Engine::new(EngineConfig {
            paint_mode: PaintMode::Exact,
            pending_timeout_frames: 1000,
            ..cfg
        });
        e.declare_actions(actions);
        e.load_html(&format!(
            "<!DOCTYPE html><html><head>{STYLE}</head><body>{body}\
             <input id=\"blur\" type=\"text\" value=\"x\" /></body></html>"
        ))
        .unwrap();
        let _ = e.paint(vp()).unwrap();
        let mut h = H { e, g };
        h.sync();
        h
    }

    fn sync(&mut self) {
        self.e
            .sync(&Slices(self.g.clone()), &mut Frags(self.g.clone()))
            .unwrap();
        let _ = self.e.paint(vp()).unwrap();
    }

    /// The game re-renders section `s` with `html`.
    fn redraw(&mut self, s: &str, html: String) {
        {
            let mut gm = self.g.borrow_mut();
            gm.html.insert(s.into(), html);
            *gm.versions.entry(s.into()).or_insert(0) += 1;
        }
        self.sync();
    }

    fn set_actions(&mut self, a: Vec<(String, ActionState)>) {
        {
            let mut gm = self.g.borrow_mut();
            gm.actions = a;
            *gm.versions.entry("actions".into()).or_insert(0) += 1;
        }
        self.sync();
    }

    fn feed(&mut self, ev: InputEvent) -> Vec<InputOutcome> {
        let mut out = vec![self.e.input(ev)];
        out.extend(self.e.take_outcomes());
        let _ = self.e.paint(vp());
        out
    }

    fn mid(&self, id: &str) -> (f32, f32) {
        let b = self.e.test_element_box(id).unwrap();
        (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
    }

    fn down(&mut self, id: &str) -> Vec<InputOutcome> {
        let (x, y) = self.mid(id);
        self.feed(InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        })
    }

    fn up(&mut self, id: &str) -> Vec<InputOutcome> {
        let (x, y) = self.mid(id);
        self.feed(InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        })
    }

    fn click(&mut self, id: &str) -> Vec<InputOutcome> {
        let mut o = self.down(id);
        o.extend(self.up(id));
        o
    }

    fn key(&mut self, k: &str, pressed: bool, repeat: bool) -> Vec<InputOutcome> {
        self.feed(InputEvent::Key(KeyEvent {
            key: k.into(),
            code: k.into(),
            mods: Modifiers::default(),
            repeat,
            pressed,
        }))
    }

    fn tap(&mut self, k: &str) -> Vec<InputOutcome> {
        let mut o = self.key(k, true, false);
        o.extend(self.key(k, false, false));
        o
    }

    fn text(&mut self, t: &str) -> Vec<InputOutcome> {
        self.feed(InputEvent::Text(t.into()))
    }

    fn val(&self, id: &str) -> String {
        self.e.attr(id, "value").unwrap_or_default()
    }

    fn finish(&mut self, outs: &[InputOutcome], ok: bool) -> Vec<InputOutcome> {
        for r in requests(outs) {
            let res = if ok {
                Ok(())
            } else {
                Err(Reason("Not now".into()))
            };
            self.e.complete(r, res);
        }
        let o = self.e.take_outcomes();
        let _ = self.e.paint(vp());
        o
    }
}

fn requests(outs: &[InputOutcome]) -> Vec<RequestId> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action { request, .. } => Some(*request),
            _ => None,
        })
        .collect()
}

fn actions(outs: &[InputOutcome]) -> Vec<Vec<(String, String)>> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action { args, .. } => Some(args.clone()),
            _ => None,
        })
        .collect()
}

fn arg(args: &[(String, String)], k: &str) -> Option<String> {
    args.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
}

fn changes(outs: &[InputOutcome], name: &str) -> Vec<String> {
    actions(outs).iter().filter_map(|a| arg(a, name)).collect()
}

// ---------- the fee box ----------

fn fee_page(cfg: EngineConfig) -> H {
    H::new(
        &["deal.fee"],
        "<section id=\"deal\" hx-trigger=\"slice:deal\" hx-target=\"#deal\" hx-swap=\"innerHTML\"></section>",
        &[("deal", fee_box("fee", "fee", "4.69", true))],
        cfg,
    )
}

fn cfg(key_releases: bool) -> EngineConfig {
    EngineConfig {
        key_releases,
        ..Default::default()
    }
}

/// Plays the game: every request is completed and then the game redraws
/// the deal section with the value it accepted (clamped to `max`). Returns
/// every change sent.
fn settle(h: &mut H, mut inflight: Vec<InputOutcome>, max: f64) -> Vec<String> {
    let mut sent = Vec::new();
    while !requests(&inflight).is_empty() {
        let last = changes(&inflight, "fee").last().cloned();
        let next = h.finish(&inflight, true);
        if let Some(v) = last {
            let n: f64 = v.parse().unwrap();
            let acc = if n > max { format!("{max:.2}") } else { v };
            h.redraw("deal", fee_box("fee", "fee", &acc, true));
        }
        sent.extend(changes(&next, "fee"));
        inflight = next;
    }
    sent
}

/// Review point 3: the game clamps the first request and redraws before
/// (and, separately, after) it completes; the newer 4.72 still goes out
/// once, the game's clamp of 4.72 then stands, and nothing more is sent.
#[test]
fn clamp_of_an_older_request_still_sends_the_newer_commit_once() {
    for redraw_first in [true, false] {
        let mut h = fee_page(cfg(false));
        let o1 = h.click("fee-up");
        assert_eq!(changes(&o1, "fee"), ["4.7"]);
        h.click("fee-up");
        h.click("fee-up");
        assert_eq!(h.val("fee"), "4.72");
        let caught = if redraw_first {
            h.redraw("deal", fee_box("fee", "fee", "4.65", true));
            assert_eq!(h.val("fee"), "4.72", "older clamp never overwrites 4.72");
            h.finish(&o1, true)
        } else {
            let c = h.finish(&o1, true);
            h.redraw("deal", fee_box("fee", "fee", "4.65", true));
            assert_eq!(h.val("fee"), "4.72", "late reply to 4.7 keeps 4.72");
            c
        };
        assert_eq!(
            changes(&caught, "fee"),
            ["4.72"],
            "redraw_first={redraw_first}"
        );
        // Locked frame order: the game's queue completes, then publishes.
        assert!(h.finish(&caught, true).is_empty());
        h.redraw("deal", fee_box("fee", "fee", "4.65", true));
        assert_eq!(h.val("fee"), "4.65", "the game's answer to 4.72 stands");
        assert!(changes(&h.click("blur"), "fee").is_empty());
        h.redraw("deal", fee_box("fee", "fee", "4.65", true));
        assert!(h.e.take_outcomes().is_empty(), "no resend loop");
    }
}

/// Known limit, pinned: the engine has no request generation, so a redraw
/// that answers the catch-up itself but arrives BEFORE its `complete()` is
/// treated like an older one and overwritten with the committed value. The
/// locked frame order (the game's queue completes, the snapshot publishes at
/// end of tick) rules this out for Game actions; this pins today's result
/// so a change is deliberate.
#[test]
fn a_redraw_before_complete_of_the_catch_up_is_treated_as_older() {
    let mut h = fee_page(cfg(false));
    let o1 = h.click("fee-up");
    h.click("fee-up");
    let caught = h.finish(&o1, true);
    assert_eq!(changes(&caught, "fee"), ["4.71"]);
    h.redraw("deal", fee_box("fee", "fee", "4.65", true));
    assert_eq!(h.val("fee"), "4.71", "pinned: the clamp is held off");
    assert!(h.finish(&caught, true).is_empty(), "still no resend");
    // Only the game's next redraw shows its value.
    h.redraw("deal", fee_box("fee", "fee", "4.65", true));
    assert_eq!(h.val("fee"), "4.65");
}

/// Review point 4: three fast ▲ presses send exactly two changes and the box
/// and the game end equal, on spin clicks and on arrow keys, with
/// key_releases off and on.
#[test]
fn three_fast_steps_send_two_changes_clicks_and_keys_both_key_release_modes() {
    for key_releases in [false, true] {
        // Spin clicks.
        let mut h = fee_page(cfg(key_releases));
        let mut inflight = Vec::new();
        for _ in 0..3 {
            inflight.extend(h.click("fee-up"));
        }
        let mut sent = changes(&inflight, "fee");
        sent.extend(settle(&mut h, inflight, 100.0));
        sent.extend(changes(&h.click("blur"), "fee"));
        assert_eq!(sent, ["4.7", "4.72"], "clicks, key_releases={key_releases}");
        assert_eq!(h.val("fee"), "4.72");

        // Arrow keys.
        let mut h = fee_page(cfg(key_releases));
        h.e.test_set_focus("fee");
        let mut inflight = Vec::new();
        for _ in 0..3 {
            inflight.extend(h.tap("ArrowUp"));
        }
        let mut sent = changes(&inflight, "fee");
        sent.extend(settle(&mut h, inflight, 100.0));
        sent.extend(changes(&h.click("blur"), "fee"));
        assert_eq!(sent, ["4.7", "4.72"], "keys, key_releases={key_releases}");
        assert_eq!(h.val("fee"), "4.72");
    }
}

/// Review point 4, held: a held arrow key and a held spin arrow, each held
/// again while the first release's change is pending.
#[test]
fn held_arrows_while_pending_send_one_catch_up() {
    for key_releases in [false, true] {
        let mut h = fee_page(cfg(key_releases));
        h.e.test_set_focus("fee");
        let mut inflight = h.key("ArrowUp", true, false);
        for _ in 0..4 {
            inflight.extend(h.key("ArrowUp", true, true));
        }
        inflight.extend(h.key("ArrowUp", false, false));
        assert_eq!(h.val("fee"), "4.74");
        // Held again while that is pending.
        inflight.extend(h.key("ArrowUp", true, false));
        for _ in 0..2 {
            inflight.extend(h.key("ArrowUp", true, true));
        }
        inflight.extend(h.key("ArrowUp", false, false));
        assert_eq!(h.val("fee"), "4.77");
        let mut sent = changes(&inflight, "fee");
        sent.extend(settle(&mut h, inflight, 100.0));
        sent.extend(changes(&h.click("blur"), "fee"));
        assert_eq!(sent.last().map(String::as_str), Some("4.77"), "{sent:?}");
        assert!(sent.len() <= 3, "key_releases={key_releases}: {sent:?}");
        assert_eq!(h.val("fee"), "4.77");
    }

    let mut h = fee_page(cfg(false));
    let mut inflight = h.down("fee-up");
    h.e.test_hold_spin_for(Duration::from_millis(600));
    inflight.extend(h.up("fee-up"));
    let first = changes(&inflight, "fee");
    assert_eq!(first.len(), 1, "{first:?}");
    inflight.extend(h.down("fee-up"));
    h.e.test_hold_spin_for(Duration::from_millis(600));
    inflight.extend(h.up("fee-up"));
    assert_eq!(changes(&inflight, "fee"), first, "second hold is held");
    let box_value = h.val("fee");
    let caught = settle(&mut h, inflight, 100.0);
    assert_eq!(
        caught.as_slice(),
        std::slice::from_ref(&box_value),
        "one catch-up"
    );
    assert_eq!(h.val("fee"), box_value);
}

/// Review point 5: an error completion runs the catch-up exactly once, and a
/// second error does not resend.
#[test]
fn error_path_runs_the_catch_up_exactly_once() {
    let mut h = fee_page(cfg(false));
    let o1 = h.click("fee-up");
    h.click("fee-up");
    h.click("fee-up");
    let caught = h.finish(&o1, false);
    assert_eq!(changes(&caught, "fee"), ["4.72"]);
    let again = h.finish(&caught, false);
    assert!(actions(&again).is_empty(), "{again:?}");
    assert!(changes(&h.click("blur"), "fee").is_empty());
}

// ---------- checkbox, radio, text ----------

fn tick_html(checked: bool) -> String {
    format!(
        "<input id=\"tick\" type=\"checkbox\" name=\"tick\" value=\"on\"{} \
         hx-post=\"deal.tick\" hx-trigger=\"change\" />",
        if checked { " checked" } else { "" }
    )
}

fn radios_html(on: &str) -> String {
    ["a", "b", "c"]
        .iter()
        .map(|v| {
            format!(
                "<input id=\"r{v}\" type=\"radio\" name=\"r\" value=\"{v}\"{} hx-post=\"deal.r\" hx-trigger=\"change\" />",
                if *v == on { " checked" } else { "" }
            )
        })
        .collect()
}

fn checks_page() -> H {
    H::new(
        &["deal.tick", "deal.r"],
        "<section id=\"ts\" hx-trigger=\"slice:ts\" hx-target=\"#ts\" hx-swap=\"innerHTML\"></section>\
         <form id=\"rf\">\
         <section id=\"rs\" hx-trigger=\"slice:rs\" hx-target=\"#rs\" hx-swap=\"innerHTML\"></section></form>",
        &[("ts", tick_html(false)), ("rs", radios_html("a"))],
        cfg(false),
    )
}

/// Review point 6: a checkbox catches up by presence of its pair, never by
/// toggling: tick (sent), untick + tick while pending sends nothing more;
/// tick (sent), untick while pending sends the untick once, without the pair.
#[test]
fn checkbox_catch_up_compares_presence() {
    let mut h = checks_page();
    let o1 = h.click("tick");
    assert_eq!(changes(&o1, "tick"), ["on"]);
    h.click("tick");
    h.click("tick");
    assert!(
        actions(&h.finish(&o1, true)).is_empty(),
        "back on the sent state"
    );

    let mut h = checks_page();
    let o1 = h.click("tick");
    h.click("tick");
    // An older redraw (answer to the tick) shows it ticked: the untick stays.
    h.redraw("ts", tick_html(true));
    h.redraw("ts", tick_html(true));
    let caught = h.finish(&o1, true);
    let a = actions(&caught);
    assert_eq!(a.len(), 1, "{caught:?}");
    assert_eq!(arg(&a[0], "tick"), None, "untick posts no pair");
    assert!(h.finish(&caught, true).is_empty());
    h.redraw("ts", tick_html(false));
    assert!(h.e.take_outcomes().is_empty());
}

/// Review point 6: radios, each posting its own request (`change` is only
/// valid on the control). b, then c, then b again while b's request is
/// still pending: c went out on its own request, so the game's last value
/// is c and the held b must catch up, or the box shows b while the game
/// has c.
#[test]
fn radio_b_c_b_while_b_is_pending_ends_on_b() {
    let mut h = checks_page();
    let o1 = h.click("rb");
    assert_eq!(changes(&o1, "r"), ["b"]);
    let o2 = h.click("rc");
    assert_eq!(changes(&o2, "r"), ["c"], "rc is not pending, so it sends");
    let o3 = h.click("rb");
    assert!(actions(&o3).is_empty(), "rb is pending, so b is held");
    let caught = h.finish(&o1, true);
    assert_eq!(
        changes(&caught, "r"),
        ["b"],
        "the game must end on b, not c"
    );
    let mut rest = h.finish(&o2, true);
    rest.extend(h.finish(&caught, true));
    assert!(actions(&rest).is_empty(), "{rest:?}");
}

/// Review point 6: the same with an older redraw (the game's answer to c)
/// arriving while b is held: the box keeps b.
#[test]
fn radio_held_b_survives_an_older_redraw() {
    let mut h = checks_page();
    let o1 = h.click("rb");
    let o2 = h.click("rc");
    h.click("rb");
    let _ = h.finish(&o2, true);
    h.redraw("rs", radios_html("c"));
    let caught = h.finish(&o1, true);
    assert_eq!(changes(&caught, "r"), ["b"]);
    let _ = h.finish(&caught, true);
    // Clicking c again must send c: the box really shows b.
    let again = h.click("rc");
    assert_eq!(changes(&again, "r"), ["c"], "box was really on b");
}

fn note_page() -> H {
    H::new(
        &["deal.note"],
        "<input id=\"note\" type=\"text\" name=\"note\" value=\"\" hx-post=\"deal.note\" hx-trigger=\"change\" />",
        &[],
        cfg(false),
    )
}

/// Review point 6, text: a held Enter commit catches up with the committed
/// text, not with what the player has typed since; that later typing
/// commits on its own Enter, and blur adds nothing.
#[test]
fn text_catch_up_sends_the_commit_not_the_typing_in_progress() {
    let mut h = note_page();
    h.e.test_set_focus("note");
    h.text("a");
    let o1 = h.tap("Enter");
    assert_eq!(changes(&o1, "note"), ["a"]);
    h.text("b");
    assert!(changes(&h.tap("Enter"), "note").is_empty());
    h.text("c");
    let caught = h.finish(&o1, true);
    assert_eq!(changes(&caught, "note"), ["ab"]);
    assert_eq!(h.e.test_editor_display("note").as_deref(), Some("abc"));
    assert!(changes(&h.tap("Enter"), "note").is_empty(), "ab pending");
    let caught2 = h.finish(&caught, true);
    assert_eq!(
        changes(&caught2, "note"),
        ["abc"],
        "Enter was held, caught up"
    );
    h.finish(&caught2, true);
    assert!(changes(&h.click("blur"), "note").is_empty());
}

/// Review point 6, text: blur while pending holds the commit, and the
/// catch-up sends it once.
#[test]
fn text_blur_while_pending_is_caught_up() {
    let mut h = note_page();
    h.e.test_set_focus("note");
    h.text("a");
    let o1 = h.tap("Enter");
    h.text("b");
    assert!(changes(&h.click("blur"), "note").is_empty());
    let caught = h.finish(&o1, true);
    assert_eq!(changes(&caught, "note"), ["ab"]);
    assert!(h.finish(&caught, true).is_empty());
}

// ---------- two controls in one form, identity ----------

fn terms_html(fee: &str, qty: &str, fee_id: &str) -> String {
    format!(
        "{}{}",
        fee_box(fee_id, "fee", fee, true),
        fee_box("qty", "qty", qty, true)
    )
}

fn terms_page() -> H {
    H::new(
        &["deal.fee", "deal.qty"],
        "<form id=\"tf\">\
         <section id=\"tsec\" hx-trigger=\"slice:tsec\" hx-target=\"#tsec\" hx-swap=\"innerHTML\"></section></form>\
         <section id=\"side\" hx-trigger=\"slice:side\" hx-target=\"#side\" hx-swap=\"innerHTML\"></section>",
        &[
            ("tsec", terms_html("1.00", "2.00", "fee")),
            ("side", "<b id=\"sb\">1</b>".into()),
        ],
        cfg(false),
    )
}

/// Review point 2: two controls in one form, each with its own request
/// (`change` is only valid on the control). A held change on `fee` is
/// tracked apart from `qty`: `qty`'s own request goes out at once and
/// carries the form, `fee`'s held 1.02 included; `fee`'s catch-up still
/// sends 1.02 once. A sibling redraw keeps both boxes; an older redraw of
/// the form's section keeps the held fee (qty: see the pinned gap below).
#[test]
fn two_controls_in_one_form_are_tracked_apart() {
    let mut h = terms_page();
    let o1 = h.click("fee-up");
    let a1 = actions(&o1);
    assert_eq!(a1.len(), 1);
    assert_eq!(arg(&a1[0], "fee").as_deref(), Some("1.01"));
    h.click("fee-up");
    let q = h.click("qty-up");
    let aq = actions(&q);
    assert_eq!(aq.len(), 1, "qty is not pending, so it sends: {q:?}");
    assert_eq!(arg(&aq[0], "qty").as_deref(), Some("2.01"));
    assert_eq!(
        arg(&aq[0], "fee").as_deref(),
        Some("1.02"),
        "form args read the box"
    );
    h.redraw("side", "<b id=\"sb\">2</b>".into());
    assert_eq!(h.val("fee"), "1.02");
    assert_eq!(h.val("qty"), "2.01");
    // Known gap, pinned: only a held (or caught-up) value is protected. qty's
    // 2.01 was sent directly and is still unanswered, yet the older redraw
    // (the game's answer to fee 1.01, drawn before qty's ran) shows 2.00
    // until the game's answer to qty redraws it.
    h.redraw("tsec", terms_html("1.01", "2.00", "fee"));
    assert_eq!(h.val("fee"), "1.02", "held fee is kept");
    assert_eq!(h.val("qty"), "2.00", "pinned gap: sent qty is drawn over");
    let caught = h.finish(&o1, true);
    let a = actions(&caught);
    assert_eq!(a.len(), 1, "{caught:?}");
    assert_eq!(arg(&a[0], "fee").as_deref(), Some("1.02"));
    assert!(h.finish(&caught, true).is_empty());
    assert!(h.finish(&q, true).is_empty());
    h.redraw("tsec", terms_html("1.02", "2.01", "fee"));
    assert_eq!(h.val("qty"), "2.01", "the game's answer to qty");
    assert!(actions(&h.click("blur")).is_empty());
}

/// Review point 2: a redraw that replaces the control with one of another
/// id drops its catch-up (the box the player committed is gone); nothing is
/// sent for it and nothing panics.
#[test]
fn a_redraw_that_renames_the_control_drops_its_catch_up() {
    let mut h = terms_page();
    let o1 = h.click("fee-up");
    h.click("fee-up");
    h.redraw("tsec", terms_html("1.01", "2.00", "fee2"));
    let caught = h.finish(&o1, true);
    assert!(actions(&caught).is_empty(), "{caught:?}");
    assert_eq!(h.val("fee2"), "1.01");
}

/// Review point 7: many cycles of held commits leave nothing behind: after
/// each cycle settles, a blur sends nothing and an unrelated redraw does not
/// rewrite the box.
#[test]
fn nothing_accumulates_over_many_cycles() {
    let mut h = fee_page(cfg(false));
    for i in 0..60 {
        let mut inflight = h.click("fee-up");
        inflight.extend(h.click("fee-up"));
        settle(&mut h, inflight, 100.0);
        assert!(changes(&h.click("blur"), "fee").is_empty(), "cycle {i}");
        let v = h.val("fee");
        h.redraw("deal", fee_box("fee", "fee", "1.00", true));
        assert_eq!(
            h.val("fee"),
            "1.00",
            "cycle {i}: no stale entry restores {v}"
        );
    }
}

// ---------- Designer's rulings at 5983ff17 ----------

fn grey_fee(h: &mut H) {
    h.set_actions(vec![(
        "deal.fee".into(),
        ActionState::Disabled {
            reason: "Deal closed".into(),
        },
    )]);
}

/// Ruling (Mainnet 5983ff17, question 1): the game greys the control while
/// a change is held, so the held change is dropped and the box shows the
/// game's value. Nothing goes out when the pending request finishes, not
/// even a `Blocked`, since the player did nothing at that moment.
#[test]
fn ruling_greyed_while_held_reports_nothing_on_complete() {
    let mut h = fee_page(cfg(false));
    let o1 = h.click("fee-up");
    h.click("fee-up");
    grey_fee(&mut h);
    assert!(h.e.attr("fee", "disabled").is_some(), "greyed");
    assert_eq!(h.val("fee"), "4.7", "the game's value (last sent)");
    let done = h.finish(&o1, true);
    assert!(done.is_empty(), "held change dropped: {done:?}");
}

/// Ruling 1, ungreyed before the request finishes: the held change was
/// dropped when the box greyed, so it must not come back and send.
#[test]
fn ruling_greyed_then_ungreyed_while_pending_sends_nothing() {
    let mut h = fee_page(cfg(false));
    let o1 = h.click("fee-up");
    h.click("fee-up");
    grey_fee(&mut h);
    h.set_actions(vec![]);
    assert_eq!(h.val("fee"), "4.7");
    let done = h.finish(&o1, true);
    assert!(actions(&done).is_empty(), "dropped change resent: {done:?}");
    assert_eq!(h.val("fee"), "4.7");
}

/// Ruling 1, an older redraw while greyed: the box keeps the game's value;
/// the dropped 4.71 must not be written back into a greyed box.
#[test]
fn ruling_greyed_box_is_not_rewritten_by_an_older_redraw() {
    let mut h = fee_page(cfg(false));
    let o1 = h.click("fee-up");
    h.click("fee-up");
    grey_fee(&mut h);
    h.redraw("deal", fee_box("fee", "fee", "4.7", true));
    assert_eq!(
        h.val("fee"),
        "4.7",
        "the game's value, not the dropped 4.71"
    );
    let _ = h.finish(&o1, true);
}

fn plan_html(fee: &str, applied: Option<&str>) -> String {
    let box_html = fee_box("fee", "fee", fee, true);
    let box_html = match applied {
        Some(a) => box_html.replace(" min=", &format!(" data-hx-applied=\"{a}\" min=")),
        None => box_html,
    };
    format!(
        "<form id=\"pf\">{box_html}<button id=\"apply\" type=\"button\" \
         hx-post=\"deal.apply\">Apply</button></form>"
    )
}

fn plan_page(applied: bool) -> H {
    let a = applied.then_some("4.69");
    let mut h = H::new(
        &["deal.fee", "deal.apply"],
        "<section id=\"deal\" hx-trigger=\"slice:deal\" hx-target=\"#deal\" \
         hx-swap=\"innerHTML\"></section>",
        &[("deal", plan_html("4.69", a))],
        cfg(false),
    );
    h.set_actions(vec![(
        "deal.apply".into(),
        ActionState::RequiresChange {
            form: "pf".into(),
            reason: "Change something first".into(),
        },
    )]);
    h
}

/// Ruling (Mainnet 5983ff17, question 2): while a change is held, the
/// unapplied-change baseline stays at the game's last value, so an older
/// redraw (here one that re-renders the whole form, so the fallback
/// baseline is captured again) does not make the form read clean. Once the
/// game answers the caught-up value, the form reads clean.
#[test]
fn ruling_baseline_stays_at_the_games_value_while_held() {
    for applied in [false, true] {
        let mut h = plan_page(applied);
        assert!(h.e.attr("apply", "disabled").is_some(), "clean form greys");
        let o1 = h.click("fee-up");
        h.click("fee-up");
        assert_eq!(h.val("fee"), "4.71");
        // The game answers 4.7 (older) and re-renders the form with it as
        // the applied value.
        h.redraw("deal", plan_html("4.7", applied.then_some("4.7")));
        assert_eq!(h.val("fee"), "4.71", "applied={applied}: newer commit kept");
        assert!(
            h.e.attr("apply", "disabled").is_none(),
            "applied={applied}: baseline is the game's 4.7, form reads changed"
        );
        let caught = h.finish(&o1, true);
        assert_eq!(changes(&caught, "fee"), ["4.71"]);
        assert!(h.finish(&caught, true).is_empty());
        h.redraw("deal", plan_html("4.71", applied.then_some("4.71")));
        assert!(
            h.e.attr("apply", "disabled").is_some(),
            "applied={applied}: the game answered 4.71, form clean"
        );
    }
}

// ---------- Designer's rulings at 5983ff17 ----------

/// Htmx's flag (existing behaviour): after a redraw replaces a whole form,
/// Apply's `RequiresChange` greying is up to date in that same `sync`, with
/// and without `data-hx-applied`. Here it is not stale; #94's baseline
/// override (only while a change is held or unanswered) is not involved.
#[test]
fn apply_greying_is_current_after_a_form_redraw() {
    for applied in [false, true] {
        let mut h = plan_page(applied);
        let o1 = h.click("fee-up");
        assert!(h.e.attr("apply", "disabled").is_none(), "applied={applied}");
        let _ = h.finish(&o1, true);
        h.redraw("deal", plan_html("4.7", applied.then_some("4.7")));
        assert!(
            h.e.attr("apply", "disabled").is_some(),
            "applied={applied}: the game answered 4.7, the redrawn form is clean"
        );
    }
}
