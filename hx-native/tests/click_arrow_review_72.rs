//! Graph review of #72 (issue #62): spin-arrow clicks on Mainnet's #787
//! number-box layout (arrows inside `.spin`, `padding-right: 9px`, upfront
//! 124/110, arrows over the input's last 5 px). Hit-testing matches paint at
//! scale 1 and 2, decimals and `step="any"`, a click on another box, Enter
//! then click, disabled and greyed-mid-edit boxes, a sync or a
//! `RequiresChange` form between typing and the click, Tab then click, a press
//! released on the other arrow, partial == full at scale 1 and 2, and a click
//! inside the box being edited keeping the edit (Enter still fires `change`).
use hx_native::*;
use std::collections::HashMap;

const CSS: &str = "html,body{margin:0;background:#c0c0c0}\
.w{position:absolute;left:10px}\
.spin{position:relative;display:inline-block;width:50px;height:20px;overflow:hidden;background:#fff;box-shadow:inset 1px 1px #808080,inset -1px -1px #fff,inset 2px 2px #0a0a0a}\
.spin .n{position:absolute;left:3px;top:3px;width:28px;padding:0;text-align:right;min-width:30px;font-weight:700}\
.spin .ud{position:absolute;right:2px;top:2px;width:14px;height:16px;display:block}\
.spin .ud i{display:block;height:8px;line-height:8px;font-size:6px;background:#c0c0c0;box-shadow:inset -1px -1px #0a0a0a,inset 1px 1px #fff;font-style:normal;text-align:center;overflow:hidden}\
.spin input.n{border:0;outline:0;background:transparent;font-family:\"DejaVu Sans\";font-size:11px;font-weight:700;text-align:right;padding:2px 9px 0 4px;height:20px;box-sizing:border-box;appearance:textfield;min-width:30px;width:auto;color:#000}\
.spin.dis{background:#c0c0c0}";

fn spin(id: &str, sw: u32, iw: u32, attrs: &str, value: &str, dis: bool) -> String {
    let (cls, extra) = if dis {
        (" dis", " disabled aria-disabled=\"true\"".to_string())
    } else {
        ("", format!(" hx-post=\"deal.{id}\" hx-trigger=\"change\""))
    };
    format!(
        "<span class=\"spin num{cls}\" style=\"width:{sw}px\"><input class=\"n\" style=\"width:{iw}px\" type=\"number\" id=\"{id}\" name=\"{id}\" value=\"{value}\" {attrs}{extra}><span class=\"ud\"><i id=\"{id}-up\" data-hx-step=\"up\" data-hx-step-for=\"{id}\">▲</i><i id=\"{id}-down\" data-hx-step=\"down\" data-hx-step-for=\"{id}\">▼</i></span></span>"
    )
}

const FEE: &str = r#"min="0" max="100.00" step="0.01" data-hx-group="on""#;
const UPF: &str = r#"min="0" max="1000000" step="1000" data-hx-group="on""#;
const TERM: &str = r#"min="1" max="36" step="1""#;
const ANY: &str = r#"min="0" max="10" step="any""#;

fn body(fee_dis: bool) -> String {
    format!(
        "<div id=\"other\" hx-trigger=\"slice:o\" hx-target=\"#other\" hx-swap=\"innerHTML\">x</div>\
         <form id=\"f\">\
         <div class=\"w\" style=\"top:10px\" id=\"feew\" hx-trigger=\"slice:fee\" hx-target=\"#feew\" hx-swap=\"innerHTML\">{}</div>\
         <div class=\"w\" style=\"top:40px\">{}</div>\
         <div class=\"w\" style=\"top:70px\">{}</div>\
         <div class=\"w\" style=\"top:100px\">{}</div>\
         <button id=\"go\" hx-post=\"deal.go\" style=\"position:absolute;left:10px;top:130px\">Go</button>\
         </form>",
        spin("fee", 60, 46, FEE, "6.12", fee_dis),
        spin("upf", 124, 110, UPF, "73480", false),
        spin("term", 60, 46, TERM, "12", false),
        spin("any", 60, 46, ANY, "2", false),
    )
}

fn page() -> String {
    format!(
        "<!DOCTYPE html><html><head><style>{CSS}</style></head><body>{}</body></html>",
        body(false)
    )
}

fn vp(s: f64) -> Viewport {
    Viewport::with_scale((200.0 * s) as u32, (160.0 * s) as u32, s)
}

struct Src {
    v: HashMap<String, u64>,
    st: Vec<(String, ActionState)>,
}
impl SliceSource for Src {
    fn version(&self, n: &str) -> Option<u64> {
        self.v.get(n).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.st.clone()
    }
}
struct Frags {
    fee_dis: bool,
}
impl FragmentSource for Frags {
    fn render(&mut self, sec: &str, _s: &[&str]) -> String {
        if sec == "feew" {
            spin("fee", 60, 46, FEE, "6.12", self.fee_dis)
        } else {
            "y".into()
        }
    }
}

fn boot_st(scale: f64, st: Vec<(String, ActionState)>) -> (Engine, Src) {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee", "deal.upf", "deal.term", "deal.any", "deal.go"]);
    e.load_html(&page()).unwrap();
    let src = Src {
        v: HashMap::from([("o".to_string(), 1), ("fee".to_string(), 1)]),
        st,
    };
    e.sync(&src, &mut Frags { fee_dis: false }).unwrap();
    let _ = e.paint(vp(scale)).unwrap();
    (e, src)
}
fn boot(scale: f64) -> Engine {
    boot_st(scale, vec![]).0
}

/// (input name, value) of every Change outcome.
fn feed(e: &mut Engine, ev: InputEvent, ch: &mut Vec<(String, String)>) {
    let d = e.input(ev);
    for o in std::iter::once(d).chain(e.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            trigger: TriggerInfo::Change,
            name,
            args,
            ..
        } = o
        {
            let name = name.trim_start_matches("deal.").to_string();
            let v = args
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            ch.push((name, v));
            e.complete(request, Ok(()));
        }
    }
}
fn key(k: &str, ctrl: bool, shift: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            ctrl,
            shift,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    })
}
fn click(e: &mut Engine, x: f32, y: f32, ch: &mut Vec<(String, String)>) {
    let m = Modifiers::default();
    feed(e, InputEvent::PointerMove { x, y, mods: m }, ch);
    feed(
        e,
        InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        },
        ch,
    );
    feed(
        e,
        InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        },
        ch,
    );
}
fn bx(e: &Engine, id: &str) -> (f32, f32, f32, f32) {
    e.test_element_box(id)
        .unwrap_or_else(|| panic!("no box {id}"))
}
/// Real click into the input's left part, select all, type.
fn type_in(e: &mut Engine, id: &str, s: &str, ch: &mut Vec<(String, String)>) {
    let (x, y, _w, h) = bx(e, id);
    click(e, x + 6.0, y + h / 2.0, ch);
    feed(e, key("a", true, false), ch);
    feed(e, InputEvent::Text(s.into()), ch);
}
fn arrow(e: &mut Engine, id: &str, up: bool, ch: &mut Vec<(String, String)>) {
    let (x, y, w, h) = bx(e, &format!("{id}-{}", if up { "up" } else { "down" }));
    click(e, x + w / 2.0, y + h / 2.0, ch);
}
fn val(e: &Engine, id: &str) -> String {
    e.attr(id, "value").unwrap_or_default()
}
fn c(n: &str, v: &str) -> (String, String) {
    (n.into(), v.into())
}
fn diff_full(e: &mut Engine, s: f64) -> usize {
    let out = e.paint(vp(s)).unwrap();
    let full_flag = e.last_paint_stats().full_repaint;
    let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
    e.mark_theme_or_stylesheet_changed();
    let full = e.paint(vp(s)).unwrap().rgba.expect("full");
    let d = partial
        .pixels
        .chunks_exact(4)
        .zip(full.pixels.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count();
    eprintln!("  partial(full_flag={full_flag}) vs full: {d} px");
    d
}

// ---------- 1. behaviour ----------
#[test]
fn g2_decimals_and_step_any() {
    for (id, typed, up, want, shown) in [
        ("fee", "6.1", true, "6.11", "6.11"),
        ("fee", "6.125", false, "6.12", "6.12"),
        ("fee", "6.12", true, "6.13", "6.13"),
        ("fee", "0", false, "0", "0.00"),
        ("fee", "100", true, "100", "100.00"),
        ("any", "3.75", true, "4.75", "4.75"),
        ("any", "0.2", false, "0", "0"),
        ("upf", "151234", true, "152000", "152,000"),
        ("upf", "151234", false, "151000", "151,000"),
        ("upf", "73,480", true, "74000", "74,000"),
        ("term", "40", false, "36", "36"),
        ("term", "0", true, "1", "1"),
    ] {
        let mut e = boot(1.0);
        let sent = val(&e, id);
        let mut ch = vec![];
        type_in(&mut e, id, typed, &mut ch);
        arrow(&mut e, id, up, &mut ch);
        // Issue #81 (Designer ruling 1): the box keeps the focus after the
        // click, so leave it (Tab, no second `change`) to see the committed
        // display this table checks; before #81 the click itself blurred.
        assert_eq!(e.focus_state().focused_id.as_deref(), Some(id));
        feed(&mut e, key("Tab", false, false), &mut ch);
        let disp = e.test_editor_display(id);
        eprintln!(
            "{id} typed {typed} {} -> {} shown {disp:?} ch {ch:?}",
            if up { "▲" } else { "▼" },
            val(&e, id)
        );
        assert_eq!(val(&e, id), want, "{id} {typed}");
        // Ruling 5 (Mainnet b6f2c892, #81): ending on the sent value sends
        // nothing (fee typed `6.125` ▼ to the sent `6.12`); before that
        // ruling every row sent one `change`.
        let sends = if want == sent {
            vec![]
        } else {
            vec![c(id, want)]
        };
        assert_eq!(ch, sends, "{id} {typed}");
        assert_eq!(disp.as_deref(), Some(shown), "{id} {typed} shown");
    }
}

#[test]
fn g3_hit_columns_match_paint_787() {
    for s in [1.0f64, 2.0] {
        for id in ["upf", "fee", "term"] {
            let e = boot(s);
            let (ax, ay, aw, ah) = bx(&e, &format!("{id}-up"));
            if id == "upf" {
                // #787: a 110 px input in a 124 px spin; the 14 px arrows sit
                // over the input's last 5 px.
                let (ix, _, iw, _) = bx(&e, "upf");
                assert_eq!((iw, aw, ix + iw - ax), (110.0, 14.0, 5.0));
            }
            // Painted arrow: the contiguous #c0c0c0 face run through the arrow's
            // centre, bounded by its highlight (left) and dark bevel (right),
            // in device pixels.
            let fr = e.last_frame().unwrap().clone();
            let row = ((ay + ah / 2.0) as f64 * s) as u32;
            let dpx = |x: u32| -> [u8; 3] {
                let i = ((row * fr.width + x) * 4) as usize;
                [fr.pixels[i], fr.pixels[i + 1], fr.pixels[i + 2]]
            };
            // start inside the face, left and right of the glyph
            let mut l = ((ax as f64 + 2.5) * s) as u32;
            let mut r = ((ax as f64 + aw as f64 - 2.5) * s) as u32;
            assert_eq!(dpx(l), [192, 192, 192]);
            assert_eq!(dpx(r), [192, 192, 192]);
            while dpx(l - 1) == [192, 192, 192] {
                l -= 1;
            }
            while dpx(r + 1) == [192, 192, 192] {
                r += 1;
            }
            // The 1px highlight and dark bevel are 1 CSS px = `s` device px
            // wide, as in Chrome (#125; they were 1 device px at every scale).
            let w = s as u32;
            let (left_edge, right_edge) = (l - w, r + w); // highlight, dark bevel
            eprintln!("scale {s} {id}-up box css x {ax}..{} ; painted dev px {left_edge}..={right_edge} hl {:?} dk {:?} -> css {:.2}..{:.2}", ax + aw, dpx(left_edge), dpx(right_edge), left_edge as f64 / s, (right_edge + 1) as f64 / s);
            for k in 1..=w {
                assert_eq!(dpx(l - k), [255, 255, 255], "highlight");
                assert_eq!(dpx(r + k), [10, 10, 10], "dark bevel");
            }
            assert!(
                (left_edge as f64 / s - ax as f64).abs() < 1e-6,
                "painted left = box left"
            );
            assert!(
                ((right_edge + 1) as f64 / s - (ax + aw) as f64).abs() < 1e-6,
                "painted right = box right"
            );
            // Every CSS column inside the arrow box steps, typed or committed.
            let want = match id {
                "fee" => "7.01",
                "upf" => "1000",
                _ => "8",
            };
            let start: String = val(&e, id);
            let mut bad = vec![];
            let mut col = ax + 0.5;
            while col < ax + aw {
                let mut e = boot(s);
                let mut ch = vec![];
                type_in(&mut e, id, "7", &mut ch);
                click(&mut e, col, ay + ah / 2.0, &mut ch);
                let typed_ok = val(&e, id) == want && ch.len() == 1;
                let typed_res = (val(&e, id), ch.clone(), e.focus_state().focused_id.clone());
                let mut e = boot(s);
                let mut ch = vec![];
                click(&mut e, col, ay + ah / 2.0, &mut ch);
                let committed_ok = val(&e, id) != start && ch.len() == 1;
                if !typed_ok || !committed_ok {
                    bad.push(format!(
                        "col {col}: typed {typed_res:?} committed ({}, {ch:?})",
                        val(&e, id)
                    ));
                }
                col += 1.0;
            }
            for b in &bad {
                eprintln!("  MISS scale {s} {id}: {b}");
            }
            assert!(
                bad.is_empty(),
                "scale {s} {id}: {} of {} arrow columns do not step",
                bad.len(),
                aw
            );
            // 1 px left of the arrow is the input (caret), not a step.
            let mut e = boot(s);
            let mut ch = vec![];
            type_in(&mut e, id, "7", &mut ch);
            click(&mut e, ax - 0.5, ay + ah / 2.0, &mut ch);
            assert!(ch.is_empty(), "left of arrow fired {ch:?}");
            assert_eq!(e.focus_state().focused_id.as_deref(), Some(id));
            let d = e.test_editor_display(id);
            eprintln!("  scale {s} {id}: click left of arrow -> display {d:?}");
        }
    }
}

#[test]
fn g4_partial_equals_full_scale_1_and_2() {
    for s in [1.0f64, 2.0] {
        let mut e = boot(s);
        let mut ch = vec![];
        for (id, typed, up) in [
            ("upf", "151234", true),
            ("fee", "6.125", false),
            ("term", "3", true),
            ("any", "3.7", true),
        ] {
            type_in(&mut e, id, typed, &mut ch);
            assert_eq!(diff_full(&mut e, s), 0, "typed {id} s{s}");
            arrow(&mut e, id, up, &mut ch);
            assert_eq!(diff_full(&mut e, s), 0, "click {id} s{s}");
            // committed click
            arrow(&mut e, id, up, &mut ch);
            assert_eq!(diff_full(&mut e, s), 0, "committed {id} s{s}");
        }
        // editing fee, click upfront's arrow
        type_in(&mut e, "fee", "7", &mut ch);
        let _ = e.paint(vp(s)).unwrap();
        arrow(&mut e, "upf", true, &mut ch);
        assert_eq!(diff_full(&mut e, s), 0, "cross s{s}");
        eprintln!("s{s} changes {ch:?}");
    }
}

#[test]
fn g5_click_other_box_while_editing() {
    let mut e = boot(1.0);
    let mut ch = vec![];
    type_in(&mut e, "fee", "7", &mut ch);
    arrow(&mut e, "upf", true, &mut ch);
    eprintln!("{ch:?}");
    assert_eq!(val(&e, "fee"), "7");
    assert_eq!(val(&e, "upf"), "74000");
    assert_eq!(ch, vec![c("fee", "7"), c("upf", "74000")]);
}

#[test]
fn g6_enter_then_click() {
    let mut e = boot(1.0);
    let mut ch = vec![];
    type_in(&mut e, "upf", "151234", &mut ch);
    feed(&mut e, key("Enter", false, false), &mut ch);
    arrow(&mut e, "upf", true, &mut ch);
    eprintln!("{ch:?}");
    assert_eq!(ch, vec![c("upf", "151234"), c("upf", "152000")]);
}

#[test]
fn g7_disabled_box() {
    // markup-disabled
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee", "deal.upf", "deal.term", "deal.any", "deal.go"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head><style>{CSS}</style></head><body>{}</body></html>",
        body(true)
    ))
    .unwrap();
    let _ = e.paint(vp(1.0)).unwrap();
    let mut ch = vec![];
    arrow(&mut e, "fee", true, &mut ch);
    assert_eq!(val(&e, "fee"), "6.12");
    assert!(ch.is_empty());
    // greyed by a swap while being edited
    let (mut e, mut src) = boot_st(1.0, vec![]);
    let mut ch = vec![];
    type_in(&mut e, "fee", "7", &mut ch);
    src.v.insert("fee".into(), 2);
    e.sync(&src, &mut Frags { fee_dis: true }).unwrap();
    let _ = e.paint(vp(1.0)).unwrap();
    arrow(&mut e, "fee", true, &mut ch);
    eprintln!(
        "greyed mid-edit: value {} focus {:?} ch {ch:?}",
        val(&e, "fee"),
        e.focus_state().focused_id
    );
    assert_eq!(val(&e, "fee"), "6.12");
    assert!(ch.is_empty());
    assert_eq!(diff_full(&mut e, 1.0), 0);
}

#[test]
fn g8_unrelated_sync_between_type_and_click() {
    let (mut e, mut src) = boot_st(1.0, vec![]);
    let mut ch = vec![];
    type_in(&mut e, "term", "3.7", &mut ch);
    src.v.insert("o".into(), 2);
    e.sync(&src, &mut Frags { fee_dis: false }).unwrap();
    let _ = e.paint(vp(1.0)).unwrap();
    arrow(&mut e, "term", true, &mut ch);
    eprintln!("after sync: {} {ch:?}", val(&e, "term"));
    assert_eq!(val(&e, "term"), "4");
    assert_eq!(ch, vec![c("term", "4")]);
}

fn rc() -> Vec<(String, ActionState)> {
    vec![(
        "deal.go".into(),
        ActionState::RequiresChange {
            form: "f".into(),
            reason: "edit".into(),
        },
    )]
}

#[test]
fn g9_requires_change_click() {
    let (mut e, _s) = boot_st(1.0, rc());
    let mut ch = vec![];
    type_in(&mut e, "term", "3.7", &mut ch);
    let _ = e.paint(vp(1.0)).unwrap();
    arrow(&mut e, "term", true, &mut ch);
    eprintln!("RC click: {} {ch:?}", val(&e, "term"));
    assert_eq!(val(&e, "term"), "4");
    assert_eq!(ch, vec![c("term", "4")]);
}

#[test]
fn g10_requires_change_enter() {
    let (mut e, _s) = boot_st(1.0, rc());
    let mut ch = vec![];
    type_in(&mut e, "term", "3.7", &mut ch);
    let _ = e.paint(vp(1.0)).unwrap();
    feed(&mut e, key("Enter", false, false), &mut ch);
    eprintln!("RC enter: {} {ch:?}", val(&e, "term"));
    assert_eq!(ch, vec![c("term", "4")]);
}

#[test]
fn g13_tab_select_then_click() {
    let mut e = boot(1.0);
    let mut ch = vec![];
    // focus term via click, then Shift+Tab back? Use Tab from fee into upf.
    type_in(&mut e, "fee", "6.12", &mut ch);
    feed(&mut e, key("Tab", false, false), &mut ch);
    let f = e.focus_state().focused_id.clone();
    let d = e.test_editor_display("upf");
    eprintln!("after Tab focus {f:?} sel display {d:?}");
    arrow(&mut e, "upf", true, &mut ch);
    eprintln!("tab+click: {} {ch:?}", val(&e, "upf"));
    assert_eq!(val(&e, "upf"), "74000");
    assert_eq!(ch, vec![c("upf", "74000")]);
}

#[test]
fn g14_press_up_release_down() {
    let mut e = boot(1.0);
    let mut ch = vec![];
    type_in(&mut e, "term", "3.7", &mut ch);
    let (ux, uy, uw, uh) = bx(&e, "term-up");
    let (dx, dy, dw, dh) = bx(&e, "term-down");
    let m = Modifiers::default();
    feed(
        &mut e,
        InputEvent::PointerDown {
            x: ux + uw / 2.0,
            y: uy + uh / 2.0,
            button: PointerButton::Primary,
            mods: m,
        },
        &mut ch,
    );
    feed(
        &mut e,
        InputEvent::PointerUp {
            x: dx + dw / 2.0,
            y: dy + dh / 2.0,
            button: PointerButton::Primary,
            mods: m,
        },
        &mut ch,
    );
    let disp = e.test_editor_display("term");
    eprintln!(
        "press ▲ release ▼: {} disp {disp:?} focus {:?} ch {ch:?}",
        val(&e, "term"),
        e.focus_state().focused_id
    );
    // Issue #81 (Designer ruling 2): the press steps 3.7 ▲ to 4 and the
    // release, on the other arrow or anywhere, sends that one `change`;
    // the box keeps the focus. Before #81 a release off the pressed arrow
    // sent nothing.
    assert_eq!(val(&e, "term"), "4");
    assert_eq!(ch, vec![c("term", "4")]);
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("term"));
}

#[test]
fn g15_click_inside_edited_box_keeps_the_edit() {
    for (id, typed, up_want) in [
        ("fee", "7", "7.01"),
        ("term", "7", "8"),
        ("upf", "151234", "152000"),
    ] {
        let mut e = boot(1.0);
        let start = val(&e, id);
        let (x, y, _w, h) = bx(&e, id);
        // Enter after a click that moves the caret: one change.
        let mut ch = vec![];
        type_in(&mut e, id, typed, &mut ch);
        click(&mut e, x + 8.0, y + h / 2.0, &mut ch);
        assert_eq!(
            e.test_editor_display(id).as_deref(),
            Some(typed),
            "{id}: the typed text stays as typed"
        );
        feed(&mut e, key("Enter", false, false), &mut ch);
        assert_eq!(
            ch,
            vec![c(id, typed)],
            "{id}: Enter after a click in the box"
        );
        // Esc after such a click still reverts to the focus-time value.
        let mut e = boot(1.0);
        let mut ch = vec![];
        type_in(&mut e, id, typed, &mut ch);
        click(&mut e, x + 8.0, y + h / 2.0, &mut ch);
        feed(&mut e, key("Escape", false, false), &mut ch);
        assert_eq!(val(&e, id), start, "{id}: Esc after a click in the box");
        assert!(ch.is_empty());
        // Blur by clicking elsewhere: one change.
        let mut e = boot(1.0);
        let mut ch = vec![];
        type_in(&mut e, id, typed, &mut ch);
        click(&mut e, x + 8.0, y + h / 2.0, &mut ch);
        click(&mut e, 190.0, 150.0, &mut ch);
        assert_eq!(
            ch,
            vec![c(id, typed)],
            "{id}: blur after a click in the box"
        );
        // ▲ after such a click steps the typed value.
        let mut e = boot(1.0);
        let mut ch = vec![];
        type_in(&mut e, id, typed, &mut ch);
        click(&mut e, x + 8.0, y + h / 2.0, &mut ch);
        arrow(&mut e, id, true, &mut ch);
        assert_eq!(ch, vec![c(id, up_want)], "{id}: ▲ after a click in the box");
    }
}
