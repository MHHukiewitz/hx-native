//! Issue #85 (Designer, HTML_CONTRACT §2 Ruling 7, Mainnet master bdcdfbca):
//! one click places the caret (#68), a double-click selects the word under
//! it, and a triple-click or more selects the whole field. In a number box
//! the whole number is one word, so `.` and `,` don't split it.
//!
//! Mainnet #751 item 7: triple-click the fee "4.69", type 4.69, Tab gave
//! "4.46969" because every press only placed the caret.

use std::time::Duration;

use hx_native::*;

const STYLE: &str = "<style>*{box-sizing:border-box}html,body{margin:0;background:#fff;\
font:11px \"DejaVu Sans\"}input{position:absolute;left:20px;width:120px;height:20px}\
#fee{top:10px;text-align:right}#big{top:40px;text-align:right}#txt{top:70px}\
#other{top:100px}#neg{top:130px;width:60px;text-align:right}\
#l{top:160px;width:60px}#r{top:160px;left:80px;width:60px}</style>";

fn page() -> String {
    format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body>\
         <input type=\"number\" id=\"fee\" name=\"fee\" value=\"4.69\" step=\"0.01\" min=\"0\" max=\"100.00\" data-hx-group=\"on\" hx-post=\"deal.n\" hx-trigger=\"change\">\
         <input type=\"number\" id=\"big\" name=\"big\" value=\"1234.56\" step=\"0.01\" data-hx-group=\"on\">\
         <input type=\"text\" id=\"txt\" name=\"txt\" value=\"hello brave world\">\
         <input type=\"text\" id=\"other\" name=\"other\" value=\"x\">\
         <input type=\"number\" id=\"neg\" name=\"neg\" value=\"-4.69\" step=\"0.01\" min=\"-100\">\
         <input type=\"text\" id=\"l\" name=\"l\" value=\"left\"><input type=\"text\" id=\"r\" name=\"r\" value=\"right\">\
         </body></html>"
    )
}

fn vp() -> Viewport {
    Viewport::with_scale(200, 190, 1.0)
}

fn engine() -> Engine {
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.n"]);
    e.load_html(&page()).unwrap();
    let _ = e.paint(vp()).unwrap();
    e
}

fn press(e: &mut Engine, x: f32, y: f32, button: PointerButton) {
    let mods = Modifiers::default();
    e.input(InputEvent::PointerDown { x, y, button, mods });
    e.input(InputEvent::PointerUp { x, y, button, mods });
    let _ = e.paint(vp());
}

fn click(e: &mut Engine, x: f32, y: f32) {
    press(e, x, y, PointerButton::Primary);
}

fn clicks(e: &mut Engine, (x, y): (f32, f32), n: usize) {
    for _ in 0..n {
        click(e, x, y);
    }
}

fn selection(e: &mut Engine) -> String {
    match e.input(InputEvent::Copy) {
        InputOutcome::ClipboardWrite { text } => text,
        _ => String::new(),
    }
}

/// A point `frac` of the way across `id`'s content box, vertically centred.
fn at(e: &Engine, id: &str, frac: f32) -> (f32, f32) {
    let c = e.test_content_box(id).unwrap();
    (c.0 + c.2 * frac, c.1 + c.3 / 2.0)
}

/// Inside "69" of the right-aligned "4.69".
fn inside_69(e: &Engine) -> (f32, f32) {
    let c = e.test_content_box("fee").unwrap();
    (c.0 + c.2 - 4.0, c.1 + c.3 / 2.0)
}

fn key(e: &mut Engine, k: &str) -> Vec<InputOutcome> {
    let ev = |pressed| {
        InputEvent::Key(KeyEvent {
            key: k.into(),
            code: k.into(),
            mods: Modifiers::default(),
            repeat: false,
            pressed,
        })
    };
    let mut out = vec![e.input(ev(true))];
    out.push(e.input(ev(false)));
    out.extend(e.take_outcomes());
    let _ = e.paint(vp());
    out
}

fn fee_changes(outs: &[InputOutcome]) -> Vec<String> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action {
                trigger: TriggerInfo::Change,
                args,
                ..
            } => args
                .iter()
                .find(|(k, _)| k == "fee")
                .map(|(_, v)| v.clone()),
            _ => None,
        })
        .collect()
}

fn type_str(e: &mut Engine, s: &str) {
    for ch in s.chars() {
        e.input(InputEvent::Text(ch.to_string()));
    }
    let _ = e.paint(vp());
}

#[test]
fn triple_click_selects_the_whole_field_and_typing_replaces_it() {
    let mut e = engine();
    let p = inside_69(&e);
    clicks(&mut e, p, 3);
    assert_eq!(
        selection(&mut e),
        "4.69",
        "triple-click selects the whole field"
    );
    type_str(&mut e, "4.69");
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.69"));
}

#[test]
fn double_click_selects_the_whole_number_in_a_number_box() {
    for frac in [0.15_f32, 0.5, 0.85, 0.97] {
        let mut e = engine();
        let p = at(&e, "fee", frac);
        clicks(&mut e, p, 2);
        assert_eq!(
            selection(&mut e),
            "4.69",
            "double-click at {frac}: '.' must not split"
        );
    }
    let mut e = engine();
    let p = inside_69(&e);
    clicks(&mut e, p, 2);
    assert_eq!(selection(&mut e), "4.69");
}

/// The sign belongs to the number too: a word break would split "-4.69"
/// at the minus.
#[test]
fn double_click_selects_a_negative_number_with_its_sign() {
    for frac in [0.6_f32, 0.8, 0.95] {
        let mut e = engine();
        let p = at(&e, "neg", frac);
        clicks(&mut e, p, 2);
        assert_eq!(selection(&mut e), "-4.69", "double-click at {frac}");
    }
}

/// '1,234.56' is one word too: neither the grouping comma nor the decimal
/// point splits it, whatever part of it is double-clicked.
#[test]
fn double_click_selects_a_grouped_number_whole() {
    for frac in [0.6_f32, 0.75, 0.9, 0.97] {
        let mut e = engine();
        assert_eq!(e.test_editor_display("big").as_deref(), Some("1,234.56"));
        let p = at(&e, "big", frac);
        click(&mut e, p.0, p.1);
        let shown = e.test_editor_display("big").unwrap_or_default();
        click(&mut e, p.0, p.1);
        let sel = selection(&mut e);
        assert_eq!(
            sel, shown,
            "double-click at {frac} selects all of {shown:?}"
        );
        assert!(sel.contains('.'), "{sel:?}");
    } // A typed grouping comma stays while editing (#51); still one word.
    let mut e = engine();
    let p = at(&e, "big", 0.9);
    clicks(&mut e, p, 3);
    type_str(&mut e, "1,234.56");
    assert_eq!(e.test_editor_display("big").as_deref(), Some("1,234.56"));
    for frac in [0.6_f32, 0.75, 0.9] {
        e.test_age_last_click(Duration::from_secs(1));
        let p = at(&e, "big", frac);
        clicks(&mut e, p, 2);
        assert_eq!(
            selection(&mut e),
            "1,234.56",
            "typed text, double-click at {frac}"
        );
    }
}

#[test]
fn double_click_in_a_text_box_selects_only_the_word() {
    let mut e = engine();
    // Find the point over "brave": click once, read where the caret went.
    let mut found = None;
    for i in 1..40 {
        let frac = i as f32 / 40.0;
        let mut probe = engine();
        let p = at(&probe, "txt", frac);
        clicks(&mut probe, p, 2);
        if selection(&mut probe) == "brave" {
            found = Some(frac);
            break;
        }
    }
    let frac = found.expect("some double-click in the text box selects \"brave\"");
    let p = at(&e, "txt", frac);
    clicks(&mut e, p, 2);
    assert_eq!(selection(&mut e), "brave");
    let p = at(&e, "txt", frac);
    clicks(&mut e, p, 1);
    assert_eq!(
        selection(&mut e),
        "hello brave world",
        "third click selects the field"
    );
}

#[test]
fn four_or_more_clicks_keep_the_whole_field() {
    let mut e = engine();
    let p = at(&e, "txt", 0.1);
    for n in 3..=6 {
        clicks(&mut e, p, if n == 3 { 3 } else { 1 });
        assert_eq!(selection(&mut e), "hello brave world", "{n} clicks");
    }
}

/// #68 stays: one click places the caret, nothing selected.
#[test]
fn single_click_still_only_places_the_caret() {
    let mut e = engine();
    let p = inside_69(&e);
    clicks(&mut e, p, 1);
    assert_eq!(selection(&mut e), "");
    let mut e = engine();
    let p = at(&e, "txt", 0.3);
    clicks(&mut e, p, 1);
    assert_eq!(selection(&mut e), "");
}

#[test]
fn slow_far_or_other_field_presses_start_a_new_click_run() {
    // Too slow: the second press is a single click again.
    let mut e = engine();
    let p = at(&e, "txt", 0.3);
    click(&mut e, p.0, p.1);
    e.test_age_last_click(Duration::from_millis(501));
    click(&mut e, p.0, p.1);
    assert_eq!(
        selection(&mut e),
        "",
        "a press 501 ms later is a new single click"
    );
    // Just in time still counts.
    let mut e = engine();
    click(&mut e, p.0, p.1);
    e.test_age_last_click(Duration::from_millis(450));
    click(&mut e, p.0, p.1);
    assert!(
        !selection(&mut e).is_empty(),
        "a press 450 ms later is a double-click"
    );
    // Moved more than 2 px.
    let mut e = engine();
    click(&mut e, p.0, p.1);
    click(&mut e, p.0 + 3.0, p.1);
    assert_eq!(selection(&mut e), "", "3 px away is a new click");
    let mut e = engine();
    click(&mut e, p.0, p.1);
    click(&mut e, p.0, p.1 + 3.0);
    assert_eq!(selection(&mut e), "", "3 px away vertically is a new click");
    // Within 2 px still counts.
    let mut e = engine();
    click(&mut e, p.0, p.1);
    click(&mut e, p.0 + 2.0, p.1 - 2.0);
    assert!(
        !selection(&mut e).is_empty(),
        "2 px away is the same click run"
    );
    // A press elsewhere breaks the run.
    let mut e = engine();
    let o = at(&e, "other", 0.5);
    click(&mut e, p.0, p.1);
    click(&mut e, o.0, o.1);
    click(&mut e, p.0, p.1);
    assert_eq!(
        selection(&mut e),
        "",
        "a click in another field resets the count"
    );
    // Within 2 px but in the next field: a new click there.
    let mut e = engine();
    let l = e.test_element_box("l").unwrap();
    let r = e.test_element_box("r").unwrap();
    assert_eq!(l.0 + l.2, r.0, "the two boxes touch");
    let y = l.1 + l.3 / 2.0;
    click(&mut e, r.0 - 1.0, y);
    click(&mut e, r.0 + 1.0, y);
    assert_eq!(
        selection(&mut e),
        "",
        "a press in the next field starts a new run"
    );
    // A secondary press doesn't count, and breaks the run.
    let mut e = engine();
    click(&mut e, p.0, p.1);
    press(&mut e, p.0, p.1, PointerButton::Secondary);
    click(&mut e, p.0, p.1);
    assert_eq!(
        selection(&mut e),
        "",
        "a right-click is not part of a click run"
    );
    // A press outside any field breaks the run.
    let mut e = engine();
    click(&mut e, p.0, p.1);
    click(&mut e, 190.0, 125.0);
    click(&mut e, p.0, p.1);
    assert_eq!(
        selection(&mut e),
        "",
        "a press on the page resets the count"
    );
}

/// The real Mainnet Deals fee markup (Mainnet master 5feee7cd,
/// templates/hx_deals/items_proposal.html): triple-click inside "4.69",
/// type 4.69, Tab. The box must end on 4.69 (it gave "4.46969"), and by
/// Ruling 5 that is the value the game already has, so no `change`.
mod deals_fee {
    use super::*;

    const CSS: &str = r#"*{box-sizing:border-box}body{margin:0;font-family:sans-serif;font-size:11px}
.row{position:absolute;left:20px;top:40px;display:flex;gap:4px;align-items:center}
.spin{position:relative;display:inline-block;width:50px;height:20px;overflow:hidden;background:#fff}
.spin .n{position:absolute;left:3px;top:3px;width:28px;padding:0;text-align:right}
.spin .ud{position:absolute;right:2px;top:2px;width:14px;height:16px;display:block}
.spin .ud i{display:block;height:8px;line-height:8px;font-size:6px}
.spin input.n{border:0;outline:0;background:transparent;font-weight:700;text-align:right;padding:2px 9px 0 4px;height:20px;min-width:30px;width:auto}"#;

    fn page() -> String {
        format!(
            r##"<!DOCTYPE html><html><head><style>{CSS}</style></head><body><div class="row"><button type="button" id="before" hx-post="deals.proposal.edit_map" hx-trigger="click">Edit on map…</button><span>$</span><span class="spin num" style="width:60px"><input class="n" style="width:46px" type="number" id="deals-fee" name="fee" data-hx-group="on" data-hx-paste-ignore="$ " value="4.69" step="0.01" min="0" max="100.00" hx-post="deals.company.select" hx-trigger="change" hx-sync="drop" hx-indicator="#deals-ind"><span class="ud"><i id="deals-fee-up" data-hx-step="up" data-hx-step-for="deals-fee" aria-label="Up">▲</i><i id="deals-fee-down" data-hx-step="down" data-hx-step-for="deals-fee" aria-label="Down">▼</i></span></span><span>/mo per line used</span><span class="hx-ind" id="deals-ind" hidden></span></div><input type="text" id="after" value=""></body></html>"##
        )
    }

    fn eng() -> Engine {
        let mut e = Engine::new(EngineConfig {
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        e.declare_actions(&["deals.company.select", "deals.proposal.edit_map"]);
        e.load_html(&page()).expect("load");
        let _ = e.paint(vp());
        e
    }

    #[test]
    fn triple_click_then_typing_replaces_the_whole_fee() {
        for frac in [0.35_f32, 0.55, 0.75] {
            let mut e = eng();
            let p = at(&e, "deals-fee", frac);
            clicks(&mut e, p, 3);
            assert_eq!(selection(&mut e), "4.69", "triple-click at {frac}");
            type_str(&mut e, "4.69");
            let outs = key(&mut e, "Tab");
            assert_eq!(
                e.test_editor_display("deals-fee").as_deref(),
                Some("4.69"),
                "triple-click at {frac} + type 4.69 + Tab must give 4.69"
            );
            assert_eq!(
                fee_changes(&outs),
                Vec::<String>::new(),
                "4.69 is the sent value"
            );
        }
    }

    /// Same input with a new fee: the whole field is replaced and committed.
    #[test]
    fn triple_click_then_typing_a_new_fee_commits_it() {
        let mut e = eng();
        let p = at(&e, "deals-fee", 0.55);
        clicks(&mut e, p, 3);
        type_str(&mut e, "5.25");
        let outs = key(&mut e, "Tab");
        assert_eq!(e.test_editor_display("deals-fee").as_deref(), Some("5.25"));
        assert_eq!(fee_changes(&outs), vec!["5.25".to_string()]);
    }

    #[test]
    fn double_click_then_typing_replaces_the_whole_fee() {
        let mut e = eng();
        let p = at(&e, "deals-fee", 0.75);
        clicks(&mut e, p, 2);
        assert_eq!(selection(&mut e), "4.69");
        type_str(&mut e, "5.25");
        let outs = key(&mut e, "Tab");
        assert_eq!(fee_changes(&outs), vec!["5.25".to_string()]);
    }
}

/// The selection highlight a multi-click paints is the same in a partial
/// repaint as in a full one.
#[test]
fn multi_click_selection_partial_paint_equals_full() {
    for (id, n) in [("fee", 2), ("fee", 3), ("txt", 2), ("txt", 3)] {
        let mut e = engine();
        let p = at(&e, id, 0.4);
        for _ in 0..n {
            let mods = Modifiers::default();
            e.input(InputEvent::PointerDown {
                x: p.0,
                y: p.1,
                button: PointerButton::Primary,
                mods,
            });
            e.input(InputEvent::PointerUp {
                x: p.0,
                y: p.1,
                button: PointerButton::Primary,
                mods,
            });
        }
        let partial = e.paint(vp()).unwrap();
        assert!(
            !e.last_paint_stats().full_repaint,
            "{id} x{n}: partial paint"
        );
        let partial_px = partial.rgba.unwrap().pixels;
        e.mark_theme_or_stylesheet_changed();
        let full = e.paint(vp()).unwrap();
        assert_eq!(
            partial_px,
            full.rgba.unwrap().pixels,
            "{id} x{n}: partial vs full"
        );
    }
}
