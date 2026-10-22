//! Issue #87 (Designer, HTML_CONTRACT §2 Ruling 5, Mainnet master 04c95f50):
//! a number box compares its value with the last value sent by number, not
//! by text. 4.10 stepped away and back sends nothing even though the box
//! now reads 4.1; a value that parses equal to the sent one never sends;
//! text that does not parse as a number sends nothing.

use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#n{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}\
#other{position:absolute;left:0;top:40px}</style>";

const UP: (f32, f32) = (140.0, 6.0);
const DOWN: (f32, f32) = (140.0, 18.0);

fn boot_with(value: &str, step: &str, key_releases: bool) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        key_releases,
        ..Default::default()
    });
    e.declare_actions(&["deal.n"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
         <input id=\"n\" type=\"number\" name=\"n\" value=\"{value}\" min=\"0\" max=\"100\" step=\"{step}\" hx-post=\"deal.n\" hx-trigger=\"change\" />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"n\">▲</span>\
         <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"n\">▼</span>\
         <input id=\"other\" type=\"text\" name=\"other\" value=\"x\" /></form></body></html>"
    ))
    .unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
    e
}

fn boot(value: &str) -> Engine {
    boot_with(value, "0.01", false)
}

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
                    .find(|(k, _)| k == "n")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default(),
            );
            e.complete(request, Ok(()));
        }
    }
}

fn key(k: &str, pressed: bool, repeat: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat,
        pressed,
    })
}

fn click(e: &mut Engine, (x, y): (f32, f32), ch: &mut Vec<String>) {
    let mods = Modifiers::default();
    let b = PointerButton::Primary;
    feed(
        e,
        InputEvent::PointerDown {
            x,
            y,
            button: b,
            mods,
        },
        ch,
    );
    feed(
        e,
        InputEvent::PointerUp {
            x,
            y,
            button: b,
            mods,
        },
        ch,
    );
}

/// Tab into the box (selects all) and type `text` over it.
fn retype(e: &mut Engine, text: &str, ch: &mut Vec<String>) {
    e.test_set_focus("other");
    feed(e, key("Tab", true, false), ch);
    feed(e, key("Tab", false, false), ch);
    feed(e, InputEvent::Text(text.into()), ch);
}

fn val(e: &Engine) -> String {
    e.attr("n", "value").unwrap_or_default()
}

#[test]
fn arrow_click_back_onto_a_sent_4_10_sends_nothing() {
    let mut e = boot("4.10");
    let mut ch = vec![];
    retype(&mut e, "4.11", &mut ch);
    click(&mut e, DOWN, &mut ch);
    assert!(val(&e) == "4.1" || val(&e) == "4.10", "{}", val(&e));
    assert_eq!(ch, Vec::<String>::new(), "4.1 is the sent 4.10 as a number");
    feed(&mut e, key("Tab", true, false), &mut ch);
    assert_eq!(
        ch,
        Vec::<String>::new(),
        "and the blur sends nothing either"
    );
}

/// ▲ sends 4.11; ▼ back to 4.10 is a real move from the sent 4.11; a
/// retyped 4.10 then equals the sent 4.1 and sends nothing.
#[test]
fn arrow_step_away_and_back_sends_each_real_move() {
    let mut e = boot("4.10");
    let mut ch = vec![];
    click(&mut e, UP, &mut ch);
    click(&mut e, DOWN, &mut ch);
    assert_eq!(ch.len(), 2, "{ch:?}");
    assert_eq!(ch[0], "4.11");
    let back = ch[1].clone();
    assert!(back == "4.1" || back == "4.10", "{back}");
    retype(&mut e, "4.10", &mut ch);
    feed(&mut e, key("Enter", true, false), &mut ch);
    assert_eq!(ch.len(), 2, "4.10 is the sent {back}: {ch:?}");
}

#[test]
fn hold_release_back_onto_a_sent_4_10_sends_nothing() {
    let mut e = boot("4.10");
    let mut ch = vec![];
    retype(&mut e, "4.11", &mut ch);
    let mods = Modifiers::default();
    let b = PointerButton::Primary;
    feed(
        &mut e,
        InputEvent::PointerDown {
            x: DOWN.0,
            y: DOWN.1,
            button: b,
            mods,
        },
        &mut ch,
    );
    // Released off the arrow: still the release path.
    feed(
        &mut e,
        InputEvent::PointerUp {
            x: 10.0,
            y: 70.0,
            button: b,
            mods,
        },
        &mut ch,
    );
    assert_eq!(ch, Vec::<String>::new(), "{}", val(&e));
}

#[test]
fn key_step_back_onto_a_sent_4_10_sends_nothing_on_both_host_kinds() {
    for key_releases in [false, true] {
        let mut e = boot_with("4.10", "0.01", key_releases);
        let mut ch = vec![];
        retype(&mut e, "4.11", &mut ch);
        feed(&mut e, key("ArrowDown", true, false), &mut ch);
        feed(&mut e, key("ArrowDown", false, false), &mut ch);
        assert_eq!(
            ch,
            Vec::<String>::new(),
            "key_releases={key_releases}: {}",
            val(&e)
        );
    }
}

#[test]
fn enter_or_blur_on_a_retyped_4_10_sends_nothing() {
    for (typed, end) in [
        ("4.10", "Enter"),
        ("4.1", "Enter"),
        ("4.100", "Tab"),
        ("04.1", "Tab"),
    ] {
        let mut e = boot("4.10");
        let mut ch = vec![];
        retype(&mut e, typed, &mut ch);
        feed(&mut e, key(end, true, false), &mut ch);
        assert_eq!(ch, Vec::<String>::new(), "typed {typed:?} then {end}");
    }
    // A different number still sends.
    let mut e = boot("4.10");
    let mut ch = vec![];
    retype(&mut e, "4.2", &mut ch);
    feed(&mut e, key("Enter", true, false), &mut ch);
    assert_eq!(ch.len(), 1, "{ch:?}");
}

#[test]
fn whole_numbers_compare_by_number_too() {
    let mut e = boot_with("7", "1", false);
    let mut ch = vec![];
    retype(&mut e, "07", &mut ch);
    feed(&mut e, key("Enter", true, false), &mut ch);
    assert_eq!(ch, Vec::<String>::new(), "07 is the sent 7");
}

/// Text that does not parse as a number is not a value: it sends nothing.
#[test]
fn unparsable_text_sends_nothing() {
    for typed in ["3,5", "-", ".", "1,,2"] {
        let mut e = boot("4.10");
        let mut ch = vec![];
        retype(&mut e, typed, &mut ch);
        feed(&mut e, key("Enter", true, false), &mut ch);
        feed(&mut e, key("Tab", true, false), &mut ch);
        assert_eq!(ch, Vec::<String>::new(), "typed {typed:?}");
    }
    // A host value that is not a number: Enter and an arrow onto nothing
    // new send nothing.
    let mut e = boot("abc");
    let mut ch = vec![];
    retype(&mut e, "", &mut ch);
    feed(&mut e, key("Enter", true, false), &mut ch);
    assert_eq!(ch, Vec::<String>::new(), "{}", val(&e));
}

/// Text boxes keep the text compare: `4.1` over a sent `4.10` is a change.
#[test]
fn a_text_box_still_compares_text() {
    let mut e = Engine::new(EngineConfig::default());
    e.declare_actions(&["deal.t"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body>\
         <input id=\"n\" type=\"text\" name=\"n\" value=\"4.10\" hx-post=\"deal.t\" hx-trigger=\"change\" />\
         <input id=\"other\" type=\"text\" name=\"other\" value=\"x\" /></body></html>"
    ))
    .unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
    let mut ch = vec![];
    retype(&mut e, "4.1", &mut ch);
    feed(&mut e, key("Enter", true, false), &mut ch);
    assert_eq!(ch, ["4.1".to_string()]);
}
