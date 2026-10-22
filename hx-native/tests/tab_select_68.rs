//! Issue #68 (Designer lock, HTML_CONTRACT §2): focus that arrives by Tab or
//! Shift+Tab selects the box's whole text, so typing replaces the value.
//! Focus from a click puts the caret at the click point with no selection;
//! clicking a box that already has focus doesn't re-select. Focus set by the
//! host or by a swap selects nothing (caret at the end).

use std::collections::HashMap;

use hx_native::*;

fn wrap(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}\
         input{{display:block;width:200px;height:24px;font-size:14px;margin:4px;\
         font-family:\"DejaVu Sans\"}}</style></head><body>{body}</body></html>"
    )
}

const BODY: &str = r##"<form id="f"><section id="sec" hx-trigger="slice:s" hx-target="#sec" hx-swap="innerHTML"><input id="a" type="text" name="a" value="first" /></section><input id="fee" type="number" name="fee" value="4.1" min="0" max="100.00" step="0.01" hx-post="deal.fee" hx-trigger="change" /><input id="b" type="text" name="b" value="last" /></form>"##;

fn boot(mode: PaintMode) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: mode,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee"]);
    e.load_html(&wrap(BODY)).unwrap();
    let _ = e.paint(vp()).unwrap();
    e
}

fn vp() -> Viewport {
    Viewport::with_scale(300, 120, 1.0)
}

fn key(e: &mut Engine, k: &str, shift: bool) -> InputOutcome {
    e.input(InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            shift,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }))
}

fn click_at(e: &mut Engine, x: f32, y: f32) {
    for down in [true, false] {
        let ev = if down {
            InputEvent::PointerDown {
                x,
                y,
                button: PointerButton::Primary,
                mods: Modifiers::default(),
            }
        } else {
            InputEvent::PointerUp {
                x,
                y,
                button: PointerButton::Primary,
                mods: Modifiers::default(),
            }
        };
        let _ = e.input(ev);
    }
}

fn fee_box(e: &Engine) -> (f32, f32, f32, f32) {
    e.test_element_box("fee").unwrap()
}

fn changes(direct: InputOutcome, queued: Vec<InputOutcome>) -> Vec<String> {
    std::iter::once(direct)
        .chain(queued)
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

fn focused(e: &Engine) -> Option<String> {
    e.focus_state().focused_id.clone()
}

/// Tab from the field before, and Shift+Tab from the field after: the whole
/// text ("4.10", the step's decimals) is selected, and typing replaces it.
#[test]
fn tab_and_shift_tab_select_all_and_typing_replaces() {
    for (from, shift) in [("a", false), ("b", true)] {
        let mut e = boot(PaintMode::Exact);
        e.test_set_focus(from);
        let _ = key(&mut e, "Tab", shift);
        assert_eq!(focused(&e).as_deref(), Some("fee"), "from {from}");
        assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
        assert_eq!(
            e.test_selected_text("fee").as_deref(),
            Some("4.10"),
            "from {from} (shift {shift}): whole text selected"
        );
        let _ = e.input(InputEvent::Text("5".into()));
        assert_eq!(
            e.test_editor_display("fee").as_deref(),
            Some("5"),
            "typing replaces"
        );
        assert_eq!(e.test_selected_text("fee"), None);
    }
}

/// Text inputs follow the same rule (Win95 edit boxes, Chrome).
#[test]
fn tab_into_a_text_input_selects_all() {
    let mut e = boot(PaintMode::Exact);
    e.test_set_focus("fee");
    let _ = key(&mut e, "Tab", false);
    assert_eq!(focused(&e).as_deref(), Some("b"));
    assert_eq!(e.test_selected_text("b").as_deref(), Some("last"));
}

/// Replacing after Tab and committing with Tab out fires exactly one
/// `change`, with the typed value.
#[test]
fn replace_then_commit_fires_exactly_one_change() {
    let mut e = boot(PaintMode::Exact);
    e.test_set_focus("a");
    let d1 = key(&mut e, "Tab", false);
    let _ = e.input(InputEvent::Text("5".into()));
    let d2 = key(&mut e, "Tab", false);
    let mut all = changes(d1, Vec::new());
    all.extend(changes(d2, e.take_outcomes()));
    assert_eq!(all, vec!["5".to_string()], "one change, fee=5");
    assert_eq!(e.attr("fee", "value").as_deref(), Some("5"));
}

/// Click focus puts the caret at the click point with no selection: a click
/// near the left edge then typing inserts before the old text.
#[test]
fn click_focus_places_caret_without_selection() {
    let mut e = boot(PaintMode::Exact);
    let (x, y, _, h) = fee_box(&e);
    click_at(&mut e, x + 3.0, y + h / 2.0);
    assert_eq!(focused(&e).as_deref(), Some("fee"));
    assert_eq!(e.test_selected_text("fee"), None, "click selects nothing");
    let _ = e.input(InputEvent::Text("1".into()));
    assert_eq!(
        e.test_editor_display("fee").as_deref(),
        Some("14.10"),
        "caret at the click (start), not at the end and not replacing"
    );
}

/// Tab in (all selected), then a click on the same, focused box: the click
/// clears the selection and places the caret; it never re-selects.
#[test]
fn click_on_focused_box_does_not_reselect() {
    let mut e = boot(PaintMode::Exact);
    e.test_set_focus("a");
    let _ = key(&mut e, "Tab", false);
    assert!(e.test_selected_text("fee").is_some());
    let (x, y, _, h) = fee_box(&e);
    click_at(&mut e, x + 3.0, y + h / 2.0);
    assert_eq!(
        e.test_selected_text("fee"),
        None,
        "click clears, no re-select"
    );
    // Issue #85 (Ruling 7): an immediate second click at the same spot is a
    // double-click and selects the word, so this second click comes later.
    e.test_age_last_click(std::time::Duration::from_secs(1));
    click_at(&mut e, x + 3.0, y + h / 2.0);
    assert_eq!(
        e.test_selected_text("fee"),
        None,
        "second click still no selection"
    );
    let _ = e.input(InputEvent::Text("1".into()));
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("14.10"));
}

/// Host focus (`test_set_focus`) selects nothing; the caret is at the end.
#[test]
fn host_focus_selects_nothing() {
    let mut e = boot(PaintMode::Exact);
    e.test_set_focus("fee");
    assert_eq!(e.test_selected_text("fee"), None);
    let _ = e.input(InputEvent::Text("5".into()));
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.105"));
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
    fn render(&mut self, _s: &str, _slices: &[&str]) -> String {
        r#"<input id="a" type="text" name="a" value="swapped" />"#.into()
    }
}

/// A swap that replaces the focused field (same id) keeps focus without
/// selecting anything, even when focus had arrived by Tab.
#[test]
fn swap_refocus_selects_nothing() {
    let mut e = boot(PaintMode::Exact);
    e.test_set_focus("b");
    let _ = key(&mut e, "Tab", false); // wraps to #a
    assert_eq!(focused(&e).as_deref(), Some("a"));
    assert_eq!(e.test_selected_text("a").as_deref(), Some("first"));
    let slices = Slices(HashMap::from([("s".to_string(), 2)]));
    e.sync(&slices, &mut Frags).unwrap();
    let _ = e.paint(vp()).unwrap();
    assert_eq!(focused(&e).as_deref(), Some("a"));
    assert_eq!(e.test_editor_display("a").as_deref(), Some("swapped"));
    assert_eq!(
        e.test_selected_text("a"),
        None,
        "swap focus selects nothing"
    );
}

/// The selection highlight that Tab adds repaints partially exactly as a
/// forced full repaint draws it.
#[test]
fn selection_highlight_partial_equals_full() {
    let mut e = boot(PaintMode::Exact);
    e.test_set_focus("a");
    let _ = e.paint(vp()).unwrap();
    let _ = key(&mut e, "Tab", false);
    let out = e.paint(vp()).unwrap();
    assert!(!out.damage.is_empty(), "Tab must damage something");
    assert!(
        !e.last_paint_stats().full_repaint,
        "must be a partial repaint"
    );
    let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
    e.mark_theme_or_stylesheet_changed();
    let full = e.paint(vp()).unwrap().rgba.expect("full");
    let diff = partial
        .pixels
        .chunks_exact(4)
        .zip(full.pixels.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count();
    assert_eq!(diff, 0, "partial must equal full");
    // And the highlight is really there: the fee box differs from host focus.
    let mut h = boot(PaintMode::Exact);
    h.test_set_focus("fee");
    let host = h.paint(vp()).unwrap().rgba.unwrap();
    let (x, y, w, bh) = fee_box(&e);
    let mut highlight = 0;
    for yy in y as u32..(y + bh) as u32 {
        for xx in x as u32..(x + w) as u32 {
            let i = ((yy * full.width + xx) * 4) as usize;
            if full.pixels[i..i + 3] != host.pixels[i..i + 3] {
                highlight += 1;
            }
        }
    }
    assert!(highlight > 50, "selection highlight paints: {highlight} px");
}
