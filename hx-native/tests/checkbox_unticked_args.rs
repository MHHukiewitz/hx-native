//! Mainnet #791 (UI): a checkbox's `change` must post the box only while it
//! is ticked. Real htmx serializes like an HTML form (`shouldInclude`:
//! a checkbox or radio is included only when `checked`), for the element
//! that fired as well as for the rest of the form, so an untick sends the
//! form without that box.

use hx_native::*;

fn boot(html: &str, actions: &[&str]) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(actions);
    let style = "<style>html,body{margin:0} input{position:absolute;width:16px;height:16px;margin:0}</style>";
    eng.load_html(&format!(
        "<!DOCTYPE html><html><head>{style}</head><body>{html}</body></html>"
    ))
    .unwrap();
    let _ = eng.paint(Viewport::with_scale(300, 100, 1.0)).unwrap();
    eng
}

/// Every `change` args list the event produced; each request is completed
/// so the next one is not held by `hx-sync`.
fn sent(eng: &mut Engine, ev: InputEvent) -> Vec<Vec<(String, String)>> {
    let first = eng.input(ev);
    let mut out = Vec::new();
    for o in std::iter::once(first).chain(eng.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            trigger: TriggerInfo::Change,
            args,
            ..
        } = o
        {
            eng.complete(request, Ok(()));
            out.push(args);
        }
    }
    out
}

fn click(eng: &mut Engine, x: f32, y: f32) -> Vec<Vec<(String, String)>> {
    let _ = eng.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    sent(
        eng,
        InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
    )
}

fn space(eng: &mut Engine) -> Vec<Vec<(String, String)>> {
    sent(
        eng,
        InputEvent::Key(KeyEvent {
            key: " ".into(),
            code: "Space".into(),
            mods: Modifiers::default(),
            repeat: false,
            pressed: true,
        }),
    )
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

const FORM: &str = r#"<form id="f">
<input id="c" type="checkbox" name="music" value="on" hx-post="set.music" hx-trigger="change" style="left:0;top:0" />
<input id="a" type="checkbox" name="state" value="a" hx-post="set.state" hx-trigger="change" style="left:30px;top:0" />
<input id="b" type="checkbox" name="state" value="b" checked="" hx-post="set.state" hx-trigger="change" style="left:60px;top:0" />
<input id="d" type="checkbox" name="state" value="d" checked="" hx-post="set.state" hx-trigger="change" style="left:90px;top:0" />
<input id="r1" type="radio" name="ops" value="1" hx-post="set.ops" hx-trigger="change" style="left:0;top:40px" />
<input id="r2" type="radio" name="ops" value="2" checked="" hx-post="set.ops" hx-trigger="change" style="left:30px;top:40px" />
</form>"#;

fn engine() -> Engine {
    boot(FORM, &["set.music", "set.state", "set.ops"])
}

/// Tick posts the box; untick posts the form without it.
#[test]
fn untick_leaves_the_box_out() {
    let mut e = engine();
    assert_eq!(
        click(&mut e, 8.0, 8.0),
        [pairs(&[
            ("music", "on"),
            ("state", "b"),
            ("state", "d"),
            ("ops", "2")
        ])],
        "tick"
    );
    assert_eq!(
        click(&mut e, 8.0, 8.0),
        [pairs(&[("state", "b"), ("state", "d"), ("ops", "2")])],
        "untick: no music"
    );
}

/// The same by keyboard (Space on the focused box).
#[test]
fn space_untick_leaves_the_box_out() {
    let mut e = engine();
    e.test_set_focus("c");
    assert_eq!(
        space(&mut e),
        [pairs(&[
            ("music", "on"),
            ("state", "b"),
            ("state", "d"),
            ("ops", "2")
        ])]
    );
    assert_eq!(
        space(&mut e),
        [pairs(&[("state", "b"), ("state", "d"), ("ops", "2")])]
    );
}

/// Repeated names: unticking one of several `state` boxes posts the others
/// only; ticking one posts it once, plus the others.
#[test]
fn repeated_names_post_only_ticked_boxes() {
    let mut e = engine();
    assert_eq!(
        click(&mut e, 98.0, 8.0),
        [pairs(&[("state", "b"), ("ops", "2")])],
        "untick d"
    );
    assert_eq!(
        click(&mut e, 68.0, 8.0),
        [pairs(&[("ops", "2")])],
        "untick b: no state at all"
    );
    assert_eq!(
        click(&mut e, 38.0, 8.0),
        [pairs(&[("state", "a"), ("ops", "2")])],
        "tick a"
    );
}

/// Radios already follow the rule: the picked one is posted once, the one
/// it replaced is not.
#[test]
fn radio_posts_only_the_picked_one() {
    let mut e = engine();
    assert_eq!(
        click(&mut e, 8.0, 48.0),
        [pairs(&[("ops", "1"), ("state", "b"), ("state", "d")])]
    );
    // The rest of the form now carries only the picked radio.
    assert_eq!(
        click(&mut e, 98.0, 8.0),
        [pairs(&[("state", "b"), ("ops", "1")])]
    );
}
