//! Graph review of #83 (issue #82): form-arg cases around an unticked
//! checkbox that `checkbox_unticked_args.rs` does not cover.
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
    let style = "<style>html,body{margin:0} input,button,label{position:absolute;width:16px;height:16px;margin:0;padding:0}</style>";
    eng.load_html(&format!(
        "<!DOCTYPE html><html><head>{style}</head><body>{html}</body></html>"
    ))
    .unwrap();
    let _ = eng.paint(Viewport::with_scale(400, 200, 1.0)).unwrap();
    eng
}

/// (trigger, name, args) for every Action / Blocked the event produced.
fn all(eng: &mut Engine, ev: InputEvent) -> Vec<String> {
    let first = eng.input(ev);
    let mut out = Vec::new();
    for o in std::iter::once(first).chain(eng.take_outcomes()) {
        match o {
            InputOutcome::Action {
                request,
                name,
                args,
                trigger,
                source_id,
                ..
            } => {
                eng.complete(request, Ok(()));
                out.push(format!("A {name} from {source_id} {trigger:?} {args:?}"));
            }
            InputOutcome::Blocked { name, .. } => out.push(format!("B {name}")),
            _ => {}
        }
    }
    out
}

fn click(eng: &mut Engine, x: f32, y: f32) -> Vec<String> {
    let m = Modifiers::default();
    let mut v = all(
        eng,
        InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        },
    );
    v.extend(all(
        eng,
        InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        },
    ));
    let _ = eng.paint(Viewport::with_scale(400, 200, 1.0));
    v
}

fn s(v: &[(&str, &str)]) -> String {
    format!(
        "{:?}",
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect::<Vec<_>>()
    )
}

#[test]
fn p1_repeated_names_order_when_trigger_is_ticked() {
    let mut e = boot(
        r#"<form id="f">
<input id="a" type="checkbox" name="state" value="a" checked hx-post="s" hx-trigger="change" style="left:0;top:0">
<input id="b" type="checkbox" name="state" value="b" hx-post="s" hx-trigger="change" style="left:30px;top:0">
<input id="c" type="checkbox" name="state" value="c" checked hx-post="s" hx-trigger="change" style="left:60px;top:0">
</form>"#,
        &["s"],
    );
    let out = click(&mut e, 38.0, 8.0);
    // Pins today's documented trigger-first order (REQUIREMENTS "Form args");
    // htmx/FormData would give document order a, b, c.
    assert_eq!(out.len(), 1);
    assert!(
        out[0].ends_with(&s(&[("state", "b"), ("state", "a"), ("state", "c")])),
        "{out:?}"
    );
}

#[test]
fn p2_no_value_sends_on_and_disabled_is_left_out() {
    let mut e = boot(
        r#"<form id="f">
<input id="t" type="checkbox" name="t" hx-post="s" hx-trigger="change" style="left:0;top:0">
<input id="o" type="checkbox" name="o" checked style="left:30px;top:0">
<input id="d" type="checkbox" name="d" value="x" checked disabled style="left:60px;top:0">
<input id="u" type="checkbox" name="u" value="y" style="left:90px;top:0">
</form>"#,
        &["s"],
    );
    let tick = click(&mut e, 8.0, 8.0);
    assert!(
        tick[0].ends_with(&s(&[("t", "on"), ("o", "on")])),
        "{tick:?}"
    );
    let untick = click(&mut e, 8.0, 8.0);
    assert!(untick[0].ends_with(&s(&[("o", "on")])), "{untick:?}");
}

#[test]
fn p3_no_form_untick_sends_nothing() {
    let mut e = boot(
        r#"<input id="m" type="checkbox" name="m" hx-post="s" hx-trigger="change" style="left:0;top:0">"#,
        &["s"],
    );
    assert!(click(&mut e, 8.0, 8.0)[0].ends_with(&s(&[("m", "on")])));
    let u = click(&mut e, 8.0, 8.0);
    assert_eq!(u.len(), 1, "change still fires");
    assert!(u[0].ends_with("[]"), "{u:?}");
}

#[test]
fn p4_label_for_untick_leaves_box_out() {
    let mut e = boot(
        r#"<form id="f">
<input id="c" type="checkbox" name="c" value="v" checked hx-post="s" hx-trigger="change" style="left:0;top:0">
<label for="c" id="l" style="left:100px;top:0;width:40px">L</label>
<input id="k" type="checkbox" name="k" checked style="left:30px;top:0">
</form>"#,
        &["s"],
    );
    let u = click(&mut e, 110.0, 8.0);
    assert!(u[0].ends_with(&s(&[("k", "on")])), "{u:?}");
}

#[test]
fn p6_button_in_form_after_untick() {
    let mut e = boot(
        r#"<form id="f">
<input id="c" type="checkbox" name="c" value="v" checked style="left:0;top:0">
<input id="n" type="number" name="n" value="3" style="left:30px;top:0;width:40px">
<button id="go" hx-post="apply" name="b" value="1" style="left:100px;top:0">G</button>
</form>"#,
        &["apply"],
    );
    let _ = click(&mut e, 8.0, 8.0);
    let g = click(&mut e, 108.0, 8.0);
    assert!(
        g.last().unwrap().ends_with(&s(&[("b", "1"), ("n", "3")])),
        "{g:?}"
    );
}

#[derive(Default)]
struct Sl {
    v: HashMap<String, u64>,
    a: Vec<(String, ActionState)>,
    r: Vec<(String, String, InputRule)>,
}
impl SliceSource for Sl {
    fn version(&self, n: &str) -> Option<u64> {
        self.v.get(n).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.a.clone()
    }
    fn input_rules(&self) -> Vec<(String, String, InputRule)> {
        self.r.clone()
    }
}
struct Fr;
impl FragmentSource for Fr {
    fn render(&mut self, id: &str, _s: &[&str]) -> String {
        format!("<!-- {id} -->")
    }
}

#[test]
fn p7_requires_change_with_checkbox_untick() {
    let mut e = boot(
        r#"<form id="f">
<input id="c" type="checkbox" name="c" value="v" checked hx-post="edit" hx-trigger="change" style="left:0;top:0">
<input id="n" type="number" name="n" value="3" style="left:30px;top:0;width:40px">
<button id="go" hx-post="apply" style="left:100px;top:0">G</button>
</form>"#,
        &["apply", "edit"],
    );
    let mut sl = Sl::default();
    sl.v.insert("actions".into(), 1);
    sl.a = vec![(
        "apply".into(),
        ActionState::RequiresChange {
            form: "f".into(),
            reason: "No change".into(),
        },
    )];
    e.sync(&sl, &mut Fr).unwrap();
    let _ = e.paint(Viewport::with_scale(400, 200, 1.0));
    assert_eq!(
        click(&mut e, 108.0, 8.0),
        vec!["B apply".to_string()],
        "clean form greys apply"
    );
    let u = click(&mut e, 8.0, 8.0);
    assert!(u[0].ends_with(&s(&[("n", "3")])), "{u:?}");
    let g = click(&mut e, 108.0, 8.0);
    assert!(
        g.last().unwrap().starts_with("A apply") && g.last().unwrap().ends_with(&s(&[("n", "3")])),
        "{g:?}"
    );
    let t = click(&mut e, 8.0, 8.0);
    assert!(t[0].ends_with(&s(&[("c", "v"), ("n", "3")])), "{t:?}");
    assert_eq!(
        click(&mut e, 108.0, 8.0),
        vec!["B apply".to_string()],
        "back at baseline greys again"
    );
}

#[test]
fn p8_typed_number_then_untick() {
    let mut e = boot(
        r#"<form id="f">
<input id="c" type="checkbox" name="c" value="v" checked hx-post="s" hx-trigger="change" style="left:0;top:0">
<input id="n" type="number" name="n" value="3" hx-post="s" hx-trigger="change" style="left:30px;top:0;width:40px">
</form>"#,
        &["s"],
    );
    let _ = click(&mut e, 50.0, 8.0);
    let _ = all(&mut e, InputEvent::Text("7".into()));
    let u = click(&mut e, 8.0, 8.0);
    let last = u.last().unwrap();
    assert!(last.contains("from c") && !last.contains("\"c\""), "{u:?}");
}

#[test]
fn p9_enabled_when_gate_untick() {
    let mut e = boot(
        r#"<form id="f">
<input id="g" type="checkbox" name="g" checked hx-post="s" hx-trigger="change" style="left:0;top:0">
<input id="dep" type="checkbox" name="dep" value="d" checked style="left:30px;top:0">
</form>"#,
        &["s"],
    );
    let mut sl = Sl::default();
    sl.v.insert("actions".into(), 1);
    sl.r = vec![(
        "s".into(),
        "dep".into(),
        InputRule::EnabledWhen {
            input: "g".into(),
            checked: true,
            reason: "gate".into(),
        },
    )];
    e.sync(&sl, &mut Fr).unwrap();
    let _ = e.paint(Viewport::with_scale(400, 200, 1.0));
    let u = click(&mut e, 8.0, 8.0);
    assert!(
        u[0].ends_with(&s(&[("dep", "d")])),
        "K1: greyed dep still posts, gate left out: {u:?}"
    );
}
