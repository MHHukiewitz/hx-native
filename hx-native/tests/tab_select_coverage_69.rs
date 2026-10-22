//! Graph review of #69 (issue #68): Tab-select coverage at scale 1 and 2
//! with a `:focus` restyle, Tab out, Esc, Enter, a grouped box, the number
//! filter over a selection, a range in the Tab order, and caret blink.
use hx_native::*;

const CSS: &str = "html,body{margin:0;background:#fff}\
 input{display:block;width:200px;height:24px;font-size:14px;margin:4px;font-family:\"DejaVu Sans\"}\
 input:focus{background:#ffffc0;border:2px solid #00f}";

fn page(body: &str) -> String {
    format!("<!DOCTYPE html><html><head><style>{CSS}</style></head><body>{body}</body></html>")
}

const BODY: &str = r##"<form id="f"><input id="a" type="text" name="a" value="first" /><input id="fee" type="number" name="fee" value="4.1" min="-50" max="100.00" step="0.01" hx-post="deal.fee" hx-trigger="change" /><input id="up" type="number" name="up" value="73480" min="0" max="999999" step="1" data-hx-group="on" /><input id="r" type="range" name="r" min="0" max="10" value="3" /><input id="b" type="text" name="b" value="last" /></form>"##;

fn boot(scale: f32) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee"]);
    e.load_html(&page(BODY)).unwrap();
    let _ = e.paint(vp(scale)).unwrap();
    e
}
fn vp(s: f32) -> Viewport {
    Viewport::with_scale((300.0 * s) as u32, (200.0 * s) as u32, s as f64)
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
/// Paint, then force a full repaint and count differing pixels.
fn partial_vs_full(e: &mut Engine, s: f32) -> (usize, bool) {
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
    (d, full_flag)
}

#[test]
fn p1_tab_walk_partial_equals_full_scale_1_and_2() {
    for s in [1.0f32, 2.0] {
        let mut e = boot(s);
        e.test_set_focus("a");
        let _ = e.paint(vp(s)).unwrap();
        let mut log = Vec::new();
        for (k, sh) in [
            ("Tab", false),
            ("Tab", false),
            ("Tab", false),
            ("Tab", false),
            ("Tab", true),
            ("Tab", true),
            ("Tab", true),
        ] {
            let _ = key(&mut e, k, sh);
            let f = e.focus_state().focused_id.clone();
            let (d, full) = partial_vs_full(&mut e, s);
            log.push(format!("{f:?} full={full} diff={d}"));
            assert_eq!(d, 0, "scale {s}: {log:?}");
        }
        eprintln!("scale {s}: {log:?}");
    }
}

#[test]
fn p2_tab_out_leaves_no_highlight() {
    for s in [1.0f32, 2.0] {
        // Tab a -> fee -> up: fee was tab-selected, then left.
        let mut e = boot(s);
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        let _ = e.paint(vp(s)).unwrap();
        let _ = key(&mut e, "Tab", false);
        let _ = e.paint(vp(s)).unwrap();
        e.mark_theme_or_stylesheet_changed();
        let got = e.paint(vp(s)).unwrap().rgba.unwrap();
        // Reference: focus put on `up` by the host, fee never selected.
        let mut r = boot(s);
        r.test_set_focus("up");
        let _ = r.paint(vp(s)).unwrap();
        r.mark_theme_or_stylesheet_changed();
        let want = r.paint(vp(s)).unwrap().rgba.unwrap();
        let (x, y, w, h) = e.test_element_box("fee").unwrap();
        let mut d = 0;
        for yy in (y * s) as u32..((y + h) * s) as u32 {
            for xx in (x * s) as u32..((x + w) * s) as u32 {
                let i = ((yy * got.width + xx) * 4) as usize;
                if got.pixels[i..i + 4] != want.pixels[i..i + 4] {
                    d += 1;
                }
            }
        }
        assert_eq!(d, 0, "scale {s}: fee keeps {d} highlight px after Tab out");
    }
}

#[test]
fn p3_esc_after_tab_select_reverts() {
    let mut e = boot(1.0);
    e.test_set_focus("a");
    let _ = key(&mut e, "Tab", false);
    let _ = e.input(InputEvent::Text("9".into()));
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("9"));
    let o = key(&mut e, "Escape", false);
    assert!(matches!(o, InputOutcome::None), "{o:?}");
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("4.10"));
    assert_eq!(e.attr("fee", "value").as_deref(), Some("4.1"));
    let (d, _) = partial_vs_full(&mut e, 1.0);
    assert_eq!(d, 0);
    eprintln!("after Esc selected={:?}", e.test_selected_text("fee"));
    // Tab out: no change.
    let o = key(&mut e, "Tab", false);
    let mut all = vec![o];
    all.extend(e.take_outcomes());
    assert!(
        !all.iter().any(|o| matches!(o, InputOutcome::Action { .. })),
        "{all:?}"
    );
}

#[test]
fn p4_enter_commit_after_replace_one_change() {
    let mut e = boot(2.0);
    e.test_set_focus("a");
    let _ = key(&mut e, "Tab", false);
    let _ = e.input(InputEvent::Text("7".into()));
    let o = key(&mut e, "Enter", false);
    let mut all = vec![o];
    all.extend(e.take_outcomes());
    let o2 = key(&mut e, "Tab", false);
    all.push(o2);
    all.extend(e.take_outcomes());
    let n = all
        .iter()
        .filter(|o| {
            matches!(
                o,
                InputOutcome::Action {
                    trigger: TriggerInfo::Change,
                    ..
                }
            )
        })
        .count();
    assert_eq!(n, 1, "{all:?}");
    assert_eq!(e.attr("fee", "value").as_deref(), Some("7"));
    let (d, _) = partial_vs_full(&mut e, 2.0);
    assert_eq!(d, 0);
}

#[test]
fn p5_grouped_box_selects_raw_and_replaces() {
    let mut e = boot(1.0);
    assert_eq!(e.test_editor_display("up").as_deref(), Some("73,480"));
    e.test_set_focus("fee");
    let _ = key(&mut e, "Tab", false);
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("up"));
    assert_eq!(e.test_selected_text("up").as_deref(), Some("73480"));
    let _ = e.input(InputEvent::Text("12".into()));
    assert_eq!(e.test_editor_display("up").as_deref(), Some("12"));
    let _ = key(&mut e, "Tab", false);
    assert_eq!(e.attr("up", "value").as_deref(), Some("12"));
}

#[test]
fn p6_selection_replaced_by_dot_sign_paste_backspace() {
    for (input, want) in [(".", "."), ("-", "-"), ("+", "+"), ("x", "4.10")] {
        let mut e = boot(1.0);
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        let _ = e.input(InputEvent::Text(input.into()));
        let got = e.test_editor_display("fee").unwrap();
        eprintln!("type {input:?} over selected 4.10 -> {got:?}");
        assert_eq!(got, want, "type {input:?}");
    }
    let mut e = boot(1.0);
    e.test_set_focus("a");
    let _ = key(&mut e, "Tab", false);
    let _ = e.input(InputEvent::Paste("3.45".into()));
    assert_eq!(e.test_editor_display("fee").as_deref(), Some("3.45"));
    let mut e = boot(1.0);
    e.test_set_focus("a");
    let _ = key(&mut e, "Tab", false);
    let _ = key(&mut e, "Backspace", false);
    assert_eq!(e.test_editor_display("fee").as_deref(), Some(""));
    let mut e = boot(1.0);
    e.test_set_focus("a");
    let _ = key(&mut e, "Tab", false);
    let _ = key(&mut e, "ArrowLeft", false);
    eprintln!(
        "ArrowLeft after tab-select: display={:?} sel={:?} value={:?}",
        e.test_editor_display("fee"),
        e.test_selected_text("fee"),
        e.attr("fee", "value")
    );
}

#[test]
fn p7_range_takes_focus_without_text_select_then_next() {
    let mut e = boot(1.0);
    e.test_set_focus("up");
    let _ = key(&mut e, "Tab", false);
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("r"));
    let (d, _) = partial_vs_full(&mut e, 1.0);
    assert_eq!(d, 0);
    let _ = key(&mut e, "Tab", false);
    assert_eq!(e.test_selected_text("b").as_deref(), Some("last"));
    let _ = key(&mut e, "Tab", true);
    let _ = key(&mut e, "Tab", true);
    assert_eq!(e.test_selected_text("up").as_deref(), Some("73480"));
}

#[test]
fn p8_caret_blink_with_selection_partial_equals_full() {
    for s in [1.0f32, 2.0] {
        let mut e = boot(s);
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        let _ = e.paint(vp(s)).unwrap();
        for _ in 0..3 {
            e.test_force_caret_blink();
            let (d, _) = partial_vs_full(&mut e, s);
            assert_eq!(d, 0, "scale {s}");
        }
    }
}
