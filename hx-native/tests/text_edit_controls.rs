//! Issue #17 Graph HOLD follow-up: blur Change delivery, Tab order, swap
//! reconcile, focus-ring damage, caret blink without clear_focus, right-click
//! vs open select, ±2^53 number step — plus a port of Graph's probe19 parity
//! matrix (driven by real clicks / Tab, not `test_set_focus`).

use std::collections::HashMap;

use hx_native::*;

// ---------------------------------------------------------------------------
// Shared boot / input helpers (click-driven)
// ---------------------------------------------------------------------------

struct Slices {
    v: HashMap<String, u64>,
    acts: Vec<(String, ActionState)>,
}
impl SliceSource for Slices {
    fn version(&self, n: &str) -> Option<u64> {
        self.v.get(n).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.acts.clone()
    }
}
struct Frag(String);
impl FragmentSource for Frag {
    fn render(&mut self, _: &str, _: &[&str]) -> String {
        self.0.clone()
    }
}

const S_FIELD: &str = r#"<input id="s" type="text" name="s" value="swap" hx-post="set.s" hx-trigger="change" style="width:100px;height:22px;display:block"/>"#;

fn page_html(transformed: bool) -> String {
    let inner = format!(
        r##"
<div id="bg" style="position:absolute;left:0;top:0;width:280px;height:180px;background:#408040"></div>
<input id="t" type="text" name="t" value="hello" hx-post="set.t" hx-trigger="change" style="position:absolute;left:13.3px;top:17.7px;width:120px;height:22px;font-size:14px"/>
<input id="n" type="number" name="n" value="1234" min="0" max="5000" step="1" hx-post="set.n" hx-trigger="change" style="position:absolute;left:13.3px;top:57.7px;width:120px;height:22px"/>
<input id="d" type="text" name="d" value="locked" disabled="" style="position:absolute;left:150px;top:17.7px;width:100px;height:22px"/>
<section id="slot" hx-trigger="slice:ui" hx-target="#slot" hx-swap="innerHTML" style="position:absolute;left:150px;top:57.7px;width:110px">{S_FIELD}</section>
<button id="other" hx-post="noop" style="position:absolute;left:150px;top:120px">other</button>"##
    );
    let body = if transformed {
        format!(
            r#"<div id="panel" class="pause-panel" style="position:absolute;left:50%;top:50%;width:290px;height:190px;transform:translate(-50%,-50%);background:#c0c0c0">{inner}</div>"#
        )
    } else {
        inner
    };
    let edge = r#"<input id="edge" type="text" name="edge" value="edge" hx-post="set.e" hx-trigger="change" style="position:absolute;left:340px;top:285px;width:120px;height:24px"/>"#;
    format!(
        "<!DOCTYPE html><html><body style=\"margin:0;width:400px;height:300px;background:#fff\">{body}{edge}</body></html>"
    )
}

fn vp(s: f64) -> Viewport {
    Viewport {
        width: (400.0 * s).round() as u32,
        height: (300.0 * s).round() as u32,
        scale: s,
        holes: vec![],
    }
}

fn boot(transformed: bool, scale: f64) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["set.t", "set.n", "set.s", "set.e", "noop", "set.lang"]);
    e.load_html(&page_html(transformed)).unwrap();
    let _ = e.paint(vp(scale)).unwrap();
    e
}

fn mods() -> Modifiers {
    Modifiers::default()
}

fn ptr(e: &mut Engine, x: f32, y: f32) -> InputOutcome {
    let _ = e.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: mods(),
    });
    e.input(InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: mods(),
    })
}

fn center(e: &Engine, id: &str) -> (f32, f32) {
    let b = e.test_element_box(id).unwrap();
    (b.0 + b.2 * 0.5, b.1 + b.3 * 0.5)
}

fn click(e: &mut Engine, id: &str) -> InputOutcome {
    let (x, y) = center(e, id);
    ptr(e, x, y)
}

fn key_press(e: &mut Engine, key: &str, shift: bool, ctrl: bool) -> InputOutcome {
    e.input(InputEvent::Key(KeyEvent {
        key: key.into(),
        code: key.into(),
        mods: Modifiers {
            shift,
            ctrl,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    }))
}

fn collect_change_actions(direct: InputOutcome, queued: Vec<InputOutcome>) -> Vec<InputOutcome> {
    let mut out = Vec::new();
    for o in std::iter::once(direct).chain(queued) {
        match o {
            InputOutcome::Action {
                ref trigger,
                ref name,
                ..
            } if matches!(trigger, TriggerInfo::Change) => {
                assert!(
                    name.starts_with("set."),
                    "change action name unexpected: {name}"
                );
                out.push(o);
            }
            _ => {}
        }
    }
    out
}

fn assert_partial_eq_full(partial: &RgbaImage, full: &RgbaImage, label: &str) {
    assert_eq!(
        partial.pixels,
        full.pixels,
        "{label}: partial vs full differ ({} px)",
        partial
            .pixels
            .iter()
            .zip(full.pixels.iter())
            .filter(|(a, b)| a != b)
            .count()
    );
}

fn damage_overlaps_box(damage: &[Rect], box_css: (f32, f32, f32, f32), scale: f64) -> bool {
    let (bx, by, bw, bh) = box_css;
    let s = scale as f32;
    let ix0 = bx * s;
    let iy0 = by * s;
    let ix1 = (bx + bw) * s;
    let iy1 = (by + bh) * s;
    // Inflate expected hit by ~4 CSS px so :focus outline outside the border
    // box still counts (mutation: border-only damage fails at scale 1).
    let pad = 4.0 * s;
    damage.iter().any(|r| {
        r.x < ix1 + pad
            && r.x + r.width > ix0 - pad
            && r.y < iy1 + pad
            && r.y + r.height > iy0 - pad
    })
}

// ---------------------------------------------------------------------------
// Blocker 1: blur paths deliver exactly one Change, no pending timeout
// ---------------------------------------------------------------------------

/// Mutation: `let _ = self.blur_text_edit(ctx)` without queueing → Action lost,
/// pending stays, timeout after 5 frames.
fn blur_path_expect_one_change(label: &str, blur: impl FnOnce(&mut Engine) -> InputOutcome) {
    let mut e = boot(false, 1.0);
    let _ = click(&mut e, "t");
    let _ = e.input(InputEvent::Text("q".into()));
    let direct = blur(&mut e);
    let queued = e.take_outcomes();
    let changes = collect_change_actions(direct.clone(), queued.clone());
    assert_eq!(
        changes.len(),
        1,
        "{label}: expected exactly one Change Action, got {changes:?}"
    );
    // Host completes every Action (Change from blur, plus Click on a button like
    // `#other`). Leaving the button Click pending is not a blur-delivery bug.
    for o in std::iter::once(direct).chain(queued) {
        if let InputOutcome::Action { request, .. } = o {
            e.complete(request, Ok(()));
        }
    }
    assert!(
        !e.has_pending(),
        "{label}: after complete, no leftover pending"
    );
    for _ in 0..8 {
        e.test_tick_frame();
        let _ = e.paint(vp(1.0));
    }
    assert!(
        e.pending_timeout_failures().is_empty(),
        "{label}: no pending timeout after blur, got {:?}",
        e.pending_timeout_failures()
    );
    assert_eq!(e.attr("t", "value").as_deref(), Some("helloq"));
}

#[test]
fn blur_click_elsewhere_delivers_change() {
    blur_path_expect_one_change("click_other", |e| click(e, "other"));
}

#[test]
fn blur_click_background_delivers_change() {
    blur_path_expect_one_change("click_bg", |e| {
        let b = e.test_element_box("bg").unwrap();
        ptr(e, b.0 + b.2 - 5.0, b.1 + b.3 - 5.0)
    });
}

#[test]
fn blur_tab_delivers_change() {
    blur_path_expect_one_change("tab", |e| key_press(e, "Tab", false, false));
}

#[test]
fn blur_click_disabled_delivers_change() {
    blur_path_expect_one_change("click_disabled", |e| click(e, "d"));
}

#[test]
fn blur_switch_field_delivers_change() {
    blur_path_expect_one_change("switch_field", |e| click(e, "n"));
}

#[test]
fn unchanged_enter_does_not_fire_change() {
    let mut e = boot(false, 1.0);
    let _ = click(&mut e, "t");
    let o = key_press(&mut e, "Enter", false, false);
    let queued = e.take_outcomes();
    let changes = collect_change_actions(o, queued);
    assert!(
        changes.is_empty(),
        "Enter with no edit must not fire Change, got {changes:?}"
    );
}

// ---------------------------------------------------------------------------
// Blocker 2: Tab / Shift+Tab step from the field that had focus
// ---------------------------------------------------------------------------

#[test]
fn tab_from_number_goes_to_next_not_first() {
    let mut e = boot(false, 1.0);
    let order = e.focusable_ids_for_test();
    let n_idx = order.iter().position(|id| id == "n").expect("n in order");
    let expect_next = order.get(n_idx + 1).cloned();
    let _ = click(&mut e, "n");
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"));
    let _ = key_press(&mut e, "Tab", false, false);
    assert_eq!(
        e.focus_state().focused_id,
        expect_next,
        "Tab from n must advance in order {order:?}, not jump to first"
    );

    let _ = click(&mut e, "n");
    let expect_prev = if n_idx > 0 {
        Some(order[n_idx - 1].clone())
    } else {
        None
    };
    let _ = key_press(&mut e, "Tab", true, false);
    assert_eq!(
        e.focus_state().focused_id,
        expect_prev,
        "Shift+Tab from n must go to previous in order {order:?}"
    );
}

// ---------------------------------------------------------------------------
// Blocker 3: reconcile TextEditSession after swap
// ---------------------------------------------------------------------------

#[test]
fn swap_replace_same_id_rebounds_session() {
    let mut e = boot(false, 1.0);
    let _ = click(&mut e, "s");
    let _ = e.input(InputEvent::Text("x".into()));
    let sl = Slices {
        v: HashMap::from([("ui".into(), 1)]),
        acts: vec![],
    };
    let _ = e
        .sync(&sl, &mut Frag(S_FIELD.into()))
        .expect("swap replace");
    // Typing must still land in the re-bound field (no paint required between
    // sync and Text — prepare runs via ensure_resolved on first edit).
    let _ = e.input(InputEvent::Text("y".into()));
    let v = e.attr("s", "value").unwrap_or_default();
    assert!(
        v.contains('y'),
        "after same-id swap, typing must edit the new node, value={v:?}"
    );
    assert!(
        v.ends_with('y'),
        "caret must be at end after rebind prepare, value={v:?}"
    );
}

#[test]
fn swap_remove_focused_ends_session() {
    let mut e = boot(false, 1.0);
    let _ = click(&mut e, "s");
    let _ = e.input(InputEvent::Text("x".into()));
    let sl = Slices {
        v: HashMap::from([("ui".into(), 1)]),
        acts: vec![],
    };
    let _ = e
        .sync(&sl, &mut Frag("<span id=\"gone\">removed</span>".into()))
        .expect("swap remove");
    let o = e.input(InputEvent::Text("y".into()));
    assert!(
        matches!(o, InputOutcome::Unhandled(_)),
        "after remove, Text must be Unhandled (no stale session), got {o:?}"
    );
    assert!(e.focus_state().focused_id.is_none());
}

// ---------------------------------------------------------------------------
// Blocker 6: right-click text field closes open select
// ---------------------------------------------------------------------------

#[test]
fn right_click_text_field_closes_open_select() {
    let html = r#"<!DOCTYPE html><html><body style="margin:0;width:400px;height:300px">
<select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
        style="position:absolute;left:10px;top:10px;width:150px;height:22px">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
  <option value="fr">Fr</option>
</select>
<input id="t" type="text" name="t" value="hello" hx-post="set.t" hx-trigger="change"
       style="position:absolute;left:200px;top:10px;width:120px;height:22px"/>
</body></html>"#;
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["set.lang", "set.t"]);
    e.load_html(html).unwrap();
    let _ = e.paint(vp(1.0)).unwrap();

    // Open select via Alt+ArrowDown (same as form_controls helper).
    let _ = click(&mut e, "lang");
    let _ = key_press(&mut e, "ArrowDown", false, false); // may step; open via alt
    let _ = e.input(InputEvent::Key(KeyEvent {
        key: "ArrowDown".into(),
        code: "ArrowDown".into(),
        mods: Modifiers {
            alt: true,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    }));
    let _ = e.paint(vp(1.0)).unwrap();

    // Secondary click on the text field must close the list.
    let (tx, ty) = center(&e, "t");
    let _ = e.input(InputEvent::PointerDown {
        x: tx,
        y: ty,
        button: PointerButton::Secondary,
        mods: mods(),
    });
    let _ = e.input(InputEvent::PointerUp {
        x: tx,
        y: ty,
        button: PointerButton::Secondary,
        mods: mods(),
    });

    // Enter must NOT fire set.lang from a still-open list.
    let o = key_press(&mut e, "Enter", false, false);
    let queued = e.take_outcomes();
    for o in std::iter::once(o).chain(queued) {
        if let InputOutcome::Action { name, .. } = o {
            assert_ne!(
                name, "set.lang",
                "Enter after right-click on text must not pick from a leftover open select"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Blocker 7: number step beyond ±2^53 does not panic / wrap
// ---------------------------------------------------------------------------

#[test]
fn number_step_clamps_beyond_safe_integer() {
    let html = r#"<!DOCTYPE html><html><body>
<input id="big" type="number" name="big" value="9007199254740992" step="1"
       hx-post="set.big" hx-trigger="change"
       style="width:200px;height:22px"/>
</body></html>"#;
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["set.big"]);
    e.load_html(html).unwrap();
    let _ = e.paint(vp(1.0)).unwrap();
    let _ = click(&mut e, "big");
    // Must not panic (debug) or wrap (release).
    let _ = key_press(&mut e, "ArrowUp", false, false);
    let v = e.attr("big", "value").unwrap_or_default();
    let n: f64 = v.parse().expect("numeric value");
    assert!(
        n.is_finite() && n.abs() <= 9_007_199_254_740_992.0,
        "step must stay within ±2^53, got {v}"
    );
}

#[test]
fn number_commit_clamps_twenty_nines() {
    let html = r#"<!DOCTYPE html><html><body>
<input id="big" type="number" name="big" value="1" step="1"
       hx-post="set.big" hx-trigger="change"
       style="width:200px;height:22px"/>
</body></html>"#;
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["set.big"]);
    e.load_html(html).unwrap();
    let _ = e.paint(vp(1.0)).unwrap();
    let _ = click(&mut e, "big");
    let _ = key_press(&mut e, "a", false, true);
    let _ = e.input(InputEvent::Text("99999999999999999999".into()));
    let _ = key_press(&mut e, "Enter", false, false);
    let v = e.attr("big", "value").unwrap_or_default();
    let n: f64 = v.parse().unwrap_or(f64::NAN);
    assert!(
        n.is_finite() && n.abs() <= 9_007_199_254_740_992.0,
        "commit must clamp to ±2^53, got {v}"
    );
    // ArrowUp after huge commit must not panic.
    let _ = key_press(&mut e, "ArrowUp", false, false);
}

// ---------------------------------------------------------------------------
// Focus-ring / typing damage: click-driven, all scales (blocker 4 + mutations)
// ---------------------------------------------------------------------------

/// Mutation notes (verified):
/// - `absolute_position` in `queue_text_field_damage` → damage misses outline /
///   translated box at scale 1 (overlap assert).
/// - Skip `queue_text_field_damage` on Text / arrows / Backspace → paint skip
///   or overlap miss.
/// - Shrinking this loop to `[1.0]` alone is not enough to hide a scale-1 bug:
///   the scale-1 iteration asserts overlap against the transform-aware box.
#[test]
fn type_focus_damage_overlaps_ink_all_scales() {
    const SCALES: [f64; 3] = [1.0, 1.25, 2.0];
    for transformed in [false, true] {
        for scale in SCALES {
            let mut e = boot(transformed, scale);
            let direct = click(&mut e, "t");
            let _ = direct;
            let box_css = e.test_element_box("t").expect("t box");
            let focus_paint = e.paint(vp(scale)).unwrap();
            assert!(
                !focus_paint.skipped && !e.last_paint_stats().full_repaint,
                "transformed={transformed} scale={scale}: focus must partial-paint"
            );
            assert!(
                damage_overlaps_box(&focus_paint.damage, box_css, scale),
                "transformed={transformed} scale={scale}: focus damage must cover ink/outline; \
                 box={box_css:?} damage={:?}",
                focus_paint.damage
            );

            let _ = e.input(InputEvent::Text("Hi".into()));
            let type_paint = e.paint(vp(scale)).unwrap();
            assert!(
                !type_paint.skipped && !e.last_paint_stats().full_repaint,
                "transformed={transformed} scale={scale}: type must partial-paint"
            );
            assert!(
                damage_overlaps_box(&type_paint.damage, box_css, scale),
                "transformed={transformed} scale={scale}: type damage must cover field"
            );

            // Partial vs full byte-identical after typing.
            let partial_px = type_paint.rgba.unwrap();
            let mut full_e = boot(transformed, scale);
            let _ = click(&mut full_e, "t");
            let _ = full_e.paint(vp(scale)).unwrap();
            let _ = full_e.input(InputEvent::Text("Hi".into()));
            full_e.mark_theme_or_stylesheet_changed();
            let full = full_e.paint(vp(scale)).unwrap();
            assert_partial_eq_full(
                &partial_px,
                full.rgba.as_ref().unwrap(),
                &format!("type focus transformed={transformed} scale={scale}"),
            );
        }
    }
}

#[test]
fn arrow_backspace_commit_damage_overlaps() {
    let scale = 1.0;
    let mut e = boot(true, scale);
    let _ = click(&mut e, "t");
    let box_css = e.test_element_box("t").unwrap();
    let _ = e.paint(vp(scale)).unwrap();

    let _ = key_press(&mut e, "ArrowLeft", false, false);
    let p = e.paint(vp(scale)).unwrap();
    assert!(
        !p.skipped && damage_overlaps_box(&p.damage, box_css, scale),
        "arrow move must damage the field: {:?}",
        p.damage
    );

    let _ = key_press(&mut e, "Backspace", false, false);
    let p = e.paint(vp(scale)).unwrap();
    assert!(
        !p.skipped && damage_overlaps_box(&p.damage, box_css, scale),
        "Backspace must damage the field"
    );

    let _ = e.input(InputEvent::Text("z".into()));
    let _ = click(&mut e, "other"); // commit/blur
    let p = e.paint(vp(scale)).unwrap();
    assert!(!p.skipped, "blur/commit must paint (focus ring clearing)");
}

// ---------------------------------------------------------------------------
// Probe19 parity matrix (click-driven; paint each step like a host frame loop)
// ---------------------------------------------------------------------------

enum St {
    Click(&'static str),
    ClickAt(&'static str, f32),
    Text(&'static str),
    Key(&'static str, bool, bool),
    Swap(&'static str),
    Blink,
    FailPaint,
}
use St::*;

fn run(e: &mut Engine, s: f64, steps: &[St], paint_each: bool) {
    for st in steps {
        match st {
            Click(id) => {
                let _ = click(e, id);
            }
            ClickAt(id, fx) => {
                let b = e.test_element_box(id).unwrap();
                let _ = ptr(e, b.0 + b.2 * fx, b.1 + b.3 * 0.5);
            }
            Text(t) => {
                let _ = e.input(InputEvent::Text((*t).into()));
            }
            Key(k, shift, ctrl) => {
                let _ = key_press(e, k, *shift, *ctrl);
            }
            Swap(f) => {
                let sl = Slices {
                    v: HashMap::from([("ui".into(), 1)]),
                    acts: vec![],
                };
                let _ = e.sync(&sl, &mut Frag((*f).into())).unwrap();
            }
            Blink => e.test_force_caret_blink(),
            FailPaint => {
                e.test_fail_next_paint();
                let _ = e.paint(vp(s));
            }
        }
        if paint_each {
            let _ = e.paint(vp(s)).unwrap();
        }
    }
}

fn scenarios() -> Vec<(&'static str, Vec<St>)> {
    let long = "a value that is much wider than the field 0123456789";
    vec![
        ("focus_click", vec![Click("t")]),
        ("type_chars", vec![Click("t"), Text("abc")]),
        (
            "backspace",
            vec![
                Click("t"),
                Key("Backspace", false, false),
                Key("Backspace", false, false),
            ],
        ),
        (
            "delete_fwd",
            vec![
                Click("t"),
                Key("Home", false, false),
                Key("Delete", false, false),
            ],
        ),
        (
            "arrows",
            vec![
                Click("t"),
                Key("ArrowLeft", false, false),
                Key("ArrowLeft", false, false),
                Text("X"),
            ],
        ),
        (
            "shift_arrows",
            vec![
                Click("t"),
                Key("ArrowLeft", true, false),
                Key("ArrowLeft", true, false),
            ],
        ),
        ("select_all", vec![Click("t"), Key("a", false, true)]),
        (
            "num_clamp_blur",
            vec![
                Click("n"),
                Key("a", false, true),
                Text("99999"),
                Click("other"),
            ],
        ),
        (
            "focus_out_click",
            vec![Click("t"), Text("q"), Click("other")],
        ),
        (
            "tab_out",
            vec![Click("t"), Text("q"), Key("Tab", false, false)],
        ),
        (
            "esc_revert",
            vec![Click("t"), Text("zzz"), Key("Escape", false, false)],
        ),
        (
            "swap_replace_focused",
            vec![Click("s"), Text("x"), Swap(S_FIELD), Text("y")],
        ),
        (
            "swap_remove_focused",
            vec![
                Click("s"),
                Text("x"),
                Swap("<span id=\"gone\">removed</span>"),
                Text("y"),
            ],
        ),
        ("blink_off", vec![Click("t"), Text("ab"), Blink]),
        ("blink_off_on", vec![Click("t"), Text("ab"), Blink, Blink]),
        (
            "blink_off_selection",
            vec![Click("t"), Key("a", false, true), Blink],
        ),
        (
            "failpaint_blinkoff",
            vec![Click("t"), Blink, FailPaint, Blink, Text("k")],
        ),
        (
            "overflow_blur",
            vec![Click("t"), Text(long), Click("other")],
        ),
        ("edge_type", vec![ClickAt("edge", 0.2), Text("ww")]),
    ]
}

#[test]
fn probe19_partial_vs_full_matrix() {
    let mut bad = 0usize;
    let mut total = 0usize;
    for transformed in [false, true] {
        for scale in [1.0, 1.25, 2.0] {
            for (name, steps) in scenarios() {
                total += 1;
                let mut a = boot(transformed, scale);
                run(&mut a, scale, &steps, true);
                let pa = a.last_frame().expect("partial frame").clone();

                let mut b = boot(transformed, scale);
                run(&mut b, scale, &steps, false);
                b.mark_theme_or_stylesheet_changed();
                let pb = b.paint(vp(scale)).unwrap();
                let full = pb.rgba.as_ref().unwrap();
                let diffs = pa
                    .pixels
                    .iter()
                    .zip(full.pixels.iter())
                    .filter(|(x, y)| x != y)
                    .count();
                if diffs != 0 {
                    bad += 1;
                    eprintln!(
                        "PROBE19 FAIL transformed={transformed} scale={scale} {name} diffs={diffs}"
                    );
                }
            }
        }
    }
    assert_eq!(
        bad, 0,
        "{bad}/{total} probe19 scenario(s) differ from full repaint"
    );
}
