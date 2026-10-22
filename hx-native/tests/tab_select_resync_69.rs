//! Graph review of #69 (issue #68): a Tab selection survives syncs that do
//! not replace the focused box (an unrelated swap, greying, `RequiresChange`
//! flips while typing), and a swap that does replace it re-seeds the box.
use hx_native::*;

const CSS: &str = "html,body{margin:0;background:#fff}\
 input{display:block;width:200px;height:24px;font-size:14px;margin:4px;font-family:\"DejaVu Sans\"}\
 input:focus{background:#ffffc0;border:2px solid #00f}";

fn page(body: &str) -> String {
    format!("<!DOCTYPE html><html><head><style>{CSS}</style></head><body>{body}</body></html>")
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

mod resync {
    use super::*;
    use std::collections::HashMap;

    const B2: &str = r##"<div id="other" hx-trigger="slice:o" hx-target="#other" hx-swap="innerHTML">x</div><form id="f"><input id="a" type="text" name="a" value="first" /><input id="fee" type="number" name="fee" value="4.1" min="0" max="100.00" step="0.01" /><input id="t" type="text" name="t" value="hello" /><button id="go" hx-post="deal.go">Go</button></form>"##;

    pub struct Src {
        pub v: HashMap<String, u64>,
        pub st: Vec<(String, ActionState)>,
    }
    impl SliceSource for Src {
        fn version(&self, name: &str) -> Option<u64> {
            self.v.get(name).copied()
        }
        fn action_states(&self) -> Vec<(String, ActionState)> {
            self.st.clone()
        }
    }
    struct Frags;
    impl FragmentSource for Frags {
        fn render(&mut self, _s: &str, _slices: &[&str]) -> String {
            "y".into()
        }
    }
    fn boot2(st: Vec<(String, ActionState)>) -> (Engine, Src) {
        let mut e = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            pending_timeout_frames: 5,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        e.declare_actions(&["deal.go"]);
        e.load_html(&page(B2)).unwrap();
        let src = Src {
            v: HashMap::from([("o".to_string(), 1)]),
            st,
        };
        e.sync(&src, &mut Frags).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        (e, src)
    }

    #[test]
    fn p9a_unrelated_swap_keeps_tab_selection() {
        let (mut e, mut src) = boot2(vec![]);
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        assert_eq!(e.test_selected_text("fee").as_deref(), Some("4.10"));
        src.v.insert("o".into(), 2);
        e.sync(&src, &mut Frags).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        let sel = e.test_selected_text("fee");
        let _ = e.input(InputEvent::Text("5".into()));
        let disp = e.test_editor_display("fee");
        eprintln!("unrelated swap: sel={sel:?} after typing 5 -> {disp:?}");
        assert_eq!(disp.as_deref(), Some("5"), "sel after swap {sel:?}");
    }

    #[test]
    fn p9b_greying_change_keeps_tab_selection() {
        let (mut e, mut src) = boot2(vec![]);
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        src.st = vec![(
            "deal.go".into(),
            ActionState::Disabled {
                reason: "no".into(),
            },
        )];
        e.sync(&src, &mut Frags).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        let sel = e.test_selected_text("fee");
        let _ = e.input(InputEvent::Text("5".into()));
        let disp = e.test_editor_display("fee");
        eprintln!("greying change: sel={sel:?} after typing 5 -> {disp:?}");
        assert_eq!(disp.as_deref(), Some("5"), "sel after greying {sel:?}");
    }

    #[test]
    fn p9c_requires_change_form_typing_after_tab() {
        let (mut e, _src) = boot2(vec![(
            "deal.go".into(),
            ActionState::RequiresChange {
                form: "f".into(),
                reason: "edit".into(),
            },
        )]);
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        let _ = e.paint(vp(1.0)).unwrap();
        for ch in ["5", "7"] {
            let _ = e.input(InputEvent::Text(ch.into()));
            let _ = e.paint(vp(1.0)).unwrap();
        }
        let disp = e.test_editor_display("fee");
        eprintln!(
            "requires-change: {disp:?} go disabled={:?}",
            e.attr("go", "disabled")
        );
        assert_eq!(disp.as_deref(), Some("57"));
        // Text box the same way.
        let _ = key(&mut e, "Tab", false);
        let _ = e.paint(vp(1.0)).unwrap();
        for ch in ["x", "y"] {
            let _ = e.input(InputEvent::Text(ch.into()));
            let _ = e.paint(vp(1.0)).unwrap();
        }
        assert_eq!(e.test_editor_display("t").as_deref(), Some("xy"));
    }
}

mod pre {
    use super::resync::*;
    use super::*;
    use std::collections::HashMap;
    struct F;
    impl FragmentSource for F {
        fn render(&mut self, _s: &str, _slices: &[&str]) -> String {
            "y".into()
        }
    }
    /// No Tab at all: host focus, Ctrl+A, type 5 then 7 in a RequiresChange form.
    #[test]
    fn p10_requires_change_typing_without_tab() {
        let mut e = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            pending_timeout_frames: 5,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        e.declare_actions(&["deal.go"]);
        e.load_html(&page(r##"<form id="f"><input id="fee" type="number" name="fee" value="4.1" min="0" max="100.00" step="0.01" /><button id="go" hx-post="deal.go">Go</button></form>"##)).unwrap();
        let src = Src {
            v: HashMap::new(),
            st: vec![(
                "deal.go".into(),
                ActionState::RequiresChange {
                    form: "f".into(),
                    reason: "edit".into(),
                },
            )],
        };
        e.sync(&src, &mut F).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        e.test_set_focus("fee");
        let _ = e.paint(vp(1.0)).unwrap();
        for _ in 0..4 {
            let _ = key(&mut e, "Backspace", false);
            let _ = e.paint(vp(1.0)).unwrap();
        }
        eprintln!(
            "after backspaces: {:?} go={:?}",
            e.test_editor_display("fee"),
            e.attr("go", "disabled")
        );
        for ch in ["5", "7"] {
            let _ = e.input(InputEvent::Text(ch.into()));
            let _ = e.paint(vp(1.0)).unwrap();
        }
        eprintln!(
            "p10: {:?} go={:?}",
            e.test_editor_display("fee"),
            e.attr("go", "disabled")
        );
        let o = key(&mut e, "Enter", false);
        let mut all = vec![o];
        all.extend(e.take_outcomes());
        eprintln!(
            "p10 enter: value={:?} outcomes={all:?}",
            e.attr("fee", "value")
        );
        assert_eq!(e.attr("fee", "value").as_deref(), Some("57"));
    }
}

mod outer {
    use super::resync::*;
    use super::*;
    use std::collections::HashMap;
    struct F;
    impl FragmentSource for F {
        fn render(&mut self, _s: &str, _slices: &[&str]) -> String {
            r##"<input id="fee" type="number" name="fee" value="8.5" min="0" max="100.00" step="0.01" hx-trigger="slice:s" hx-target="#fee" hx-swap="outerHTML" />"##.into()
        }
    }
    /// An outerHTML swap that replaces the Tab-focused field itself re-seeds
    /// it from the new markup: focus kept, nothing selected, caret at the end.
    #[test]
    fn p11_outer_html_swap_of_focused_field_reseeds() {
        let mut e = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            pending_timeout_frames: 5,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        e.load_html(&page(r##"<input id="a" type="text" name="a" value="first" /><input id="fee" type="number" name="fee" value="4.1" min="0" max="100.00" step="0.01" hx-trigger="slice:s" hx-target="#fee" hx-swap="outerHTML" />"##)).unwrap();
        let mut src = Src {
            v: HashMap::from([("s".to_string(), 1)]),
            st: vec![],
        };
        e.sync(&src, &mut F).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        assert_eq!(e.test_selected_text("fee").as_deref(), Some("8.50"));
        src.v.insert("s".into(), 2);
        e.sync(&src, &mut F).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        assert_eq!(e.focus_state().focused_id.as_deref(), Some("fee"));
        assert_eq!(e.test_selected_text("fee"), None);
        let _ = e.input(InputEvent::Text("1".into()));
        assert_eq!(e.test_editor_display("fee").as_deref(), Some("8.501"));
        // Esc goes back to the swapped-in value, not the pre-swap one.
        let _ = key(&mut e, "Escape", false);
        assert_eq!(e.attr("fee", "value").as_deref(), Some("8.5"));
        assert_eq!(e.test_editor_display("fee").as_deref(), Some("8.50"));
    }

    /// Replace the Tab-focused field, then leave it without editing: no
    /// `change` (the swapped-in value is the new value at focus).
    #[test]
    fn p12_replaced_field_tab_out_fires_no_change() {
        let mut e = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            pending_timeout_frames: 5,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        e.declare_actions(&["deal.fee"]);
        e.load_html(&page(r##"<input id="a" type="text" name="a" value="first" /><input id="fee" type="number" name="fee" value="4.1" min="0" max="100.00" step="0.01" hx-post="deal.fee" hx-trigger="change, slice:s" hx-target="#fee" hx-swap="outerHTML" /><input id="b" type="text" name="b" value="last" />"##)).unwrap();
        struct G;
        impl FragmentSource for G {
            fn render(&mut self, _s: &str, _slices: &[&str]) -> String {
                r##"<input id="fee" type="number" name="fee" value="8.5" min="0" max="100.00" step="0.01" hx-post="deal.fee" hx-trigger="change, slice:s" hx-target="#fee" hx-swap="outerHTML" />"##.into()
            }
        }
        let mut src = Src {
            v: HashMap::from([("s".to_string(), 1)]),
            st: vec![],
        };
        e.sync(&src, &mut G).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        e.test_set_focus("a");
        let _ = key(&mut e, "Tab", false);
        src.v.insert("s".into(), 2);
        e.sync(&src, &mut G).unwrap();
        let _ = e.paint(vp(1.0)).unwrap();
        let o = key(&mut e, "Tab", false);
        let mut all = vec![o];
        all.extend(e.take_outcomes());
        assert!(
            !all.iter().any(|o| matches!(o, InputOutcome::Action { .. })),
            "{all:?}"
        );
    }
}

mod inner {
    use super::resync::*;
    use super::*;
    use std::collections::HashMap;
    struct G(&'static str);
    impl FragmentSource for G {
        fn render(&mut self, _s: &str, _slices: &[&str]) -> String {
            self.0.into()
        }
    }
    /// innerHTML swap of the section around the Tab-focused box: the new
    /// box's value is the value at focus (Esc returns to it, Tab out is no change).
    #[test]
    fn p13_inner_html_replace_rebinds_value_at_focus() {
        for (body, frag) in [
            (
                r##"<section id="sec" hx-trigger="slice:s" hx-target="#sec" hx-swap="innerHTML"><input id="a" type="text" name="a" value="first" hx-post="deal.a" hx-trigger="change" /></section><input id="b" type="text" name="b" value="last" />"##,
                r##"<input id="a" type="text" name="a" value="swapped" hx-post="deal.a" hx-trigger="change" />"##,
            ),
            (
                r##"<section id="sec" hx-trigger="slice:s" hx-target="#sec" hx-swap="innerHTML"><span>x</span><input id="a" type="text" name="a" value="first" hx-post="deal.a" hx-trigger="change" /></section><input id="b" type="text" name="b" value="last" />"##,
                r##"<input id="a" type="text" name="a" value="swapped" hx-post="deal.a" hx-trigger="change" /><span>x</span>"##,
            ),
            (
                r##"<section id="sec" hx-trigger="slice:s" hx-target="#sec" hx-swap="innerHTML"><input id="a" type="text" name="a" value="first" hx-post="deal.a" hx-trigger="change" /><span>x</span></section><input id="b" type="text" name="b" value="last" />"##,
                r##"<span>x</span><input id="a" type="text" name="a" value="swapped" hx-post="deal.a" hx-trigger="change" />"##,
            ),
        ] {
            for esc in [true, false] {
                let mut e = Engine::new(EngineConfig {
                    max_paint_ns: 50_000_000,
                    pending_timeout_frames: 5,
                    paint_mode: PaintMode::Exact,
                    ..Default::default()
                });
                e.declare_actions(&["deal.a"]);
                e.load_html(&page(body)).unwrap();
                let mut src = Src {
                    v: HashMap::from([("s".to_string(), 1)]),
                    st: vec![],
                };
                let mut g = G(frag);
                e.sync(&src, &mut g).unwrap();
                let _ = e.paint(vp(1.0)).unwrap();
                e.test_set_focus("b");
                let _ = key(&mut e, "Tab", false);
                assert_eq!(e.focus_state().focused_id.as_deref(), Some("a"));
                src.v.insert("s".into(), 2);
                e.sync(&src, &mut g).unwrap();
                let _ = e.paint(vp(1.0)).unwrap();
                if esc {
                    let _ = key(&mut e, "Escape", false);
                    assert_eq!(
                        e.test_editor_display("a").as_deref(),
                        Some("swapped"),
                        "{body}"
                    );
                } else {
                    let o = key(&mut e, "Tab", false);
                    let mut all = vec![o];
                    all.extend(e.take_outcomes());
                    assert!(
                        !all.iter().any(|o| matches!(o, InputOutcome::Action { .. })),
                        "{body} {all:?}"
                    );
                }
            }
        }
    }
}
