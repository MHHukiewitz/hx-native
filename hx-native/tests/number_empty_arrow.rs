//! Issue #92 (Designer, HTML_CONTRACT §2 Ruling 5, Mainnet master cea8a42a):
//! an arrow press or Up/Down key on a number box that is empty or does not
//! parse steps from the last value sent, not from `min` or 0, so the arrows
//! agree with what blur or Enter would put back (#90): ▲ on a fee cleared
//! from 4.10 reads 4.11. Before anything is sent it steps from the box's
//! starting value. Graph, review of #91 (comment 6340237214).

use std::time::Duration;

use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#fee{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}\
#other{position:absolute;left:0;top:40px}</style>";

const UP: (f32, f32) = (140.0, 6.0);
const DOWN: (f32, f32) = (140.0, 18.0);

/// A fee box; `min="-100"` so a typed `-` is allowed.
fn boot_with(value: &str, key_releases: bool) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        key_releases,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
         <input class=\"n\" id=\"fee\" type=\"number\" name=\"fee\" value=\"{value}\" min=\"-100\" max=\"100\" step=\"0.01\" hx-post=\"deal.fee\" hx-trigger=\"change\" />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"fee\">▲</span>\
         <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"fee\">▼</span>\
         <input id=\"other\" type=\"text\" name=\"other\" value=\"x\" />\
         </form></body></html>"
    ))
    .unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
    e
}

fn boot(value: &str) -> Engine {
    boot_with(value, false)
}

/// Feed one event; collect the `fee` of every `change` it produced.
fn feed(e: &mut Engine, ev: InputEvent, ch: &mut Vec<String>) {
    let d = e.input(ev);
    for o in std::iter::once(d).chain(e.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            trigger: TriggerInfo::Change,
            args,
            ..
        } = o
        {
            ch.push(
                args.iter()
                    .find(|(k, _)| k == "fee")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default(),
            );
            e.complete(request, Ok(()));
        }
    }
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
}

fn key(k: &str, pressed: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed,
    })
}

fn down(e: &mut Engine, (x, y): (f32, f32), ch: &mut Vec<String>) {
    let mods = Modifiers::default();
    let button = PointerButton::Primary;
    feed(e, InputEvent::PointerDown { x, y, button, mods }, ch);
}

fn up(e: &mut Engine, (x, y): (f32, f32), ch: &mut Vec<String>) {
    let mods = Modifiers::default();
    let button = PointerButton::Primary;
    feed(e, InputEvent::PointerUp { x, y, button, mods }, ch);
}

fn click(e: &mut Engine, at: (f32, f32), ch: &mut Vec<String>) {
    down(e, at, ch);
    up(e, at, ch);
}

/// Tab into the fee (all selected, #68) and replace its text with `typed`;
/// "" empties it.
fn retype(e: &mut Engine, typed: &str) {
    let mut ch = vec![];
    e.test_set_focus("other");
    feed(e, key("Tab", true), &mut ch);
    feed(e, key("Tab", false), &mut ch);
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("fee"));
    if typed.is_empty() {
        feed(e, key("Backspace", true), &mut ch);
        feed(e, key("Backspace", false), &mut ch);
    } else {
        feed(e, InputEvent::Text(typed.into()), &mut ch);
    }
    assert_eq!(
        e.test_editor_display("fee").as_deref(),
        Some(typed),
        "box holds {typed:?}"
    );
    assert!(ch.is_empty(), "{ch:?}");
}

fn val(e: &Engine) -> String {
    e.attr("fee", "value").unwrap_or_default()
}

#[test]
fn arrows_on_an_emptied_box_step_from_the_sent_value() {
    let mut e = boot("4.10");
    let mut ch = vec![];
    retype(&mut e, "");
    click(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "4.11", "▲ on a fee cleared from 4.10");
    assert_eq!(ch, ["4.11"]);
    let mut e = boot("4.10");
    let mut ch = vec![];
    retype(&mut e, "");
    click(&mut e, DOWN, &mut ch);
    assert_eq!(val(&e), "4.09", "▼ on a fee cleared from 4.10");
    assert_eq!(ch, ["4.09"]);
}

#[test]
fn keys_on_an_emptied_box_step_from_the_sent_value() {
    for (k, want) in [("ArrowUp", "4.11"), ("ArrowDown", "4.09")] {
        let mut e = boot("4.10");
        let mut ch = vec![];
        retype(&mut e, "");
        feed(&mut e, key(k, true), &mut ch);
        feed(&mut e, key(k, false), &mut ch);
        assert_eq!(val(&e), want, "{k}");
        assert_eq!(ch, [want], "{k}");
    }
}

#[test]
fn a_lone_minus_or_a_refused_comma_then_up_steps_from_the_sent_value() {
    for typed in ["-", ".", "3,5"] {
        let mut e = boot("4.10");
        let mut ch = vec![];
        retype(&mut e, typed);
        click(&mut e, UP, &mut ch);
        assert_eq!(val(&e), "4.11", "{typed:?} then ▲");
        assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.11"));
        assert_eq!(ch, ["4.11"], "{typed:?}");
    }
}

/// "The last value sent" is the last `change`; before any, the starting
/// value.
#[test]
fn nothing_sent_yet_steps_from_the_starting_value_then_from_the_last_change() {
    let mut e = boot("7.5");
    let mut ch = vec![];
    retype(&mut e, "");
    feed(&mut e, key("ArrowUp", true), &mut ch);
    feed(&mut e, key("ArrowUp", false), &mut ch);
    assert_eq!(val(&e), "7.51", "nothing sent yet: from the starting 7.5");
    assert_eq!(ch, ["7.51"]);
    // 7.51 is now the last value sent; empty the box again.
    for _ in 0..4 {
        feed(&mut e, key("Backspace", true), &mut ch);
        feed(&mut e, key("Backspace", false), &mut ch);
    }
    assert_eq!(e.test_editor_display("fee").as_deref(), Some(""));
    click(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "7.52", "from the sent 7.51");
    assert_eq!(ch, ["7.51", "7.52"]);
}

/// The step from the sent value clamps at max and min like any step; a
/// step that ends on the sent value sends nothing (Ruling 5).
#[test]
fn the_step_clamps_at_max_and_min() {
    let mut e = boot("100");
    let mut ch = vec![];
    retype(&mut e, "");
    click(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "100", "▲ on a box cleared from max stays at max");
    assert!(ch.is_empty(), "100 is the sent value: {ch:?}");
    let mut e = boot("99.995");
    let mut ch = vec![];
    retype(&mut e, "");
    click(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "100");
    assert_eq!(ch, ["100"]);
    let mut e = boot("-100");
    let mut ch = vec![];
    retype(&mut e, "");
    click(&mut e, DOWN, &mut ch);
    assert_eq!(val(&e), "-100", "▼ on a box cleared from min stays at min");
    assert!(ch.is_empty(), "{ch:?}");
    let mut e = boot("-99.99");
    let mut ch = vec![];
    retype(&mut e, "");
    feed(&mut e, key("ArrowDown", true), &mut ch);
    feed(&mut e, key("ArrowDown", false), &mut ch);
    assert_eq!(val(&e), "-100");
    assert_eq!(ch, ["-100"]);
}

#[test]
fn a_held_arrow_on_an_emptied_box_repeats_from_the_sent_value() {
    let mut e = boot("4.10");
    let mut ch = vec![];
    retype(&mut e, "");
    down(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "4.11");
    e.test_hold_spin_for(Duration::from_millis(500));
    assert_eq!(val(&e), "4.13", "two repeats by 500 ms");
    assert!(ch.is_empty(), "nothing until the release: {ch:?}");
    up(&mut e, UP, &mut ch);
    assert_eq!(ch, ["4.13"]);
}

#[test]
fn key_releases_on_an_emptied_box_step_from_the_sent_value() {
    let mut e = boot_with("4.10", true);
    let mut ch = vec![];
    retype(&mut e, "");
    feed(&mut e, key("ArrowUp", true), &mut ch);
    assert_eq!(val(&e), "4.11");
    assert!(
        ch.is_empty(),
        "keydown sends nothing with key_releases: {ch:?}"
    );
    feed(&mut e, key("ArrowUp", false), &mut ch);
    assert_eq!(ch, ["4.11"], "one change on the key release");
}
