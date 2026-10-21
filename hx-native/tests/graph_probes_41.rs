//! Graph's review probes for hx-native PR #41, taken into the suite in
//! round 2. Every former observation line is now an assertion of the
//! contract (paste all-or-nothing, whole-number rounding on commit, browser
//! arrows, limits refresh leaves the focused box alone; see #38, #39).
use hx_native::*;
use std::collections::HashMap;

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
fn k(eng: &mut Engine, key: &str, ctrl: bool, shift: bool) -> InputOutcome {
    eng.input(InputEvent::Key(KeyEvent {
        key: key.into(),
        code: key.into(),
        mods: Modifiers {
            ctrl,
            shift,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }))
}
fn all(e: &mut Engine) {
    let _ = e.input(InputEvent::Key(KeyEvent {
        key: "a".into(),
        code: "KeyA".into(),
        mods: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    }));
}
fn paste(e: &mut Engine, s: &str) {
    let _ = e.input(InputEvent::Paste(s.into()));
}
fn typ(e: &mut Engine, s: &str) {
    let _ = e.input(InputEvent::Text(s.into()));
}
fn enter(e: &mut Engine) {
    let _ = k(e, "Enter", false, false);
}
fn disp(e: &mut Engine, id: &str) -> String {
    e.test_editor_display(id).unwrap_or_default()
}
fn val(e: &Engine, id: &str) -> String {
    e.attr(id, "value").unwrap_or_default()
}

const FEE: &str = r#"<input id="f" type="number" name="f" value="6.12" min="0" max="100" step="0.01" data-hx-group="off" />"#;
const NEG: &str = r#"<input id="f" type="number" name="f" value="6.12" min="-100" max="100" step="0.01" data-hx-group="off" />"#;
const TERM: &str = r#"<input id="t" type="number" name="t" value="12" min="1" max="60" step="1" data-hx-group="off" />"#;
const UP: &str = r#"<input id="u" type="number" name="u" value="150000" min="0" max="1000000" step="1000" data-hx-group="off" />"#;

#[test]
fn p02_paste_over_head_selection_with_point() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    k(&mut e, "Home", false, false);
    k(&mut e, "ArrowRight", false, true);
    k(&mut e, "ArrowRight", false, true);
    paste(&mut e, "7.5");
    assert_eq!(disp(&mut e, "f"), "7.512");
}
#[test]
fn p03_paste_multi_dot_over_all() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    paste(&mut e, "1.2.3");
    assert_eq!(disp(&mut e, "f"), "6.12");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "6.12");
}
#[test]
fn p04_paste_minus_in_middle_rejected() {
    let mut e = boot(
        r#"<input id="f" type="number" name="f" value="12" min="-100" max="100" step="1" data-hx-group="off" />"#,
        &[],
    );
    e.test_set_focus("f");
    k(&mut e, "Home", false, false);
    k(&mut e, "ArrowRight", false, false);
    paste(&mut e, "-");
    assert_eq!(disp(&mut e, "f"), "12");
}
#[test]
fn p05_paste_negative_over_all() {
    let mut e = boot(NEG, &[]);
    e.test_set_focus("f");
    all(&mut e);
    paste(&mut e, "-5.5");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "-5.5");
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    paste(&mut e, "-5.5");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "6.12", "min 0: -5.5 refused");
}
#[test]
fn p06_paste_non_numeric_keeps_old() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    paste(&mut e, "abc");
    assert_eq!(disp(&mut e, "f"), "6.12");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "6.12");
}
#[test]
fn p07_paste_mixed_text() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    paste(&mut e, "12abc34");
    assert_eq!(disp(&mut e, "f"), "6.12");
    all(&mut e);
    paste(&mut e, "1e5");
    assert_eq!(disp(&mut e, "f"), "6.12");
    all(&mut e);
    paste(&mut e, "$1,234.5");
    assert_eq!(disp(&mut e, "f"), "6.12", "no data-hx-paste-ignore");
    all(&mut e);
    paste(&mut e, " 42 ");
    assert_eq!(disp(&mut e, "f"), "6.12", "no trimming");
}
#[test]
fn p08_selection_at_start_sign() {
    let mut e = boot(NEG, &[]);
    e.test_set_focus("f");
    k(&mut e, "Home", false, false);
    k(&mut e, "ArrowRight", false, true);
    paste(&mut e, "-9");
    assert_eq!(disp(&mut e, "f"), "-9.12");
}
#[test]
fn p09_selection_at_end() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    k(&mut e, "End", false, false);
    k(&mut e, "ArrowLeft", false, true);
    k(&mut e, "ArrowLeft", false, true);
    paste(&mut e, "5.5");
    assert_eq!(disp(&mut e, "f"), "6.12", "6.5.5 refused");
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    k(&mut e, "End", false, false);
    for _ in 0..4 {
        k(&mut e, "ArrowLeft", false, true);
    }
    paste(&mut e, "9.99");
    assert_eq!(disp(&mut e, "f"), "9.99");
}
#[test]
fn p10_esc_restores_after_paste() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    paste(&mut e, "3.45");
    assert_eq!(
        val(&e, "f"),
        "3.45",
        "live value tracks paste before commit"
    );
    let _ = k(&mut e, "Escape", false, false);
    assert_eq!(val(&e, "f"), "6.12");
    assert_eq!(disp(&mut e, "f"), "6.12");
}
#[test]
fn p11_bounds() {
    let h = r#"<input id="f" type="number" name="f" value="50" min="10" max="100" step="5" data-hx-group="off" />"#;
    for (typed, want) in [
        ("10", "10"),
        ("100", "100"),
        ("100.5", "100"),
        ("101", "100"),
        ("5", "10"),
        ("13", "13"),
        ("99", "99"),
        ("12.5", "13"),
    ] {
        let mut e = boot(h, &[]);
        e.test_set_focus("f");
        all(&mut e);
        typ(&mut e, typed);
        enter(&mut e);
        assert_eq!(val(&e, "f"), want, "typed {typed}");
    }
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    typ(&mut e, "-1");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "1", "min 0: the '-' key is dropped");
}
#[test]
fn p12_float() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    typ(&mut e, "0.30000000000000004");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "0.3");
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    typ(&mut e, "6.125");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "6.125", "extra decimals not snapped to 0.01");
    let mut e = boot(
        r#"<input id="f" type="number" name="f" value="0.1" min="0" max="1" step="0.1" data-hx-group="off" />"#,
        &[],
    );
    e.test_set_focus("f");
    k(&mut e, "ArrowUp", false, false);
    k(&mut e, "ArrowUp", false, false);
    assert_eq!(val(&e, "f"), "0.3");
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    typ(&mut e, "0.1");
    k(&mut e, "ArrowUp", false, false);
    assert_eq!(val(&e, "f"), "0.11");
}
#[test]
fn p13_integer_box() {
    let mut e = boot(TERM, &[]);
    e.test_set_focus("t");
    all(&mut e);
    typ(&mut e, "3.7");
    assert_eq!(disp(&mut e, "t"), "3.7");
    enter(&mut e);
    assert_eq!(val(&e, "t"), "4");
    let mut e = boot(TERM, &[]);
    e.test_set_focus("t");
    all(&mut e);
    paste(&mut e, "3.7");
    enter(&mut e);
    assert_eq!(val(&e, "t"), "4");
    let mut e = boot(UP, &[]);
    e.test_set_focus("u");
    all(&mut e);
    paste(&mut e, "151234.50");
    enter(&mut e);
    assert_eq!(val(&e, "u"), "151235");
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
#[derive(Default)]
struct F;
impl FragmentSource for F {
    fn render(&mut self, _: &str, _: &[&str]) -> String {
        String::new()
    }
}
fn lim(ver: u64, input: &str, l: InputLimits) -> S {
    S {
        v: HashMap::from([("actions".into(), ver)]),
        l: vec![("a.save".into(), input.into(), l)],
    }
}
#[test]
fn p14_refresh_while_focused_keeps_typed() {
    let h = r#"<form><input id="u" type="number" name="u" value="150000" data-hx-group="off" /><button id="b" hx-post="a.save" hx-trigger="click">S</button></form>"#;
    let mut e = boot(h, &["a.save"]);
    e.sync(
        &lim(1, "u", InputLimits::new(0.0, 1_000_000.0, Some(1000.0))),
        &mut F,
    )
    .unwrap();
    e.test_set_focus("u");
    all(&mut e);
    typ(&mut e, "151234");
    e.sync(
        &lim(2, "u", InputLimits::new(0.0, 900_000.0, Some(1000.0))),
        &mut F,
    )
    .unwrap();
    assert_eq!(val(&e, "u"), "151234");
    assert_eq!(disp(&mut e, "u"), "151234");
    enter(&mut e);
    assert_eq!(val(&e, "u"), "151234");
    // out-of-range transient while focused
    all(&mut e);
    typ(&mut e, "950000");
    e.sync(
        &lim(3, "u", InputLimits::new(0.0, 900_001.0, Some(1000.0))),
        &mut F,
    )
    .unwrap();
    assert_eq!(val(&e, "u"), "950000", "focused box not rewritten");
    assert_eq!(disp(&mut e, "u"), "950000", "focused box not rewritten");
    enter(&mut e);
    assert_eq!(val(&e, "u"), "900001", "new max applies on commit");
}
#[test]
fn p15_range_still_snaps() {
    let h = r#"<form><input id="r" type="range" name="r" value="33" /><button id="b" hx-post="a.save" hx-trigger="click">S</button></form>"#;
    let mut e = boot(h, &["a.save"]);
    e.sync(
        &lim(1, "r", InputLimits::new(0.0, 100.0, Some(10.0))),
        &mut F,
    )
    .unwrap();
    assert_eq!(val(&e, "r"), "30");
    // off-grid number via same refresh does not snap
    let h = r#"<form><input id="n" type="number" name="n" value="33" data-hx-group="off" /><button id="b" hx-post="a.save" hx-trigger="click">S</button></form>"#;
    let mut e = boot(h, &["a.save"]);
    e.sync(
        &lim(1, "n", InputLimits::new(0.0, 100.0, Some(10.0))),
        &mut F,
    )
    .unwrap();
    assert_eq!(val(&e, "n"), "33");
}
#[test]
fn p16_arrows_from_off_step() {
    for (start, key, want) in [
        ("151234", "ArrowUp", "152000"),
        ("151234", "ArrowDown", "151000"),
        ("151634", "ArrowUp", "152000"),
        ("151634", "ArrowDown", "151000"),
        ("151500", "ArrowUp", "152000"),
    ] {
        let mut e = boot(UP, &[]);
        e.test_set_focus("u");
        all(&mut e);
        typ(&mut e, start);
        k(&mut e, key, false, false);
        assert_eq!(val(&e, "u"), want, "{start} {key}");
    }
    // typed but uncommitted value is what arrows step from
    let mut e = boot(UP, &[]);
    e.test_set_focus("u");
    all(&mut e);
    typ(&mut e, "200000");
    k(&mut e, "ArrowUp", false, false);
    assert_eq!(val(&e, "u"), "201000");
}
#[test]
fn p18_grouped_display_after_off_grid_commit() {
    let mut e = boot(
        r#"<input id="u" type="number" name="u" value="150000" min="0" max="1000000" step="1000" />"#,
        &[],
    );
    e.test_set_focus("u");
    all(&mut e);
    typ(&mut e, "151234");
    enter(&mut e);
    assert_eq!(val(&e, "u"), "151234");
    // The unfocused display depends on the grouping default (#40 / PR #42),
    // so it is not asserted here.
}
#[test]
fn p19_paste_comma_decimal() {
    let mut e = boot(FEE, &[]);
    e.test_set_focus("f");
    all(&mut e);
    paste(&mut e, "1,234.5");
    enter(&mut e);
    assert_eq!(val(&e, "f"), "100", "1,234.5 parses, then clamps to max");
}
