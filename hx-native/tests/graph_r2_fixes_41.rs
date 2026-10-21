//! Graph review of #41 round 2 (taken in round 3): a pasted comma must be
//! thousands grouping (listing ',' in data-hx-paste-ignore has no effect),
//! Esc applies limits refreshed while focused, a body shortcut posts the
//! commit value, a bare point commits like a bare sign (see #38, #39).
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
fn val(e: &Engine, id: &str) -> String {
    e.attr(id, "value").unwrap_or_default()
}
fn paste_commit(html: &str, id: &str, s: &str) -> String {
    let mut e = boot(html, &[]);
    e.test_set_focus(id);
    let _ = key(&mut e, "a", true);
    let _ = e.input(InputEvent::Paste(s.into()));
    let _ = key(&mut e, "Enter", false);
    val(&e, id)
}

#[test]
fn a_pasted_comma_must_be_thousands_grouping() {
    let fee = r#"<input id="f" type="number" name="f" value="6.12" min="0" max="100" step="0.01" data-hx-group="off" />"#;
    let fee_opt = r#"<input id="f" type="number" name="f" value="6.12" min="0" max="100" step="0.01" data-hx-group="off" data-hx-paste-ignore="$," />"#;
    for html in [fee, fee_opt] {
        assert_eq!(
            paste_commit(html, "f", "3,5"),
            "6.12",
            "EU decimal must not commit 35"
        );
        assert_eq!(paste_commit(html, "f", "1,2"), "6.12");
        assert_eq!(paste_commit(html, "f", "12,34"), "6.12");
        assert_eq!(paste_commit(html, "f", "1.2,5"), "6.12");
        assert_eq!(paste_commit(html, "f", "99,"), "6.12");
    }
    assert_eq!(
        paste_commit(fee, "f", "1,234.5"),
        "100",
        "grouping parses, then clamps"
    );
    let up = r#"<input id="u" type="number" name="u" value="150000" min="0" max="1000000000" step="1000" data-hx-group="off" />"#;
    assert_eq!(paste_commit(up, "u", "151,234"), "151234");
    assert_eq!(paste_commit(up, "u", "151,379,336"), "151379336");
    assert_eq!(
        paste_commit(up, "u", "1,51,234"),
        "150000",
        "lakh grouping refused"
    );
    let up_opt = r#"<input id="u" type="number" name="u" value="150000" min="0" max="1000000000" step="1000" data-hx-group="off" data-hx-paste-ignore="$," />"#;
    assert_eq!(paste_commit(up_opt, "u", "$1,234"), "1234");
    assert_eq!(paste_commit(up_opt, "u", "$151,379,336"), "151379336");
    assert_eq!(paste_commit(up_opt, "u", "$1,2345"), "150000");
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
fn lim(ver: u64, min: f64, max: f64, step: f64) -> S {
    S {
        v: HashMap::from([("actions".into(), ver)]),
        l: vec![(
            "a.save".into(),
            "u".into(),
            InputLimits::new(min, max, Some(step)),
        )],
    }
}
const FORM: &str = r#"<form><input id="u" type="number" name="u" value="950000" data-hx-group="off" /><button id="b" hx-post="a.save" hx-trigger="click, keydown[key=='F2'] from:body">S</button></form>"#;

#[test]
fn escape_applies_limits_refreshed_while_focused() {
    let mut e = boot(FORM, &["a.save"]);
    e.sync(&lim(1, 0.0, 1_000_000.0, 1000.0), &mut F).unwrap();
    e.test_set_focus("u");
    let _ = e.input(InputEvent::Text("123000".into()));
    e.sync(&lim(2, 0.0, 900_000.0, 1000.0), &mut F).unwrap();
    let _ = key(&mut e, "Escape", false);
    assert_eq!(val(&e, "u"), "900000");
}

fn f2_args(e: &mut Engine) -> Vec<(String, String)> {
    match key(e, "F2", false) {
        InputOutcome::Action { args, .. } => args,
        other => panic!("expected Action, got {other:?}"),
    }
}

#[test]
fn body_shortcut_posts_the_commit_value_of_the_focused_box() {
    // stale after a refresh
    let mut e = boot(FORM, &["a.save"]);
    e.sync(&lim(1, 0.0, 1_000_000.0, 1000.0), &mut F).unwrap();
    e.test_set_focus("u");
    e.sync(&lim(2, 0.0, 900_000.0, 1000.0), &mut F).unwrap();
    assert_eq!(f2_args(&mut e)[0], ("u".into(), "900000".into()));
    assert_eq!(
        e.test_editor_display("u").unwrap(),
        "950000",
        "text kept until commit"
    );
    // typed above max
    let mut e = boot(FORM, &["a.save"]);
    e.sync(&lim(1, 0.0, 1_000_000.0, 1000.0), &mut F).unwrap();
    e.test_set_focus("u");
    let _ = key(&mut e, "a", true);
    let _ = e.input(InputEvent::Text("5000000".into()));
    assert_eq!(f2_args(&mut e)[0], ("u".into(), "1000000".into()));
    // whole-number box rounds
    let mut e = boot(FORM, &["a.save"]);
    e.sync(&lim(1, 1.0, 120.0, 1.0), &mut F).unwrap();
    e.test_set_focus("u");
    let _ = key(&mut e, "a", true);
    let _ = e.input(InputEvent::Text("3.7".into()));
    assert_eq!(f2_args(&mut e)[0], ("u".into(), "4".into()));
    let _ = key(&mut e, "Enter", false);
    assert_eq!(val(&e, "u"), "4");
}

#[test]
fn a_bare_point_commits_like_a_bare_sign_never_as_text() {
    // "." / "-." / "+." commit exactly as a lone "-" does (an empty number
    // box, as before round 2), never the non-number text ".".
    let term = r#"<input id="t" type="number" name="t" value="12" min="-5" max="120" step="1" data-hx-group="off" />"#;
    let commit = |s: &str, paste: bool| {
        let mut e = boot(term, &[]);
        e.test_set_focus("t");
        let _ = key(&mut e, "a", true);
        if paste {
            let _ = e.input(InputEvent::Paste(s.into()));
        } else {
            let _ = e.input(InputEvent::Text(s.into()));
        }
        let _ = key(&mut e, "Enter", false);
        val(&e, "t")
    };
    let lone_sign = commit("-", false);
    for s in [".", "-.", "+."] {
        assert_eq!(commit(s, false), lone_sign, "typed {s:?}");
        assert_eq!(commit(s, true), lone_sign, "pasted {s:?}");
    }
}

#[test]
fn dollar_space_ignore_list_strips_dollar_and_spaces_and_keeps_the_comma_rule() {
    // Mainnet's fee/upfront boxes use data-hx-paste-ignore="$ ".
    let up = r#"<input id="u" type="number" name="u" value="150000" min="0" max="1000000000" step="1000" data-hx-group="off" data-hx-paste-ignore="$ " />"#;
    assert_eq!(paste_commit(up, "u", " $1,234 "), "1234");
    assert_eq!(paste_commit(up, "u", "$ 151,379,336"), "151379336");
    let fee = r#"<input id="f" type="number" name="f" value="6.12" min="0" max="100" step="0.01" data-hx-group="off" data-hx-paste-ignore="$ " />"#;
    assert_eq!(paste_commit(fee, "f", "3,5"), "6.12", "EU decimal refused");
    assert_eq!(paste_commit(fee, "f", " $12.50 "), "12.5");
    // ',' listed has no effect: the grouping rule still refuses '3,5'.
    let fee_comma = r#"<input id="f" type="number" name="f" value="6.12" min="0" max="100" step="0.01" data-hx-group="off" data-hx-paste-ignore="$ ," />"#;
    assert_eq!(
        paste_commit(fee_comma, "f", "3,5"),
        "6.12",
        "',' listed: still refused"
    );
    assert_eq!(
        paste_commit(fee_comma, "f", "$1,234"),
        "100",
        "grouping still parses"
    );
}
