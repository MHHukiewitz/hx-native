//! Issue #51: a typed comma follows the paste rule, and `data-hx-paste-ignore`
//! strips only around the number, never between two digits.
//!
//! - Commas can be typed freely; on commit (Enter, blur, a `from:body`
//!   shortcut) a comma must sit between whole-part thousands groups, or the
//!   box keeps its value at focus, the same as a refused paste.
//! - With `"$ "`, ` $1,234 ` and `$ 1,234` paste as 1234; `3 5` is refused.

use std::collections::HashMap;

use hx_native::*;

fn boot(html: &str, actions: &[&str]) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(actions);
    eng.load_html(&format!("<!DOCTYPE html><html><body>{html}</body></html>"))
        .unwrap();
    let _ = eng.paint(Viewport {
        width: 400,
        height: 200,
        scale: 1.0,
        holes: vec![],
    });
    eng
}

fn key(e: &mut Engine, k: &str, ctrl: bool) -> InputOutcome {
    e.input(InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            ctrl,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }))
}

fn select_all(e: &mut Engine) {
    let _ = key(e, "a", true);
}

fn typ(e: &mut Engine, s: &str) {
    // One Text event per key, as a keyboard delivers them.
    for c in s.chars() {
        let _ = e.input(InputEvent::Text(c.to_string()));
    }
}

fn paste(e: &mut Engine, s: &str) {
    let _ = e.input(InputEvent::Paste(s.into()));
}

fn disp(e: &mut Engine, id: &str) -> String {
    e.test_editor_display(id).unwrap_or_default()
}

fn val(e: &Engine, id: &str) -> String {
    e.attr(id, "value").unwrap_or_default()
}

fn num(id: &str, value: &str, min: &str, max: &str, step: &str, extra: &str) -> String {
    format!(
        r#"<input id="{id}" type="number" name="{id}" value="{value}" min="{min}" max="{max}" step="{step}" data-hx-group="off" {extra} />"#
    )
}

fn fee(extra: &str) -> String {
    num("f", "6.12", "0", "100", "0.01", extra)
}
fn term() -> String {
    num("t", "12", "1", "120", "1", "")
}
fn up(extra: &str) -> String {
    num("u", "150000", "0", "1000000000", "1000", extra)
}
fn signed() -> String {
    num(
        "s",
        "7",
        "-100",
        "100",
        "0.01",
        r#"data-hx-paste-ignore="$ ""#,
    )
}

/// Select all, type `s` key by key, Enter. Returns the committed value.
fn type_commit(html: &str, id: &str, s: &str) -> String {
    let mut e = boot(html, &[]);
    e.test_set_focus(id);
    select_all(&mut e);
    typ(&mut e, s);
    let _ = key(&mut e, "Enter", false);
    val(&e, id)
}

/// Select all, paste `s`, Enter. Returns the committed value.
fn paste_commit(html: &str, id: &str, s: &str) -> String {
    let mut e = boot(html, &[]);
    e.test_set_focus(id);
    select_all(&mut e);
    paste(&mut e, s);
    let _ = key(&mut e, "Enter", false);
    val(&e, id)
}

const FEE_BAD: [&str; 12] = [
    "3,5", "1,5", "12,34", "99,", ",5", "1,,234", "1,234,", "3,50", "0,5", "7,3480", "1,234.5,",
    "1.2,3",
];

#[test]
fn typed_versions_of_graph_r3_comma_shapes() {
    // Graph's #41 round-3 probe C1, typed instead of pasted.
    for ign in [
        "",
        r#"data-hx-paste-ignore="$ ""#,
        r#"data-hx-paste-ignore="$ ,""#,
    ] {
        for bad in FEE_BAD {
            assert_eq!(
                type_commit(&fee(ign), "f", bad),
                "6.12",
                "fee {ign} typed {bad:?}"
            );
        }
        for bad in ["1,5", "1,2", "12,0"] {
            assert_eq!(type_commit(&term(), "t", bad), "12", "term typed {bad:?}");
        }
        assert_eq!(type_commit(&up(ign), "u", "1,234"), "1234");
        assert_eq!(type_commit(&up(ign), "u", "151,379,336"), "151379336");
        assert_eq!(
            type_commit(&up(ign), "u", "1,51,234"),
            "150000",
            "indian grouping"
        );
        assert_eq!(type_commit(&up(ign), "u", "7,3480"), "150000");
    }
    assert_eq!(
        type_commit(&fee(""), "f", "1,234.5"),
        "100",
        "valid, then clamped"
    );
    assert_eq!(type_commit(&fee(""), "f", "12.5"), "12.5");
}

#[test]
fn a_typed_comma_stays_in_the_box_while_editing() {
    let mut e = boot(&fee(""), &[]);
    e.test_set_focus("f");
    select_all(&mut e);
    typ(&mut e, "3,5");
    assert_eq!(disp(&mut e, "f"), "3,5", "editing stays free");
    assert_eq!(val(&e, "f"), "6.12", "the value never becomes 35");
    let _ = key(&mut e, "Enter", false);
    assert_eq!(val(&e, "f"), "6.12");
    assert_eq!(
        disp(&mut e, "f"),
        "6.12",
        "Enter keeps focus and shows the kept value"
    );
    // Fixing the text before commit works: 1,234 -> 1234 in the upfront box.
    let mut e = boot(&up(""), &[]);
    e.test_set_focus("u");
    select_all(&mut e);
    typ(&mut e, "1,23");
    assert_eq!(val(&e, "u"), "150000", "1,23 is not a number yet");
    typ(&mut e, "4");
    assert_eq!(val(&e, "u"), "1234");
    let _ = key(&mut e, "Tab", false);
    assert_eq!(val(&e, "u"), "1234");
}

#[test]
fn typing_and_pasting_give_the_same_result_for_every_shape() {
    let boxes = [
        ("f", fee("")),
        ("f", fee(r#"data-hx-paste-ignore="$ ""#)),
        ("t", term()),
        ("u", up("")),
        ("s", signed()),
    ];
    let shapes = [
        "3,5",
        "1,5",
        "12,34",
        "99,",
        ",5",
        "1,,234",
        "1,234,",
        "3,50",
        "0,5",
        "7,3480",
        "1,51,234",
        "1,234",
        "151,379,336",
        "1,234.5",
        "1,234.5,",
        "1.2,3",
        "-1,234",
        "-3,5",
        "12.5",
        "3.7",
        "0.5",
        "73,480",
        "1234",
    ];
    for (id, html) in &boxes {
        for shape in shapes {
            // A sign in a box whose min forbids negatives is a separate,
            // older difference (a typed '-' is dropped per key, a pasted one
            // refuses the paste), not part of the comma rule.
            if shape.starts_with('-') && *id != "s" {
                continue;
            }
            let typed = type_commit(html, id, shape);
            let pasted = paste_commit(html, id, shape);
            assert_eq!(typed, pasted, "box {html} shape {shape:?}");
        }
    }
}

#[derive(Clone, Default)]
struct S {
    v: HashMap<String, u64>,
    l: Vec<(String, String, InputLimits)>,
}
impl SliceSource for S {
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
struct F;
impl FragmentSource for F {
    fn render(&mut self, _: &str, _: &[&str]) -> String {
        String::new()
    }
}
fn limits(ver: u64, max: f64) -> S {
    S {
        v: HashMap::from([("actions".into(), ver)]),
        l: vec![(
            "a.save".into(),
            "u".into(),
            InputLimits::new(0.0, max, Some(1000.0)),
        )],
    }
}

const FORM: &str = r#"<form><input id="u" type="number" name="u" value="950000" data-hx-group="off" /><input id="o" type="text" name="o" value="" /><button id="b" hx-post="a.save" hx-trigger="click, keydown[key=='F2'] from:body">S</button></form>"#;

fn arg_u(o: &InputOutcome) -> Option<String> {
    match o {
        InputOutcome::Action { args, .. } => {
            args.iter().find(|(n, _)| n == "u").map(|(_, v)| v.clone())
        }
        _ => None,
    }
}

fn form_typed(text: &str) -> Engine {
    let mut e = boot(FORM, &["a.save"]);
    e.sync(&limits(1, 1_000_000.0), &mut F).unwrap();
    e.test_set_focus("u");
    select_all(&mut e);
    typ(&mut e, text);
    e
}

#[test]
fn enter_blur_esc_and_body_shortcut_all_apply_the_comma_check() {
    // Enter
    let mut e = form_typed("3,5");
    let out = key(&mut e, "Enter", false);
    assert_eq!(val(&e, "u"), "950000", "Enter");
    assert!(
        !matches!(out, InputOutcome::Action { .. }),
        "a kept value fires no change: {out:?}"
    );
    // Blur by Tab
    let mut e = form_typed("7,3480");
    let _ = key(&mut e, "Tab", false);
    assert_eq!(val(&e, "u"), "950000", "Tab blur");
    assert_eq!(disp(&mut e, "u"), "950000");
    // Blur by click elsewhere
    let mut e = form_typed("1,,234");
    e.test_set_focus("o");
    assert_eq!(val(&e, "u"), "950000", "focus moved");
    // Esc
    let mut e = form_typed("3,5");
    let _ = key(&mut e, "Escape", false);
    assert_eq!(val(&e, "u"), "950000", "Esc");
    assert_eq!(
        disp(&mut e, "u"),
        "950000",
        "Esc also clears the typed text"
    );
    // from:body posts the value the commit would keep; the box keeps its text.
    let mut e = form_typed("3,5");
    let out = key(&mut e, "F2", false);
    assert_eq!(arg_u(&out).as_deref(), Some("950000"), "from:body: {out:?}");
    assert_eq!(disp(&mut e, "u"), "3,5", "box keeps its text");
    let mut e = form_typed("1,234");
    let out = key(&mut e, "F2", false);
    assert_eq!(arg_u(&out).as_deref(), Some("1234"), "valid grouping posts");
    // The kept value is held in limits refreshed while focused, like Esc.
    let mut e = form_typed("3,5");
    e.sync(&limits(2, 900_000.0), &mut F).unwrap();
    let out = key(&mut e, "F2", false);
    assert_eq!(
        arg_u(&out).as_deref(),
        Some("900000"),
        "from:body after refresh"
    );
    let _ = key(&mut e, "Enter", false);
    assert_eq!(val(&e, "u"), "900000", "Enter after refresh");
}

#[test]
fn paste_ignore_strips_only_around_the_number() {
    let ign = r#"data-hx-paste-ignore="$ ""#;
    assert_eq!(paste_commit(&up(ign), "u", " $1,234 "), "1234");
    assert_eq!(paste_commit(&up(ign), "u", "$ 1,234"), "1234");
    assert_eq!(paste_commit(&up(ign), "u", "$1,234 $ "), "1234", "trailing");
    for inner in ["1 234", "1, 234", "1 ,234", "$1 234", "15 0000"] {
        assert_eq!(paste_commit(&up(ign), "u", inner), "150000", "{inner:?}");
    }
    assert_eq!(paste_commit(&fee(ign), "f", "3 5"), "6.12", "'3 5' refused");
    assert_eq!(
        paste_commit(&fee(ign), "f", "12 .5"),
        "6.12",
        "'12 .5' refused"
    );
    assert_eq!(
        paste_commit(&fee(ign), "f", "12. 5"),
        "6.12",
        "'12. 5' refused"
    );
    assert_eq!(paste_commit(&fee(ign), "f", " 12.5 "), "12.5");
    assert_eq!(paste_commit(&fee(ign), "f", "$ .5"), "0.5");
    // Between a leading sign and the first digit.
    assert_eq!(paste_commit(&signed(), "s", "-$5"), "-5");
    assert_eq!(paste_commit(&signed(), "s", "- 5"), "-5");
    assert_eq!(paste_commit(&signed(), "s", " -$ 1.25 "), "-1.25");
    assert_eq!(paste_commit(&signed(), "s", "-5 0"), "7", "'-5 0' refused");
    // Without the attribute nothing is stripped at all.
    assert_eq!(paste_commit(&fee(""), "f", "3 5"), "6.12");
    assert_eq!(paste_commit(&fee(""), "f", " 12.5"), "6.12");
}
