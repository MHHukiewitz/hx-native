//! Graph review of #91 (issue #90, Ruling 5, Mainnet 98ac51a9): every path
//! that leaves a number box holding no number puts back the last value
//! sent, and no post carries an empty or broken number.

use hx_native::*;
use std::collections::HashMap;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#fee{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}\
#note{position:absolute;left:0;top:30px;width:120px;height:20px}\
#go{position:absolute;left:0;top:56px;width:60px;height:20px}\
#other{position:absolute;left:100px;top:56px;width:60px;height:20px}</style>";

const UP: (f32, f32) = (140.0, 6.0);

fn fee_html(value: &str) -> String {
    format!(
        "<input class=\"n\" id=\"fee\" type=\"number\" name=\"fee\" value=\"{value}\" min=\"0\" max=\"100\" step=\"0.01\" hx-post=\"deal.fee\" hx-trigger=\"change\" />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"fee\">▲</span>\
         <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"fee\">▼</span>"
    )
}

fn boot(value: &str) -> Engine {
    boot_min(value, "0")
}

fn boot_min(value: &str, min: &str) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 50,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee", "deal.send"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
         <div id=\"sec\" hx-trigger=\"slice:fee\" hx-target=\"#sec\" hx-swap=\"innerHTML\">{}</div>\
         <input id=\"note\" type=\"text\" name=\"note\" value=\"hi\" />\
         <button id=\"go\" type=\"button\" hx-post=\"deal.send\" hx-trigger=\"click, keydown[key=='F2'] from:body\">Send</button>\
         </form><div id=\"other\" hx-trigger=\"slice:other\" hx-target=\"#other\" hx-swap=\"innerHTML\">x</div></body></html>",
        fee_html(value).replace("min=\"0\"", &format!("min=\"{min}\""))
    ))
    .unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
    e
}

type Sent = (String, bool, Vec<(String, String)>);

fn feed_opt(e: &mut Engine, ev: InputEvent, out: &mut Vec<Sent>, complete: bool) {
    let d = e.input(ev);
    for o in std::iter::once(d).chain(e.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            name,
            trigger,
            args,
            ..
        } = o
        {
            out.push((name, matches!(trigger, TriggerInfo::Change), args));
            if complete {
                e.complete(request, Ok(()));
            }
        }
    }
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
}
fn feed(e: &mut Engine, ev: InputEvent, out: &mut Vec<Sent>) {
    feed_opt(e, ev, out, true)
}
fn key_ev(k: &str, pressed: bool, shift: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            shift,
            ..Default::default()
        },
        repeat: false,
        pressed,
    })
}
fn press_key(e: &mut Engine, k: &str, out: &mut Vec<Sent>) {
    feed(e, key_ev(k, true, false), out);
    feed(e, key_ev(k, false, false), out);
}
fn click_opt(e: &mut Engine, (x, y): (f32, f32), out: &mut Vec<Sent>, complete: bool) {
    let mods = Modifiers::default();
    let button = PointerButton::Primary;
    feed_opt(
        e,
        InputEvent::PointerDown { x, y, button, mods },
        out,
        complete,
    );
    feed_opt(
        e,
        InputEvent::PointerUp { x, y, button, mods },
        out,
        complete,
    );
}
fn click(e: &mut Engine, p: (f32, f32), out: &mut Vec<Sent>) {
    click_opt(e, p, out, true)
}
fn center(e: &Engine, id: &str) -> (f32, f32) {
    let b = e.test_element_box(id).unwrap();
    (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
}
/// Click into the fee and delete everything (select all + Backspace), or
/// type `typed` over it.
fn empty_fee(e: &mut Engine, typed: &str, out: &mut Vec<Sent>) {
    if e.focus_state().focused_id.as_deref() != Some("fee") {
        let p = center(e, "fee");
        click(e, p, out);
    }
    feed(
        e,
        InputEvent::Key(KeyEvent {
            key: "a".into(),
            code: "KeyA".into(),
            mods: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
            pressed: true,
        }),
        out,
    );
    if typed.is_empty() {
        press_key(e, "Backspace", out);
    } else {
        feed(e, InputEvent::Text(typed.into()), out);
    }
    assert_eq!(e.test_editor_display("fee").as_deref(), Some(typed));
    out.clear();
}
fn val(e: &Engine, id: &str) -> String {
    e.attr(id, "value").unwrap_or_default()
}
fn changes(out: &[Sent]) -> usize {
    out.iter().filter(|(_, c, _)| *c).count()
}
fn post_fee(out: &[Sent]) -> Option<String> {
    out.iter()
        .find(|(n, _, _)| n == "deal.send")
        .and_then(|(_, _, a)| a.iter().find(|(k, _)| k == "fee").map(|(_, v)| v.clone()))
}

struct Slices {
    v: HashMap<String, u64>,
    l: Vec<(String, String, InputLimits)>,
}
impl SliceSource for Slices {
    fn version(&self, n: &str) -> Option<u64> {
        self.v.get(n).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        self.l.clone()
    }
}
/// Renders `.0` for `#sec` (the fee box) and "y" for `#other`.
struct Frags(String);
impl FragmentSource for Frags {
    fn render(&mut self, section: &str, _: &[&str]) -> String {
        if section == "sec" {
            self.0.clone()
        } else {
            "y".into()
        }
    }
}

// --- Posts with no commit first --------------------------------------

/// A `from:body` shortcut posts "the value Enter would commit"; since #90
/// Enter on an empty box commits the last value sent, so the post must
/// carry 4.1, never fee="".
#[test]
fn g1_body_shortcut_on_an_empty_box_posts_the_sent_value() {
    for typed in ["", ".", "3,5"] {
        let mut e = boot("4.1");
        let mut out = vec![];
        empty_fee(&mut e, typed, &mut out);
        press_key(&mut e, "F2", &mut out);
        assert_eq!(
            post_fee(&out).as_deref(),
            Some("4.1"),
            "typed {typed:?} then F2: {out:?}"
        );
        // Then Tab: restores, sends nothing.
        out.clear();
        press_key(&mut e, "Tab", &mut out);
        assert_eq!(val(&e, "fee"), "4.1", "typed {typed:?}");
        assert_eq!(changes(&out), 0, "typed {typed:?}: {out:?}");
    }
}

/// A click on Send while the box is empty blurs it first.
#[test]
fn g2_click_send_while_empty_posts_the_sent_value() {
    for typed in ["", ".", "3,5"] {
        let mut e = boot("4.1");
        let mut out = vec![];
        empty_fee(&mut e, typed, &mut out);
        let go = center(&e, "go");
        click(&mut e, go, &mut out);
        assert_eq!(changes(&out), 0, "typed {typed:?}: {out:?}");
        assert_eq!(
            post_fee(&out).as_deref(),
            Some("4.1"),
            "typed {typed:?}: {out:?}"
        );
        assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
    }
}

// --- Other ways to leave ---------------------------------------------

#[test]
fn g3_shift_tab_restores() {
    let mut e = boot("4.1");
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    feed(&mut e, key_ev("Tab", true, true), &mut out);
    feed(&mut e, key_ev("Tab", false, true), &mut out);
    assert_ne!(e.focus_state().focused_id.as_deref(), Some("fee"));
    assert_eq!(val(&e, "fee"), "4.1");
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
    assert_eq!(changes(&out), 0, "{out:?}");
}

#[test]
fn g5_esc_on_an_empty_box_restores() {
    let mut e = boot("4.1");
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    press_key(&mut e, "Escape", &mut out);
    assert_eq!(val(&e, "fee"), "4.1");
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
    assert_eq!(changes(&out), 0);
}

// --- Arrows on an empty box (observation) ----------------------------

#[test]
fn g6_arrow_click_on_an_empty_box() {
    let mut e = boot("4.1");
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    click(&mut e, UP, &mut out);
    eprintln!("G6 ▲ on empty: value={} sent={out:?}", val(&e, "fee"));
    assert_ne!(val(&e, "fee"), "", "never empty after an arrow");
    press_key(&mut e, "Tab", &mut out);
    assert_ne!(val(&e, "fee"), "");
}

#[test]
fn g7_arrow_key_on_an_empty_box() {
    let mut e = boot("4.1");
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    press_key(&mut e, "ArrowUp", &mut out);
    eprintln!("G7 ArrowUp on empty: value={} sent={out:?}", val(&e, "fee"));
    assert_ne!(val(&e, "fee"), "");
    let n = out.len();
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(changes(&out[n..]), 0, "Tab after the arrow change: {out:?}");
}

// --- Sync and re-render while empty -----------------------------------

#[test]
fn g8_unrelated_swap_while_empty_then_tab_restores() {
    let mut e = boot("4.1");
    let mut out = vec![];
    let s = Slices {
        v: HashMap::from([("fee".into(), 1), ("other".into(), 1)]),
        l: vec![],
    };
    e.sync(&s, &mut Frags(fee_html("4.1"))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
    empty_fee(&mut e, "", &mut out);
    // Only #other changes: the fee box is not re-rendered.
    let s = Slices {
        v: HashMap::from([("fee".into(), 1), ("other".into(), 2)]),
        l: vec![],
    };
    e.sync(&s, &mut Frags(fee_html("9"))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
    assert_eq!(
        e.test_editor_display("fee").as_deref(),
        Some(""),
        "editor kept"
    );
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(val(&e, "fee"), "4.1");
    assert_eq!(changes(&out), 0, "{out:?}");
}

/// The box itself is re-rendered while empty; the new markup's value is the
/// game's, so it is what Tab puts back.
#[test]
fn g9_same_id_rerender_while_empty_then_tab() {
    for (game, want) in [("4.1", "4.1"), ("5", "5")] {
        let mut e = boot("4.1");
        let mut out = vec![];
        empty_fee(&mut e, "", &mut out);
        let s = Slices {
            v: HashMap::from([("fee".into(), 2)]),
            l: vec![],
        };
        e.sync(&s, &mut Frags(fee_html(game))).unwrap();
        let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
        eprintln!(
            "G9 game={game} after re-render display={:?} value={}",
            e.test_editor_display("fee"),
            val(&e, "fee")
        );
        press_key(&mut e, "Tab", &mut out);
        assert_eq!(val(&e, "fee"), want, "game {game}");
        assert_eq!(changes(&out), 0, "game {game}: {out:?}");
    }
}

/// A limits refresh while empty: the put-back value is held in the new
/// limits, like Esc.
#[test]
fn g10_limits_refresh_while_empty_clamps_the_restore() {
    let mut e = boot("4.1");
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    let s = Slices {
        v: HashMap::from([("actions".into(), 2)]),
        l: vec![(
            "deal.fee".into(),
            "fee".into(),
            InputLimits::new(0.0, 3.0, Some(0.01)),
        )],
    };
    e.sync(&s, &mut Frags(fee_html("4.1"))).unwrap();
    press_key(&mut e, "Tab", &mut out);
    eprintln!("G10 value={} sent={out:?}", val(&e, "fee"));
    assert_eq!(val(&e, "fee"), "3");
}

// --- Before anything was sent, #87, #89 ------------------------------

/// Nothing sent yet: the host's initial value, compared by number (#87).
#[test]
fn g11_host_value_is_the_fallback_by_number() {
    let mut e = boot("4.10");
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(changes(&out), 0, "{out:?}");
    assert!(
        ["4.1", "4.10"].contains(&val(&e, "fee").as_str()),
        "{}",
        val(&e, "fee")
    );
    let go = center(&e, "go");
    click(&mut e, go, &mut out);
    assert!(
        post_fee(&out).is_some_and(|v| v == "4.1" || v == "4.10"),
        "{out:?}"
    );
}

/// A host page that starts empty stays empty and sends nothing.
#[test]
fn g12_host_empty_stays_empty() {
    let mut e = boot("");
    let mut out = vec![];
    empty_fee(&mut e, ".", &mut out);
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(val(&e, "fee"), "");
    assert_eq!(changes(&out), 0, "{out:?}");
}

/// #89 neighbour: ▲ sent 4.11 but the game has not answered; the box is
/// emptied and left. The put-back value is the sent 4.11, with no change.
#[test]
fn g13_pending_arrow_change_then_empty_restores_the_sent_value() {
    let mut e = boot("4.1");
    let mut out = vec![];
    click_opt(&mut e, UP, &mut out, false);
    assert_eq!(changes(&out), 1, "{out:?}");
    empty_fee(&mut e, "", &mut out);
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(val(&e, "fee"), "4.11");
    assert_eq!(changes(&out), 0, "{out:?}");
}

/// Enter on an empty box keeps the focus, shows the sent value with the
/// caret at the end, and typing then appends.
#[test]
fn g14_enter_then_type_appends() {
    let mut e = boot("4.1");
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    press_key(&mut e, "Enter", &mut out);
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
    feed(&mut e, InputEvent::Text("5".into()), &mut out);
    eprintln!("G14 after Enter + '5': {:?}", e.test_editor_display("fee"));
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.105"));
}

/// Text boxes keep the empty text and post it.
#[test]
fn g15_text_box_empty_posts_empty() {
    let mut e = boot("4.1");
    let mut out = vec![];
    let p = center(&e, "note");
    click(&mut e, p, &mut out);
    feed(
        &mut e,
        InputEvent::Key(KeyEvent {
            key: "a".into(),
            code: "KeyA".into(),
            mods: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
            pressed: true,
        }),
        &mut out,
    );
    press_key(&mut e, "Backspace", &mut out);
    out.clear();
    press_key(&mut e, "F2", &mut out);
    let note = out
        .iter()
        .find(|(n, _, _)| n == "deal.send")
        .and_then(|(_, _, a)| a.iter().find(|(k, _)| k == "note").map(|(_, v)| v.clone()));
    assert_eq!(note.as_deref(), Some(""), "{out:?}");
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(val(&e, "note"), "");
}

/// The lone sign, where `min` allows it (as in #91's own tests).
#[test]
fn g16_body_shortcut_on_a_lone_minus_posts_the_sent_value() {
    for typed in ["-", "-.", "+"] {
        let mut e = boot_min("4.1", "-100");
        let mut out = vec![];
        let p = center(&e, "fee");
        click(&mut e, p, &mut out);
        feed(
            &mut e,
            InputEvent::Key(KeyEvent {
                key: "a".into(),
                code: "KeyA".into(),
                mods: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                repeat: false,
                pressed: true,
            }),
            &mut out,
        );
        feed(&mut e, InputEvent::Text(typed.into()), &mut out);
        let shown = e.test_editor_display("fee");
        out.clear();
        press_key(&mut e, "F2", &mut out);
        assert_eq!(
            post_fee(&out).as_deref(),
            Some("4.1"),
            "typed {typed:?} (shown {shown:?}) then F2: {out:?}"
        );
        assert_eq!(
            e.test_editor_display("fee"),
            shown,
            "F2 keeps the typed text"
        );
        out.clear();
        press_key(&mut e, "Tab", &mut out);
        assert_eq!(val(&e, "fee"), "4.1");
        assert_eq!(changes(&out), 0, "{out:?}");
    }
}
