//! Graph review of #86 (issue #85, HTML_CONTRACT §2 Ruling 7): the click
//! counter across a sync or a re-render, on the spin arrows (#84), and the
//! selection a multi-click leaves when the focused box is re-seeded (#69).

use std::collections::HashMap;
use std::time::Duration;

use hx_native::*;

const BODY: &str = r##"<form id="f"><section id="sec" hx-trigger="slice:s" hx-target="#sec" hx-swap="innerHTML"><input id="a" type="text" name="a" value="first word" /></section><span id="spin"><input id="fee" type="number" name="fee" value="4.69" min="0" max="100.00" step="0.01" hx-post="deal.fee" hx-trigger="change" /><i id="up" data-hx-step="up" data-hx-step-for="fee">U</i><i id="dn" data-hx-step="down" data-hx-step-for="fee">D</i></span><input id="b" type="text" name="b" value="hello brave world" /><div id="side" hx-trigger="slice:t" hx-target="#side" hx-swap="innerHTML">side 0</div></form>"##;

fn page() -> String {
    format!(
        "<!DOCTYPE html><html><head><style>*{{box-sizing:border-box}}html,body{{margin:0;background:#fff;\
         font:12px \"DejaVu Sans\"}}input{{display:block;width:200px;height:22px;margin:4px}}\
         #spin{{display:block;position:relative}}#spin i{{position:absolute;left:220px;width:20px;height:11px;\
         font-style:normal;font-size:8px}}#up{{top:4px}}#dn{{top:15px}}</style></head><body>{BODY}</body></html>"
    )
}

fn vp() -> Viewport {
    Viewport::with_scale(300, 160, 1.0)
}

fn boot() -> Engine {
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee"]);
    e.load_html(&page()).unwrap();
    // One frozen press clock (ac96196): presses are exactly as far apart as
    // the test moves it.
    e.test_advance_clock(Duration::ZERO);
    let _ = e.paint(vp()).unwrap();
    sync(&mut e, "t", 1);
    let _ = e.paint(vp()).unwrap();
    e
}

struct Slices(HashMap<String, u64>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
}
struct Frags;
impl FragmentSource for Frags {
    fn render(&mut self, s: &str, _slices: &[&str]) -> String {
        if s == "sec" {
            r#"<input id="a" type="text" name="a" value="first word" />"#.into()
        } else {
            "side 1".into()
        }
    }
}

/// `slice` "s" re-renders the section holding #a (same id, new node); "t"
/// re-renders an unrelated div.
fn sync(e: &mut Engine, slice: &str, v: u64) {
    let mut m = HashMap::from([("s".to_string(), 1), ("t".to_string(), 1)]);
    m.insert(slice.to_string(), v);
    e.sync(&Slices(m), &mut Frags).unwrap();
}

fn down(e: &mut Engine, (x, y): (f32, f32)) -> InputOutcome {
    e.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    })
}

fn up(e: &mut Engine, (x, y): (f32, f32)) -> InputOutcome {
    e.input(InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    })
}

fn click(e: &mut Engine, p: (f32, f32)) -> Vec<InputOutcome> {
    let mut out = vec![down(e, p), up(e, p)];
    out.extend(e.take_outcomes());
    out
}

fn at(e: &Engine, id: &str, frac: f32) -> (f32, f32) {
    let c = e.test_content_box(id).unwrap();
    (c.0 + c.2 * frac, c.1 + c.3 / 2.0)
}

fn mid(e: &Engine, id: &str) -> (f32, f32) {
    let b = e.test_element_box(id).unwrap();
    (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
}

fn sel(e: &mut Engine, id: &str) -> Option<String> {
    e.test_selected_text(id)
}

/// The host finishes every request (Mainnet's Game handles actions at once).
fn complete_all(e: &mut Engine, outs: &[InputOutcome]) {
    for o in outs {
        if let InputOutcome::Action { request, .. } = o {
            e.complete(*request, Ok(()));
        }
    }
}

fn changes(outs: &[InputOutcome]) -> Vec<String> {
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

/// #69 class: a sync that leaves the focused box's node in place must not
/// re-seed it, so a double- or triple-click selection survives it.
#[test]
fn g1_multi_click_selection_survives_an_unrelated_sync() {
    for (id, n, want) in [
        ("fee", 2, "4.69"),
        ("fee", 3, "4.69"),
        ("b", 3, "hello brave world"),
    ] {
        let mut e = boot();
        let p = at(&e, id, 0.5);
        for _ in 0..n {
            click(&mut e, p);
        }
        let _ = e.paint(vp()).unwrap();
        assert_eq!(
            sel(&mut e, id).as_deref(),
            Some(want),
            "{id} x{n} before sync"
        );
        sync(&mut e, "t", 2);
        let _ = e.paint(vp()).unwrap();
        assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.69"));
        assert_eq!(
            sel(&mut e, id).as_deref(),
            Some(want),
            "{id} x{n} after sync"
        );
        e.input(InputEvent::Text("7".into()));
        assert_eq!(e.test_editor_display(id).as_deref(), Some("7"), "{id} x{n}");
    }
}

/// The click run continues across an unrelated sync and paint between the
/// presses (the host syncs every frame).
#[test]
fn g2_click_run_continues_across_an_unrelated_sync() {
    let mut e = boot();
    let p = at(&e, "b", 0.5);
    click(&mut e, p);
    sync(&mut e, "t", 2);
    let _ = e.paint(vp()).unwrap();
    click(&mut e, p);
    let word = sel(&mut e, "b").unwrap_or_default();
    assert!(!word.is_empty() && word != "hello brave world", "{word:?}");
    sync(&mut e, "t", 3);
    let _ = e.paint(vp()).unwrap();
    click(&mut e, p);
    assert_eq!(sel(&mut e, "b").as_deref(), Some("hello brave world"));
}

/// A re-render that replaces the focused box (same id) between the presses:
/// the run is keyed by id, so it continues on the new node; and the
/// deferred re-seed (`needs_prepare`) runs before the press, so it doesn't
/// wipe the selection the press makes.
#[test]
fn g3_click_run_across_a_replacing_rerender() {
    // Replaced between press 1 and press 2, painted in between.
    let mut e = boot();
    let p = at(&e, "a", 0.1);
    click(&mut e, p);
    sync(&mut e, "s", 2);
    let _ = e.paint(vp()).unwrap();
    click(&mut e, p);
    let _ = e.paint(vp()).unwrap();
    assert_eq!(
        sel(&mut e, "a").as_deref(),
        Some("first"),
        "double-click on the new node"
    );
    click(&mut e, p);
    let _ = e.paint(vp()).unwrap();
    assert_eq!(sel(&mut e, "a").as_deref(), Some("first word"));

    // Replaced with no paint before the presses: the re-seed is still pending.
    let mut e = boot();
    let p = at(&e, "a", 0.1);
    click(&mut e, p);
    let _ = e.paint(vp()).unwrap();
    e.test_advance_clock(Duration::from_secs(1));
    sync(&mut e, "s", 2);
    click(&mut e, p);
    click(&mut e, p);
    click(&mut e, p);
    let _ = e.paint(vp()).unwrap();
    assert_eq!(
        sel(&mut e, "a").as_deref(),
        Some("first word"),
        "the pending re-seed must not wipe the triple-click"
    );
    e.input(InputEvent::Text("x".into()));
    assert_eq!(e.test_editor_display("a").as_deref(), Some("x"));

    // A replacing re-render after the selection clears it (by design: focus
    // restored after a swap selects nothing, #68).
    let mut e = boot();
    let p = at(&e, "a", 0.1);
    for _ in 0..3 {
        click(&mut e, p);
    }
    sync(&mut e, "s", 2);
    let _ = e.paint(vp()).unwrap();
    assert_eq!(sel(&mut e, "a"), None);
}

/// #84: the arrows never reach the counter. Each press of a double-click on
/// ▲ steps, each release sends its own change, nothing is selected; and an
/// arrow press breaks a text click run.
#[test]
fn g4_double_click_on_an_arrow_steps_twice() {
    let mut e = boot();
    let u = mid(&e, "up");
    let mut outs = click(&mut e, u);
    complete_all(&mut e, &outs);
    let second = click(&mut e, u);
    complete_all(&mut e, &second);
    outs.extend(second);
    let _ = e.paint(vp()).unwrap();
    assert_eq!(e.attr("fee", "value").as_deref(), Some("4.71"));
    assert_eq!(changes(&outs), vec!["4.7".to_string(), "4.71".to_string()]);
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("fee"));
    assert_eq!(
        sel(&mut e, "fee"),
        None,
        "an arrow double-click selects nothing"
    );
    let mut outs = click(&mut e, u);
    let _ = e.paint(vp()).unwrap();
    outs.extend(e.take_outcomes());
    assert_eq!(
        e.attr("fee", "value").as_deref(),
        Some("4.72"),
        "third press"
    );
    assert_eq!(changes(&outs), vec!["4.72".to_string()]);
    assert_eq!(sel(&mut e, "fee"), None);

    // Text click, arrow press, text click at the first spot: a single click.
    let mut e = boot();
    let p = at(&e, "fee", 0.5);
    click(&mut e, p);
    click(&mut e, u);
    click(&mut e, p);
    assert_eq!(sel(&mut e, "fee"), None, "the arrow press broke the run");
    // An arrow press between press 2 and 3: the third is a new single click.
    let mut e = boot();
    click(&mut e, p);
    click(&mut e, p);
    assert_eq!(sel(&mut e, "fee").as_deref(), Some("4.69"));
    click(&mut e, u);
    click(&mut e, p);
    assert_eq!(sel(&mut e, "fee"), None);
}

/// A held arrow, then a fast click in the box: the hold has ended on the
/// release, and the click is a single click with the caret placed.
#[test]
fn g5_hold_then_click_in_the_box_is_a_single_click() {
    let mut e = boot();
    let u = mid(&e, "up");
    down(&mut e, u);
    e.test_hold_spin_for(Duration::from_millis(500));
    up(&mut e, u);
    let _ = e.take_outcomes();
    let v = e.attr("fee", "value").unwrap();
    let p = at(&e, "fee", 0.5);
    click(&mut e, p);
    assert_eq!(sel(&mut e, "fee"), None);
    click(&mut e, p);
    assert_eq!(sel(&mut e, "fee").as_deref(), Some(v.as_str()));
}

/// Interval boundaries on the frozen clock: 0, 499 and 500 ms continue the
/// run (inclusive, as #86 rules), 501 and 600 ms do not.
#[test]
fn g6_interval_boundaries() {
    for (ms, double) in [
        (0_u64, true),
        (499, true),
        (500, true),
        (501, false),
        (600, false),
    ] {
        let mut e = boot();
        let p = at(&e, "b", 0.5);
        click(&mut e, p);
        e.test_advance_clock(Duration::from_millis(ms));
        click(&mut e, p);
        assert_eq!(sel(&mut e, "b").is_some(), double, "{ms} ms");
    }
}

/// A PointerLeave between presses doesn't touch the count (the pointer can
/// only come back within 500 ms and 2 px by a host quirk); a key between the
/// presses doesn't either. Recorded, not ruled.
#[test]
fn g7_leave_or_key_between_presses_keeps_the_run() {
    let mut e = boot();
    let p = at(&e, "b", 0.5);
    click(&mut e, p);
    e.input(InputEvent::PointerLeave);
    click(&mut e, p);
    assert!(
        sel(&mut e, "b").is_some(),
        "leave between presses: still a double-click"
    );
    let mut e = boot();
    click(&mut e, p);
    e.input(InputEvent::Text("z".into()));
    click(&mut e, p);
    assert!(
        sel(&mut e, "b").is_some(),
        "typing between presses: still a double-click"
    );
}
