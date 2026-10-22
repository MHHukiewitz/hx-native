//! Issue #90 (Designer, HTML_CONTRACT §2 Ruling 5, Mainnet master 98ac51a9):
//! leaving a number box (blur, Tab, Enter) that is empty or does not parse,
//! such as a lone `-` or `3,5`, puts back the last value sent and sends
//! nothing, so a later form post never carries an empty or broken number.
//! Graph on #86 (comment 6339127880): since #87 an emptied fee stayed empty
//! while the game kept 4.1, and the next post carried `fee=""`.

use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#fee{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}\
#note{position:absolute;left:0;top:30px;width:120px;height:20px}\
#go{position:absolute;left:0;top:56px;width:60px;height:20px}</style>";

const UP: (f32, f32) = (140.0, 6.0);

fn boot(value: &str) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee", "deal.send"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
         <input class=\"n\" id=\"fee\" type=\"number\" name=\"fee\" value=\"{value}\" min=\"-100\" max=\"100\" step=\"0.01\" hx-post=\"deal.fee\" hx-trigger=\"change\" />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"fee\">▲</span>\
         <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"fee\">▼</span>\
         <input id=\"note\" type=\"text\" name=\"note\" value=\"hi\" hx-post=\"deal.fee\" hx-trigger=\"change\" />\
         <button id=\"go\" type=\"button\" hx-post=\"deal.send\">Send</button>\
         </form></body></html>"
    ))
    .unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
    e
}

/// One `Action` an event produced: (name, trigger is change, args).
type Sent = (String, bool, Vec<(String, String)>);

/// Collect every `Action` an event produced.
fn feed(e: &mut Engine, ev: InputEvent, out: &mut Vec<Sent>) {
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

fn press_key(e: &mut Engine, k: &str, out: &mut Vec<Sent>) {
    feed(e, key(k, true), out);
    feed(e, key(k, false), out);
}

fn click(e: &mut Engine, (x, y): (f32, f32), out: &mut Vec<Sent>) {
    let mods = Modifiers::default();
    let button = PointerButton::Primary;
    feed(e, InputEvent::PointerDown { x, y, button, mods }, out);
    feed(e, InputEvent::PointerUp { x, y, button, mods }, out);
}

fn center(e: &Engine, id: &str) -> (f32, f32) {
    let b = e.test_element_box(id).unwrap();
    (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
}

/// Focus the box with everything selected (Tab in, #68) and replace the
/// text with `typed`; "" deletes it.
fn retype(e: &mut Engine, id: &str, typed: &str, out: &mut Vec<Sent>) {
    e.test_set_focus("go");
    press_key(e, "Tab", out);
    // From `go`, Tab wraps to the first field; walk to `id`.
    for _ in 0..4 {
        if e.focus_state().focused_id.clone().as_deref() == Some(id) {
            break;
        }
        press_key(e, "Tab", out);
    }
    assert_eq!(
        e.focus_state().focused_id.clone().as_deref(),
        Some(id),
        "focus reached {id}"
    );
    if typed.is_empty() {
        press_key(e, "Backspace", out);
    } else {
        feed(e, InputEvent::Text(typed.into()), out);
    }
    out.clear();
}

fn val(e: &Engine, id: &str) -> String {
    e.attr(id, "value").unwrap_or_default()
}

fn changes(out: &[Sent]) -> usize {
    out.iter().filter(|(_, c, _)| *c).count()
}

fn arg<'a>(args: &'a [(String, String)], k: &str) -> Option<&'a str> {
    args.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

#[test]
fn emptied_then_blurred_puts_back_the_sent_value() {
    let mut e = boot("4.1");
    let mut out = vec![];
    retype(&mut e, "fee", "", &mut out);
    assert_eq!(
        e.test_editor_display("fee").as_deref(),
        Some(""),
        "the box is empty while editing"
    );
    // Blur by a click elsewhere on the page.
    click(&mut e, (190.0, 75.0), &mut out);
    assert_ne!(e.focus_state().focused_id.clone().as_deref(), Some("fee"));
    assert_eq!(val(&e, "fee"), "4.1");
    assert_eq!(
        e.test_editor_display("fee").as_deref(),
        Some("4.10"),
        "the display too"
    );
    assert_eq!(changes(&out), 0, "{out:?}");
}

#[test]
fn a_lone_minus_then_tab_puts_back_the_sent_value() {
    let mut e = boot("4.1");
    let mut out = vec![];
    retype(&mut e, "fee", "-", &mut out);
    assert_eq!(
        e.test_editor_display("fee").as_deref(),
        Some("-"),
        "min -100 allows the sign"
    );
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(val(&e, "fee"), "4.1");
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
    assert_eq!(changes(&out), 0, "{out:?}");
}

#[test]
fn a_refused_comma_then_enter_puts_back_the_sent_value() {
    let mut e = boot("4.1");
    let mut out = vec![];
    retype(&mut e, "fee", "3,5", &mut out);
    press_key(&mut e, "Enter", &mut out);
    assert_eq!(
        e.focus_state().focused_id.clone().as_deref(),
        Some("fee"),
        "Enter keeps the focus"
    );
    assert_eq!(val(&e, "fee"), "4.1");
    assert_eq!(
        e.test_editor_display("fee").as_deref(),
        Some("4.10"),
        "the box shows it again"
    );
    assert_eq!(changes(&out), 0, "{out:?}");
}

/// A bare point is no number either (it commits like a lone `-`).
#[test]
fn a_bare_point_then_enter_puts_back_the_sent_value() {
    let mut e = boot("4.1");
    let mut out = vec![];
    retype(&mut e, "fee", ".", &mut out);
    press_key(&mut e, "Enter", &mut out);
    assert_eq!(val(&e, "fee"), "4.1");
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
    assert_eq!(changes(&out), 0, "{out:?}");
}

/// "The last value sent" is the last `change`, not the page's value: after
/// ▲ sends 4.11, an emptied box comes back to 4.11.
#[test]
fn the_restore_is_the_last_value_sent_after_an_arrow() {
    let mut e = boot("4.1");
    let mut out = vec![];
    click(&mut e, UP, &mut out);
    assert_eq!(changes(&out), 1);
    assert_eq!(val(&e, "fee"), "4.11");
    press_key(&mut e, "Backspace", &mut out);
    press_key(&mut e, "Backspace", &mut out);
    press_key(&mut e, "Backspace", &mut out);
    press_key(&mut e, "Backspace", &mut out);
    press_key(&mut e, "Backspace", &mut out);
    assert_eq!(e.test_editor_display("fee").as_deref(), Some(""));
    out.clear();
    press_key(&mut e, "Enter", &mut out);
    assert_eq!(val(&e, "fee"), "4.11");
    assert_eq!(changes(&out), 0, "{out:?}");
}

#[test]
fn the_next_form_post_carries_the_restored_value() {
    for (typed, leave) in [
        ("", "Tab"),
        ("-", "Tab"),
        ("3,5", "Enter"),
        ("", "Enter"),
        (".", "Enter"),
    ] {
        let mut e = boot("4.1");
        let mut out = vec![];
        retype(&mut e, "fee", typed, &mut out);
        press_key(&mut e, leave, &mut out);
        out.clear();
        let go = center(&e, "go");
        click(&mut e, go, &mut out);
        let post = out
            .iter()
            .find(|(n, _, _)| n == "deal.send")
            .unwrap_or_else(|| panic!("typed {typed:?} {leave}: the button posts: {out:?}"));
        assert_eq!(
            arg(&post.2, "fee"),
            Some("4.1"),
            "typed {typed:?} then {leave}: {post:?}"
        );
    }
}

/// Text boxes are not numbers: emptying one commits the empty text.
#[test]
fn text_boxes_are_unaffected() {
    let mut e = boot("4.1");
    let mut out = vec![];
    retype(&mut e, "note", "", &mut out);
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(val(&e, "note"), "");
    assert_eq!(
        changes(&out),
        1,
        "an emptied text box still reports its change: {out:?}"
    );
    let mut e = boot("4.1");
    let mut out = vec![];
    retype(&mut e, "note", "-", &mut out);
    press_key(&mut e, "Enter", &mut out);
    assert_eq!(val(&e, "note"), "-");
    assert_eq!(changes(&out), 1);
}
