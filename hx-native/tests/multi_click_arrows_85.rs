//! Issue #85 (Designer, HTML_CONTRACT §2 Ruling 7, Mainnet master 04c95f50,
//! Htmx ask on #86): arrow presses are not text clicks. Every press on ▲/▼
//! steps once, so a fast double-click on an arrow steps twice; pressing an
//! arrow never selects text; and an arrow press never counts toward a
//! double- or triple-click in the text.

use std::time::Duration;

use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#n{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}\
#other{position:absolute;left:0;top:40px}</style>";

const UP: (f32, f32) = (140.0, 6.0);
const DOWN: (f32, f32) = (140.0, 18.0);
/// Inside the box's text, left of the right edge.
const TEXT: (f32, f32) = (20.0, 12.0);

fn boot(value: &str) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.n"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
         <input id=\"n\" type=\"number\" name=\"n\" value=\"{value}\" min=\"0\" max=\"100\" step=\"0.01\" hx-post=\"deal.n\" hx-trigger=\"change\" />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"n\">▲</span>\
         <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"n\">▼</span>\
         <input id=\"other\" type=\"text\" name=\"other\" value=\"x\" /></form></body></html>"
    ))
    .unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
    // Frozen press clock: every press here is 0 ms after the last, well
    // inside the double-click window.
    e.test_advance_clock(Duration::ZERO);
    e
}

fn click(e: &mut Engine, (x, y): (f32, f32), ch: &mut Vec<String>) {
    let mods = Modifiers::default();
    let b = PointerButton::Primary;
    for ev in [
        InputEvent::PointerDown {
            x,
            y,
            button: b,
            mods,
        },
        InputEvent::PointerUp {
            x,
            y,
            button: b,
            mods,
        },
    ] {
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
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
}

fn selection(e: &mut Engine) -> String {
    match e.input(InputEvent::Copy) {
        InputOutcome::ClipboardWrite { text } => text,
        _ => String::new(),
    }
}

fn val(e: &Engine) -> String {
    e.attr("n", "value").unwrap_or_default()
}

#[test]
fn a_double_click_on_an_arrow_steps_twice_and_selects_nothing() {
    let mut e = boot("4.69");
    let mut ch = vec![];
    click(&mut e, UP, &mut ch);
    click(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "4.71", "two presses, two steps");
    assert_eq!(ch, ["4.7", "4.71"], "each press sends its own change");
    assert_eq!(selection(&mut e), "", "an arrow never selects text");
    click(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "4.72", "a triple-click steps three times");
    assert_eq!(selection(&mut e), "");
    let mut e = boot("4.69");
    let mut ch = vec![];
    click(&mut e, DOWN, &mut ch);
    click(&mut e, DOWN, &mut ch);
    assert_eq!(ch, ["4.68", "4.67"]);
    assert_eq!(selection(&mut e), "");
}

#[test]
fn an_arrow_press_then_a_text_click_is_a_single_click() {
    // ▲, then a click in the text: the caret, nothing selected.
    let mut e = boot("4.69");
    let mut ch = vec![];
    click(&mut e, UP, &mut ch);
    click(&mut e, TEXT, &mut ch);
    assert_eq!(
        selection(&mut e),
        "",
        "the arrow press is not the first click"
    );
    // A text click, ▲, then a text click on the same spot: still single.
    let mut e = boot("4.69");
    let mut ch = vec![];
    click(&mut e, TEXT, &mut ch);
    click(&mut e, UP, &mut ch);
    click(&mut e, TEXT, &mut ch);
    assert_eq!(
        selection(&mut e),
        "",
        "the arrow press breaks the click run"
    );
    // A double-click, ▲, then a click: not a triple-click.
    let mut e = boot("4.69");
    let mut ch = vec![];
    click(&mut e, TEXT, &mut ch);
    click(&mut e, TEXT, &mut ch);
    assert_eq!(
        selection(&mut e),
        "4.69",
        "the double-click selects the number"
    );
    click(&mut e, UP, &mut ch);
    assert_eq!(
        selection(&mut e),
        "",
        "the arrow press clears the selection (#81)"
    );
    click(&mut e, TEXT, &mut ch);
    assert_eq!(
        selection(&mut e),
        "",
        "and the next text click is a single click"
    );
    // Without the arrow, the same clicks do count (the test's own control).
    let mut e = boot("4.69");
    let mut ch = vec![];
    click(&mut e, TEXT, &mut ch);
    click(&mut e, TEXT, &mut ch);
    click(&mut e, TEXT, &mut ch);
    assert_eq!(
        selection(&mut e),
        "4.69",
        "a plain triple-click selects all"
    );
}
