//! Issue #14: `change` trigger, checkbox/radio/select, `<output for>`,
//! label activation, multi-value args (A24 / A25 + mock 27).

use hx_native::*;

fn boot(html: &str, actions: &[&str]) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(actions);
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");
    eng.load_html(&wrapped).expect("load");
    let _ = eng.paint(Viewport {
        width: 400,
        height: 300,
        scale: 1.0,
        holes: vec![],
    });
    eng
}

fn click(eng: &mut Engine, x: f32, y: f32) -> InputOutcome {
    let _ = eng.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    eng.input(InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    })
}

fn key(eng: &mut Engine, key: &str, mods: Modifiers) -> InputOutcome {
    eng.input(InputEvent::Key(KeyEvent {
        key: key.into(),
        code: key.into(),
        mods,
        repeat: false,
        pressed: true,
    }))
}

fn action_change(out: &InputOutcome) -> Option<&[(String, String)]> {
    match out {
        InputOutcome::Action {
            trigger: TriggerInfo::Change,
            args,
            ..
        } => Some(args.as_slice()),
        _ => None,
    }
}

fn vp() -> Viewport {
    Viewport {
        width: 400,
        height: 300,
        scale: 1.0,
        holes: vec![],
    }
}

/// Mean RGB of the interior of a document-space box (inset 1px to skip borders).
fn mean_rgb(img: &RgbaImage, box_xywh: (f32, f32, f32, f32), scale: f64) -> (u32, u32, u32) {
    let (x, y, w, h) = box_xywh;
    let x0 = ((x + 1.0) as f64 * scale).ceil() as u32;
    let y0 = ((y + 1.0) as f64 * scale).ceil() as u32;
    let x1 = (((x + w - 1.0) as f64 * scale).floor() as u32).min(img.width);
    let y1 = (((y + h - 1.0) as f64 * scale).floor() as u32).min(img.height);
    assert!(x0 < x1 && y0 < y1, "empty sample box {box_xywh:?}");
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    let mut n = 0u64;
    for yy in y0..y1 {
        for xx in x0..x1 {
            let i = ((yy * img.width + xx) * 4) as usize;
            r += img.pixels[i] as u64;
            g += img.pixels[i + 1] as u64;
            b += img.pixels[i + 2] as u64;
            n += 1;
        }
    }
    ((r / n) as u32, (g / n) as u32, (b / n) as u32)
}

fn damage_covers(damage: &[Rect], box_xywh: (f32, f32, f32, f32), scale: f64) -> bool {
    let (x, y, w, h) = box_xywh;
    let bx0 = x as f64 * scale;
    let by0 = y as f64 * scale;
    let bx1 = (x + w) as f64 * scale;
    let by1 = (y + h) as f64 * scale;
    damage.iter().any(|r| {
        let rx0 = r.x as f64;
        let ry0 = r.y as f64;
        let rx1 = (r.x + r.width) as f64;
        let ry1 = (r.y + r.height) as f64;
        rx0 <= bx0 && ry0 <= by0 && rx1 >= bx1 && ry1 >= by1
    })
}

fn count_pixel_diffs(
    a: &RgbaImage,
    b: &RgbaImage,
    box_xywh: (f32, f32, f32, f32),
    scale: f64,
) -> usize {
    let (x, y, w, h) = box_xywh;
    let x0 = (x as f64 * scale).floor().max(0.0) as u32;
    let y0 = (y as f64 * scale).floor().max(0.0) as u32;
    let x1 = ((x + w) as f64 * scale).ceil() as u32;
    let y1 = ((y + h) as f64 * scale).ceil() as u32;
    let x1 = x1.min(a.width).min(b.width);
    let y1 = y1.min(a.height).min(b.height);
    let mut n = 0usize;
    for yy in y0..y1 {
        for xx in x0..x1 {
            let i = ((yy * a.width + xx) * 4) as usize;
            if a.pixels[i..i + 4] != b.pixels[i..i + 4] {
                n += 1;
            }
        }
    }
    n
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

fn key_up(eng: &mut Engine, key: &str) -> InputOutcome {
    eng.input(InputEvent::Key(KeyEvent {
        key: key.into(),
        code: key.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: false,
    }))
}

fn open_select_alt_down(eng: &mut Engine) {
    let _ = key(
        eng,
        "ArrowDown",
        Modifiers {
            alt: true,
            ..Default::default()
        },
    );
}

/// Paint after `run`, then force a full repaint of the same DOM state and
/// assert byte identity. Also asserts the first paint was not a full repaint
/// (so missing select/radio damage cannot hide behind a full fallback).
fn paint_partial_then_full_parity<F>(html: &str, actions: &[&str], run: F, label: &str)
where
    F: Fn(&mut Engine),
{
    let mut eng = boot(html, actions);
    run(&mut eng);
    let partial = eng.paint(vp()).unwrap();
    assert!(
        !partial.skipped && partial.rgba.is_some(),
        "{label}: expected a raster paint"
    );
    assert!(
        !eng.last_paint_stats().full_repaint,
        "{label}: expected partial path, stats={:?}",
        eng.last_paint_stats()
    );
    let partial_px = partial.rgba.unwrap();

    let mut eng_full = boot(html, actions);
    run(&mut eng_full);
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_partial_eq_full(&partial_px, full.rgba.as_ref().unwrap(), label);
}

#[test]
fn a24_change_rejects_filter_and_non_control() {
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["a.x"]);
    let err = eng
        .load_html(r#"<!DOCTYPE html><html><body>
<input id="r" type="range" name="v" value="1" min="0" max="10" hx-post="a.x" hx-trigger="change[shiftKey]" />
</body></html>"#)
        .expect_err("change with filter");
    assert!(matches!(err, EngineError::InvalidTrigger { .. }));

    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["a.x"]);
    let err = eng
        .load_html(
            r#"<!DOCTYPE html><html><body>
<div id="d" hx-post="a.x" hx-trigger="change">x</div>
</body></html>"#,
        )
        .expect_err("change on div");
    assert!(matches!(err, EngineError::InvalidTrigger { .. }));
}

#[test]
fn a24_range_release_fires_change_not_click() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="0" min="0" max="10" step="1"
         hx-post="set.vol" hx-trigger="change" style="width:200px;height:24px" />
  <output id="out" for="vol">0</output>
</form>"#;
    let mut eng = boot(html, &["set.vol"]);
    // Drag from left toward right, release on control.
    let _ = eng.input(InputEvent::PointerDown {
        x: 10.0,
        y: 12.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let _ = eng.input(InputEvent::PointerMove {
        x: 150.0,
        y: 12.0,
        mods: Modifiers::default(),
    });
    let out = eng.input(InputEvent::PointerUp {
        x: 150.0,
        y: 12.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let args = action_change(&out).expect("change Action on release");
    assert!(
        args_get(args, "volume").is_some(),
        "form args include volume, got {args:?}"
    );
    // Value should have moved from 0.
    let v = eng.attr("vol", "value").unwrap_or_default();
    assert_ne!(v, "0", "drag should change value");
    // Output updated locally.
    // (text_content via serialize / attr path — check output moved off "0")
    let html_now = eng.serialize_html().unwrap();
    assert!(
        !html_now.contains(">0</output>")
            || html_now.contains(&format!(">{v}</output>"))
            || html_now.contains('%'),
        "output should track value, html={html_now}"
    );
}

#[test]
fn a24_range_step_fires_change() {
    let html = r#"
<input id="spd" type="range" name="speed" value="0" min="0" max="4" step="1"
       hx-post="set.spd" hx-trigger="change" />
"#;
    let mut eng = boot(html, &["set.spd"]);
    eng.test_set_focus("spd");
    let out = key(&mut eng, "ArrowRight", Modifiers::default());
    let args = action_change(&out).expect("change on step");
    assert_eq!(args_get(args, "speed"), Some("1"));
    assert_eq!(eng.attr("spd", "value").as_deref(), Some("1"));
}

#[test]
fn a24_checkbox_toggle_and_radio_exclusive() {
    let html = r#"
<form id="f">
  <input id="c" type="checkbox" name="music" value="on" hx-post="set.music" hx-trigger="change" />
  <input id="r1" type="radio" name="ops" value="1" hx-post="set.ops" hx-trigger="change" />
  <input id="r2" type="radio" name="ops" value="2" checked="" hx-post="set.ops" hx-trigger="change" />
</form>"#;
    let mut eng = boot(html, &["set.music", "set.ops"]);
    eng.test_set_focus("c");
    let out = key(&mut eng, " ", Modifiers::default());
    action_change(&out).expect("checkbox change");
    // Space toggled it on.
    let out2 = activate_click_on_id(&mut eng, "r1");
    action_change(&out2).expect("radio change");
    // r1 selected → r2 not in form args when collecting from r1.
    let args = action_change(&out2).unwrap();
    assert_eq!(args_get(args, "ops"), Some("1"));
}

fn activate_click_on_id(eng: &mut Engine, id: &str) -> InputOutcome {
    // Focus then synthesize a primary click via hit on the focused control:
    // use pointer at a location that resolves after focus set — walk by
    // forcing focus and using document hit after a trivial paint.
    eng.test_set_focus(id);
    // Probe several points; Blitz layout places body children near the origin.
    for y in [8.0_f32, 16.0, 24.0, 32.0, 40.0, 48.0, 56.0, 64.0] {
        for x in [8.0_f32, 24.0, 40.0, 56.0, 80.0, 120.0] {
            let down = eng.input(InputEvent::PointerDown {
                x,
                y,
                button: PointerButton::Primary,
                mods: Modifiers::default(),
            });
            if matches!(down, InputOutcome::Press { id: Some(ref hit) } if hit == id) {
                return eng.input(InputEvent::PointerUp {
                    x,
                    y,
                    button: PointerButton::Primary,
                    mods: Modifiers::default(),
                });
            }
            let _ = eng.input(InputEvent::PointerUp {
                x,
                y,
                button: PointerButton::Primary,
                mods: Modifiers::default(),
            });
        }
    }
    // Fallback: Space-equivalent for checkable already covered; for radio use
    // a second Space path won't select another radio. Force via key after focus.
    panic!("could not hit #{id} with pointer probe");
}

#[test]
fn a24_disabled_refuses_drag_toggle_step() {
    let html = r#"
<input id="vol" type="range" name="v" value="3" min="0" max="10" disabled=""
       hx-post="set.v" hx-trigger="change" style="width:200px;height:24px" />
<input id="c" type="checkbox" name="c" value="on" disabled=""
       hx-post="set.c" hx-trigger="change" />
"#;
    let mut eng = boot(html, &["set.v", "set.c"]);
    let _ = eng.input(InputEvent::PointerDown {
        x: 10.0,
        y: 12.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let out = eng.input(InputEvent::PointerUp {
        x: 150.0,
        y: 12.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    assert!(
        !matches!(out, InputOutcome::Action { .. }),
        "disabled range must not fire change, got {out:?}"
    );
    assert_eq!(eng.attr("vol", "value").as_deref(), Some("3"));

    eng.test_set_focus("c");
    let out = key(&mut eng, " ", Modifiers::default());
    assert!(
        !matches!(out, InputOutcome::Action { .. }),
        "disabled checkbox must not toggle"
    );
}

#[test]
fn a24_form_args_on_change() {
    let html = r#"
<form id="f">
  <input type="hidden" name="carrier" value="telco" />
  <input id="vol" type="range" name="volume" value="0" min="0" max="4" step="1"
         hx-post="set.vol" hx-trigger="change" />
</form>"#;
    let mut eng = boot(html, &["set.vol"]);
    eng.test_set_focus("vol");
    let out = key(&mut eng, "ArrowUp", Modifiers::default());
    let args = action_change(&out).expect("change");
    assert_eq!(args_get(args, "volume"), Some("1"));
    assert_eq!(args_get(args, "carrier"), Some("telco"));
}

// ---------------------------------------------------------------------------
// A25 — select
// ---------------------------------------------------------------------------

#[test]
fn a25_select_open_pick_esc_outside_arrows() {
    let html = r#"
<form id="f">
  <select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
          style="width:120px;height:22px;display:block;margin:8px">
    <option value="en" selected="">English</option>
    <option value="de">Deutsch</option>
    <option value="fr">Français</option>
  </select>
</form>"#;
    let mut eng = boot(html, &["set.lang"]);

    // Closed: ArrowDown steps and fires change.
    eng.test_set_focus("lang");
    let out = key(&mut eng, "ArrowDown", Modifiers::default());
    let args = action_change(&out).expect("closed arrow change");
    assert_eq!(args_get(args, "language"), Some("de"));
    if let InputOutcome::Action { request, .. } = &out {
        eng.complete(*request, Ok(()));
    }

    // Esc while closed stays Unhandled.
    let esc = key(&mut eng, "Escape", Modifiers::default());
    assert!(
        matches!(esc, InputOutcome::Unhandled(ref k) if k.key == "Escape"),
        "Esc closed → Unhandled, got {esc:?}"
    );

    // Open via Alt+ArrowDown.
    let open = key(
        &mut eng,
        "ArrowDown",
        Modifiers {
            alt: true,
            ..Default::default()
        },
    );
    assert!(matches!(open, InputOutcome::None));

    // Esc while open is consumed (not Unhandled).
    let esc2 = key(&mut eng, "Escape", Modifiers::default());
    assert!(
        matches!(esc2, InputOutcome::None),
        "Esc open must be consumed, got {esc2:?}"
    );
    // Value unchanged after Esc cancel (still "de").
    eng.test_set_focus("lang");
    // Re-open and pick with Enter after ArrowDown highlight → "fr".
    let _ = key(
        &mut eng,
        "ArrowDown",
        Modifiers {
            alt: true,
            ..Default::default()
        },
    );
    let _ = key(&mut eng, "ArrowDown", Modifiers::default());
    let picked = key(&mut eng, "Enter", Modifiers::default());
    let args = action_change(&picked).expect("Enter pick");
    assert_eq!(args_get(args, "language"), Some("fr"));
}

#[test]
fn a25_select_outside_click_cancels() {
    let html = r#"
<select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
        style="width:120px;height:22px;display:block;margin:8px">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
</select>
<button id="other" style="margin-top:80px;display:block">x</button>
"#;
    let mut eng = boot(html, &["set.lang"]);
    eng.test_set_focus("lang");
    let _ = key(
        &mut eng,
        "ArrowDown",
        Modifiers {
            alt: true,
            ..Default::default()
        },
    );
    // Click far below the list.
    let out = click(&mut eng, 20.0, 250.0);
    assert!(
        !matches!(
            out,
            InputOutcome::Action {
                trigger: TriggerInfo::Change,
                ..
            }
        ),
        "outside click must not change"
    );
}

#[test]
fn a25_disabled_select_refuses_open() {
    let html = r#"
<select id="lang" name="language" disabled="" hx-post="set.lang" hx-trigger="change"
        style="width:120px;height:22px;display:block;margin:8px">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
</select>"#;
    let mut eng = boot(html, &["set.lang"]);
    eng.test_set_focus("lang");
    let out = key(&mut eng, "ArrowDown", Modifiers::default());
    assert!(
        !matches!(out, InputOutcome::Action { .. }),
        "disabled select must not step"
    );
}

// ---------------------------------------------------------------------------
// Label activation + :checked restyle + multi-value (mock 27)
// ---------------------------------------------------------------------------

#[test]
fn checked_restyle_paints_i_same_frame_with_damage_and_parity() {
    // Mock-27 pattern: invisible input, visible <i>, :checked sibling style.
    let html = r#"
<style>
.vh { position:absolute; opacity:0; width:0; height:0; pointer-events:none; }
.cb { display:inline-block; padding:8px; }
.cb i {
  display:inline-block; width:16px; height:16px;
  border:1px solid #000; background:#ffffff;
}
.cb input:checked + i { background:#000080; }
</style>
<form id="deals-form">
  <label class="cb" id="lbl-oh">
    <input id="st-oh" type="checkbox" class="vh" name="state" value="US-OH"
           hx-post="set.state" hx-trigger="change" />
    <i id="box-oh"></i>Ohio
  </label>
</form>"#;
    let mut eng = boot(html, &["set.state"]);
    let before_box = eng.test_element_box("box-oh").expect("box-oh layout");
    // boot() already painted once; use the cached frame (clean paint skips rgba).
    let before_img = eng.last_frame().expect("cached frame").clone();
    let rgb_before = mean_rgb(&before_img, before_box, 1.0);
    // Unchecked face is near white.
    assert!(
        rgb_before.0 > 200 && rgb_before.1 > 200 && rgb_before.2 > 200,
        "unchecked <i> should be white-ish, got {rgb_before:?}"
    );

    // Toggle without firing change so attribute writes cannot mask the probes.
    assert!(eng.test_toggle_checkbox("st-oh"));
    assert!(
        eng.test_node_has_snapshot("st-oh"),
        "toggle must call snapshot_node_state_only on the checkbox"
    );
    assert!(
        eng.test_ancestor_has_dirty_descendants("st-oh"),
        "toggle must call mark_ancestors_dirty on the checkbox"
    );

    // Same-frame paint after the toggle (no host round-trip for the restyle).
    let after = eng.paint(vp()).unwrap();
    assert!(
        !after.skipped && after.rgba.is_some(),
        "toggle must dirty paint"
    );
    // Must take the partial path — a forced full would hide missing invalidation.
    assert!(
        !eng.last_paint_stats().full_repaint,
        "checked restyle must use partial damage, stats={:?}",
        eng.last_paint_stats()
    );
    let after_img = after.rgba.as_ref().unwrap();
    let box_after = eng.test_element_box("box-oh").expect("box-oh after");
    let rgb_after = mean_rgb(after_img, box_after, 1.0);
    assert!(
        rgb_after.2 > rgb_after.0 + 40 && rgb_after.2 > rgb_after.1 + 40,
        ":checked <i> must go navy (blue-dominant), before={rgb_before:?} after={rgb_after:?}"
    );
    assert!(
        damage_covers(&after.damage, box_after, 1.0),
        "damage must cover <i> box {box_after:?}, damage={:?}",
        after.damage
    );
    // Pixel delta vs the pre-toggle frame must be non-zero in the <i> box
    // (fails if restyle never invalidated).
    assert!(
        count_pixel_diffs(&before_img, after_img, box_after, 1.0) > 0,
        "partial paint must change <i> pixels vs pre-toggle frame"
    );

    // Partial vs full byte-identical after the toggle.
    let mut eng_full = boot(html, &["set.state"]);
    assert!(eng_full.test_toggle_checkbox("st-oh"));
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_eq!(
        after_img.pixels,
        full.rgba.as_ref().unwrap().pixels,
        "partial paint after :checked toggle must match full repaint"
    );
}

#[test]
fn label_radio_seg_exclusive_change_once() {
    let html = r#"
<style>
.vh { position:absolute; opacity:0; width:0; height:0; pointer-events:none; }
.seg { display:inline-block; padding:6px 10px; margin:4px; border:1px solid #808080; }
.seg.on { background:#000080; color:#fff; }
</style>
<form id="deals-form">
  <label class="seg" id="lbl-1">
    <input id="ops-1" type="radio" class="vh" name="ops" value="1"
           hx-post="set.ops" hx-trigger="change" />1
  </label>
  <label class="seg on" id="lbl-2">
    <input id="ops-2" type="radio" class="vh" name="ops" value="2" checked=""
           hx-post="set.ops" hx-trigger="change" />2
  </label>
  <button id="snap" name="op" value="snap" hx-post="deals.snap">Snap</button>
</form>"#;
    let mut eng = boot(html, &["set.ops", "deals.snap"]);
    // Click label for value 1.
    let mut fired = None;
    for y in [8.0_f32, 14.0, 20.0, 26.0, 32.0] {
        for x in [8.0_f32, 16.0, 24.0, 32.0, 40.0] {
            let out = click(&mut eng, x, y);
            if matches!(
                out,
                InputOutcome::Action {
                    trigger: TriggerInfo::Change,
                    ..
                }
            ) {
                fired = Some(out);
                break;
            }
        }
        if fired.is_some() {
            break;
        }
    }
    let out = fired.expect("radio label 1 should fire change");
    let args = action_change(&out).unwrap();
    assert_eq!(args_get(args, "ops"), Some("1"));
    // Exactly one Action from that click (not double-toggle / double-fire).
    assert!(eng.take_outcomes().is_empty());
    eng.complete(
        match &out {
            InputOutcome::Action { request, .. } => *request,
            _ => unreachable!(),
        },
        Ok(()),
    );
    // Form snapshot: only the selected radio contributes (exclusivity).
    eng.test_set_focus("snap");
    let snap = key(&mut eng, "Enter", Modifiers::default());
    match snap {
        InputOutcome::Action { args, .. } => {
            assert_eq!(args_get_all(&args, "ops"), vec!["1"]);
        }
        other => panic!("expected snap Action, got {other:?}"),
    }
    // Already-selected radio Space must not re-fire change.
    eng.test_set_focus("ops-1");
    let again = key(&mut eng, " ", Modifiers::default());
    assert!(
        !matches!(again, InputOutcome::Action { .. }),
        "already-selected radio Space must not re-fire change"
    );
}

#[test]
fn label_for_id_activates_external_checkbox() {
    let html = r#"
<style>
.vh { position:absolute; opacity:0; width:0; height:0; pointer-events:none; }
</style>
<form id="f">
  <input id="ext-cb" type="checkbox" class="vh" name="flag" value="yes"
         hx-post="set.flag" hx-trigger="change" />
  <label id="lbl-ext" for="ext-cb" style="display:inline-block;padding:8px 12px;border:1px solid #000">
    Toggle me
  </label>
</form>"#;
    let mut eng = boot(html, &["set.flag"]);
    let mut fired = None;
    for y in [8.0_f32, 14.0, 20.0, 26.0, 32.0, 40.0] {
        for x in [8.0_f32, 20.0, 40.0, 60.0, 80.0, 100.0] {
            let out = click(&mut eng, x, y);
            if matches!(
                out,
                InputOutcome::Action {
                    trigger: TriggerInfo::Change,
                    ..
                }
            ) {
                fired = Some(out);
                break;
            }
        }
        if fired.is_some() {
            break;
        }
    }
    let out = fired.expect("label[for] click should toggle external checkbox");
    let args = action_change(&out).unwrap();
    assert_eq!(args_get(args, "flag"), Some("yes"));
}

#[test]
fn label_activates_vh_checkbox_once() {
    let html = r#"
<style>
.vh { position:absolute; opacity:0; width:0; height:0; pointer-events:none; }
.cb { display:inline-block; padding:4px 8px; }
.cb i { display:inline-block; width:12px; height:12px; border:1px solid #000; background:#fff; }
.cb input:checked + i { background:#000080; }
</style>
<form id="deals-form">
  <label class="cb" id="lbl-oh">
    <input id="st-oh" type="checkbox" class="vh" name="state" value="US-OH"
           hx-post="set.state" hx-trigger="change" />
    <i id="box-oh"></i>Ohio
  </label>
</form>"#;
    let mut eng = boot(html, &["set.state"]);
    // Click the visible <i> / label text area.
    let mut fired = None;
    for y in [10.0_f32, 14.0, 18.0, 22.0, 28.0] {
        for x in [10.0_f32, 20.0, 40.0, 60.0, 80.0] {
            let out = click(&mut eng, x, y);
            if matches!(
                out,
                InputOutcome::Action {
                    trigger: TriggerInfo::Change,
                    ..
                }
            ) {
                fired = Some(out);
                break;
            }
        }
        if fired.is_some() {
            break;
        }
    }
    let out = fired.expect("label click should toggle checkbox and fire change");
    let args = action_change(&out).unwrap();
    assert_eq!(args_get(args, "state"), Some("US-OH"));
    // Complete so a second change is not dropped by hx-sync=drop.
    if let InputOutcome::Action { request, .. } = &out {
        eng.complete(*request, Ok(()));
    }
    // Second click unchecks — change still fires, state absent from form args
    // when unchecked (trigger still contributes its value attr).
    let mut second = None;
    for y in [10.0_f32, 14.0, 18.0, 22.0, 28.0] {
        for x in [10.0_f32, 20.0, 40.0, 60.0, 80.0] {
            let out = click(&mut eng, x, y);
            if matches!(
                out,
                InputOutcome::Action {
                    trigger: TriggerInfo::Change,
                    ..
                }
            ) {
                second = Some(out);
                break;
            }
        }
        if second.is_some() {
            break;
        }
    }
    second.expect("second label click toggles off");
}

#[test]
fn multi_value_checkboxes_and_args_get_all() {
    let html = r#"
<form id="deals-form">
  <input id="s1" type="checkbox" name="state" value="US-OH" checked="" />
  <input id="s2" type="checkbox" name="state" value="US-PA" checked="" />
  <input id="s3" type="checkbox" name="state" value="US-NY" />
  <input type="hidden" name="carrier" value="telco_usa_2" />
  <button id="go" name="op" value="submit" hx-post="deals.submit">Go</button>
</form>"#;
    let mut eng = boot(html, &["deals.submit"]);
    eng.test_set_focus("go");
    let out = key(&mut eng, "Enter", Modifiers::default());
    match out {
        InputOutcome::Action { args, .. } => {
            assert_eq!(args_get(&args, "state"), Some("US-OH"));
            assert_eq!(args_get_all(&args, "state"), vec!["US-OH", "US-PA"]);
            assert_eq!(args_get(&args, "carrier"), Some("telco_usa_2"));
            assert_eq!(args_get(&args, "op"), Some("submit"));
        }
        other => panic!("expected Action, got {other:?}"),
    }
}

#[test]
fn trigger_name_value_with_form_same_name() {
    // Mock 27: row `name=carrier value=uunet` inside a form that also has
    // a hidden `carrier`. Trigger value first, then the form field (htmx).
    let html = r#"
<form id="deals-form">
  <input type="hidden" name="carrier" value="telco_usa_2" />
  <table>
    <tr id="deals-co-uunet" name="carrier" value="uunet"
        hx-post="deals.company.select" hx-trigger="click"
        style="display:block;height:24px;width:200px">
      <td>UUNET</td>
    </tr>
  </table>
</form>"#;
    let mut eng = boot(html, &["deals.company.select"]);
    // Click the row.
    let mut hit = None;
    for y in [30.0_f32, 40.0, 50.0, 60.0, 70.0, 80.0] {
        for x in [20.0_f32, 40.0, 80.0, 120.0] {
            let out = click(&mut eng, x, y);
            if matches!(
                out,
                InputOutcome::Action {
                    name: ref n,
                    ..
                } if n == "deals.company.select"
            ) {
                hit = Some(out);
                break;
            }
        }
        if hit.is_some() {
            break;
        }
    }
    // If pointer miss (tr outside form layout), activate via focus+Enter-like click path.
    let out = hit.unwrap_or_else(|| {
        eng.test_set_focus("deals-co-uunet");
        // Primary click event matching: use activate-like via key Enter on hx-post.
        key(&mut eng, "Enter", Modifiers::default())
    });
    match out {
        InputOutcome::Action { args, .. } => {
            // Trigger first, then form field — both values for "carrier".
            let all = args_get_all(&args, "carrier");
            assert_eq!(
                all.first().copied(),
                Some("uunet"),
                "trigger value first, got {args:?}"
            );
            assert!(
                all.contains(&"telco_usa_2"),
                "form carrier also present, got {args:?}"
            );
            assert_eq!(args_get(&args, "carrier"), Some("uunet"));
        }
        other => panic!("expected Action, got {other:?}"),
    }
}

#[test]
fn output_for_updates_on_step() {
    let html = r#"
<input id="vol" type="range" name="volume" value="0.55" min="0" max="1" step="0.05"
       hx-post="set.vol" hx-trigger="change" />
<output id="out" for="vol">55%</output>
"#;
    let mut eng = boot(html, &["set.vol"]);
    eng.test_set_focus("vol");
    let _ = key(&mut eng, "ArrowRight", Modifiers::default());
    let html_now = eng.serialize_html().unwrap();
    assert!(
        html_now.contains("60%") || html_now.contains(">0.6</output>"),
        "output should update, got {html_now}"
    );
}

// ---------------------------------------------------------------------------
// Graph HOLD follow-up: select damage, plain radio uncheck, reconcile
// ---------------------------------------------------------------------------

fn select_over_button_html() -> &'static str {
    r#"
<button id="under" style="position:absolute;left:8px;top:40px;width:160px;height:120px;background:#c0c0c0">under</button>
<select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
        style="position:absolute;left:8px;top:8px;width:128px;height:22px">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
  <option value="fr">Français</option>
  <option value="es">Español</option>
</select>
"#
}

#[test]
fn select_open_partial_parity() {
    paint_partial_then_full_parity(
        select_over_button_html(),
        &["set.lang"],
        |eng| {
            eng.test_set_focus("lang");
            open_select_alt_down(eng);
        },
        "select open",
    );
}

#[test]
fn select_hover_partial_parity() {
    paint_partial_then_full_parity(
        select_over_button_html(),
        &["set.lang"],
        |eng| {
            eng.test_set_focus("lang");
            open_select_alt_down(eng);
            let _ = eng.paint(vp()).unwrap();
            // Move onto the second list row (below the 22px control).
            let _ = eng.input(InputEvent::PointerMove {
                x: 40.0,
                y: 8.0 + 22.0 + 18.0 + 4.0,
                mods: Modifiers::default(),
            });
        },
        "select hover",
    );
}

#[test]
fn select_mouse_pick_partial_parity() {
    paint_partial_then_full_parity(
        select_over_button_html(),
        &["set.lang"],
        |eng| {
            eng.test_set_focus("lang");
            open_select_alt_down(eng);
            let _ = eng.paint(vp()).unwrap();
            // Click second row (Deutsch).
            let y = 8.0 + 22.0 + 18.0 + 4.0;
            let _ = click(eng, 40.0, y);
        },
        "select mouse pick",
    );
}

#[test]
fn select_mouse_pick_over_under_fires_change_not_click() {
    // Row 2 lies over #under; the pick must fire Change, not Clicked{under}.
    let mut eng = boot(select_over_button_html(), &["set.lang"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();
    let y = 8.0 + 22.0 + 18.0 + 4.0;
    let out = click(&mut eng, 40.0, y);
    let args = action_change(&out)
        .unwrap_or_else(|| panic!("pick over #under must fire Change, got {out:?}"));
    assert_eq!(args_get(args, "language"), Some("de"));
    assert!(
        !matches!(out, InputOutcome::Clicked { .. }),
        "must not click the element under the list, got {out:?}"
    );
}

#[test]
fn select_enter_pick_partial_parity() {
    paint_partial_then_full_parity(
        select_over_button_html(),
        &["set.lang"],
        |eng| {
            eng.test_set_focus("lang");
            open_select_alt_down(eng);
            let _ = eng.paint(vp()).unwrap();
            let _ = key(eng, "ArrowDown", Modifiers::default());
            let _ = key(eng, "Enter", Modifiers::default());
        },
        "select Enter pick",
    );
}

#[test]
fn select_esc_close_partial_parity() {
    paint_partial_then_full_parity(
        select_over_button_html(),
        &["set.lang"],
        |eng| {
            eng.test_set_focus("lang");
            open_select_alt_down(eng);
            let _ = eng.paint(vp()).unwrap();
            let _ = key(eng, "Escape", Modifiers::default());
        },
        "select Esc close",
    );
}

#[test]
fn select_outside_click_close_partial_parity() {
    paint_partial_then_full_parity(
        select_over_button_html(),
        &["set.lang"],
        |eng| {
            eng.test_set_focus("lang");
            open_select_alt_down(eng);
            let _ = eng.paint(vp()).unwrap();
            let _ = click(eng, 300.0, 250.0);
        },
        "select outside-click close",
    );
}

#[test]
fn select_overflow_bottom_pick_partial_parity() {
    // Select near the bottom so the open list overflows the 300px surface;
    // a button sits under the clipped list area.
    let html = r#"
<button id="under" style="position:absolute;left:8px;top:250px;width:180px;height:40px;background:#808080">under</button>
<select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
        style="position:absolute;left:8px;top:250px;width:128px;height:22px">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
  <option value="fr">Français</option>
  <option value="es">Español</option>
  <option value="it">Italiano</option>
  <option value="pt">Português</option>
  <option value="nl">Nederlands</option>
  <option value="sv">Svenska</option>
</select>
"#;
    paint_partial_then_full_parity(
        html,
        &["set.lang"],
        |eng| {
            eng.test_set_focus("lang");
            open_select_alt_down(eng);
            let open_paint = eng.paint(vp()).unwrap();
            assert!(!open_paint.skipped, "open overflowing list must paint");
            // Pick first visible row with Enter (highlight starts on selected).
            let _ = key(eng, "Enter", Modifiers::default());
            // After close, the next paint must not skip (pending list damage).
        },
        "select overflow-bottom pick",
    );
    // Explicit skip check: after pick, paint once (parity helper did), then a
    // clean paint may skip — but the pick paint itself must have run.
    let mut eng = boot(html, &["set.lang"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();
    let _ = key(&mut eng, "Enter", Modifiers::default());
    let after = eng.paint(vp()).unwrap();
    assert!(
        !after.skipped,
        "close after overflow must not skip the clearing paint"
    );
}

#[test]
fn select_esc_keyup_consumed_after_open_close() {
    let mut eng = boot(select_over_button_html(), &["set.lang"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let down = key(&mut eng, "Escape", Modifiers::default());
    assert!(matches!(down, InputOutcome::None));
    let up = key_up(&mut eng, "Escape");
    assert!(
        matches!(up, InputOutcome::None),
        "Esc key-up after open close must be consumed, got {up:?}"
    );
}

#[test]
fn plain_native_radio_uncheck_pixel_parity() {
    // No sibling <i> — Blitz paints the native radio widget. Switching must
    // clear the previous radio's dot on the partial path.
    let html = r#"
<form id="f">
  <input id="r1" type="radio" name="ops" value="1" checked=""
         hx-post="set.ops" hx-trigger="change"
         style="width:20px;height:20px;margin:8px" />
  <input id="r2" type="radio" name="ops" value="2"
         hx-post="set.ops" hx-trigger="change"
         style="width:20px;height:20px;margin:8px" />
</form>
"#;
    let mut eng = boot(html, &["set.ops"]);
    let r1_box = eng.test_element_box("r1").expect("r1 box");
    let before = eng.last_frame().expect("frame").clone();
    let rgb_r1_before = mean_rgb(&before, r1_box, 1.0);

    // Click r2 (scan near its expected layout).
    let r2_box = eng.test_element_box("r2").expect("r2 box");
    let cx = r2_box.0 + r2_box.2 * 0.5;
    let cy = r2_box.1 + r2_box.3 * 0.5;
    let out = click(&mut eng, cx, cy);
    assert!(action_change(&out).is_some(), "r2 click fires change");

    let after = eng.paint(vp()).unwrap();
    assert!(!after.skipped);
    assert!(
        !eng.last_paint_stats().full_repaint,
        "radio switch must be partial, stats={:?}",
        eng.last_paint_stats()
    );
    let after_img = after.rgba.as_ref().unwrap();
    let r1_after = eng.test_element_box("r1").expect("r1 after");
    // r1's pixels must change (dot cleared) — catches missing previous-box damage.
    let diffs = count_pixel_diffs(&before, after_img, r1_after, 1.0);
    assert!(
        diffs > 0,
        "un-checking r1 must change its pixels (got 0 diffs); before_rgb={rgb_r1_before:?}"
    );

    let mut eng_full = boot(html, &["set.ops"]);
    let r2b = eng_full.test_element_box("r2").unwrap();
    let _ = click(&mut eng_full, r2b.0 + r2b.2 * 0.5, r2b.1 + r2b.3 * 0.5);
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_partial_eq_full(
        after_img,
        full.rgba.as_ref().unwrap(),
        "plain radio uncheck",
    );
}

#[test]
fn plain_native_radio_space_uncheck_pixel_parity() {
    let html = r#"
<form id="f">
  <input id="r1" type="radio" name="ops" value="1" checked=""
         hx-post="set.ops" hx-trigger="change"
         style="width:20px;height:20px;margin:8px" />
  <input id="r2" type="radio" name="ops" value="2"
         hx-post="set.ops" hx-trigger="change"
         style="width:20px;height:20px;margin:8px" />
</form>
"#;
    let mut eng = boot(html, &["set.ops"]);
    let before = eng.last_frame().expect("frame").clone();
    eng.test_set_focus("r2");
    let out = key(&mut eng, " ", Modifiers::default());
    assert!(action_change(&out).is_some());
    let after = eng.paint(vp()).unwrap();
    assert!(!eng.last_paint_stats().full_repaint);
    let r1 = eng.test_element_box("r1").unwrap();
    assert!(
        count_pixel_diffs(&before, after.rgba.as_ref().unwrap(), r1, 1.0) > 0,
        "Space on r2 must clear r1 pixels"
    );
}

#[test]
fn open_select_closes_on_swap_replace() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct MemSlices {
        versions: HashMap<String, u64>,
    }
    impl SliceSource for MemSlices {
        fn version(&self, name: &str) -> Option<u64> {
            self.versions.get(name).copied()
        }
        fn action_states(&self) -> Vec<(String, ActionState)> {
            Vec::new()
        }
    }
    struct Frags {
        body: String,
        calls: Arc<Mutex<u32>>,
    }
    impl FragmentSource for Frags {
        fn render(&mut self, _id: &str, _slices: &[&str]) -> String {
            *self.calls.lock().unwrap() += 1;
            self.body.clone()
        }
    }

    let html = r##"
<section id="panel" hx-trigger="slice:ui" hx-target="#panel" hx-swap="innerHTML">
  <select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
          style="width:120px;height:22px;display:block;margin:8px">
    <option value="en" selected="">English</option>
    <option value="de">Deutsch</option>
  </select>
  <button id="other">x</button>
</section>
"##;
    let mut eng = boot(html, &["set.lang"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();

    let slices = MemSlices {
        versions: HashMap::from([("ui".into(), 1), ("actions".into(), 1)]),
    };
    let mut frags = Frags {
        body: r#"<button id="other">replaced</button>"#.into(),
        calls: Arc::new(Mutex::new(0)),
    };
    // Initial sync replaces the open <select>; reconcile must close without a
    // focus change (do not call test_set_focus here — that would mask a missing
    // swap/reindex reconcile).
    let _ = eng.sync(&slices, &mut frags).unwrap();

    // Arrows must not be swallowed by a phantom open list.
    let out = key(&mut eng, "ArrowDown", Modifiers::default());
    assert!(
        !matches!(out, InputOutcome::None),
        "after swap without focus change, ArrowDown must not be consumed by a stale open select; got {out:?}"
    );
    // Esc while closed stays Unhandled.
    let esc = key(&mut eng, "Escape", Modifiers::default());
    assert!(
        matches!(esc, InputOutcome::Unhandled(ref k) if k.key == "Escape"),
        "Esc after swap-close must be Unhandled, got {esc:?}"
    );

    // Close-via-swap pending damage must clear the list at 0 px vs full.
    let mut eng = boot(html, &["set.lang"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();
    let slices = MemSlices {
        versions: HashMap::from([("ui".into(), 1), ("actions".into(), 1)]),
    };
    let mut frags = Frags {
        body: r#"<button id="other">replaced</button>"#.into(),
        calls: Arc::new(Mutex::new(0)),
    };
    let _ = eng.sync(&slices, &mut frags).unwrap();
    let partial = eng.paint(vp()).unwrap();
    assert!(!partial.skipped, "swap-close must paint");
    assert!(
        !eng.last_paint_stats().full_repaint,
        "swap-close must use pending damage, not full; stats={:?}",
        eng.last_paint_stats()
    );
    let mut eng_full = boot(html, &["set.lang"]);
    eng_full.test_set_focus("lang");
    open_select_alt_down(&mut eng_full);
    let _ = eng_full.paint(vp()).unwrap();
    let slices = MemSlices {
        versions: HashMap::from([("ui".into(), 1), ("actions".into(), 1)]),
    };
    let mut frags = Frags {
        body: r#"<button id="other">replaced</button>"#.into(),
        calls: Arc::new(Mutex::new(0)),
    };
    let _ = eng_full.sync(&slices, &mut frags).unwrap();
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_partial_eq_full(
        partial.rgba.as_ref().unwrap(),
        full.rgba.as_ref().unwrap(),
        "select swap-close",
    );
}

#[test]
fn open_select_closes_when_disabled() {
    use std::collections::HashMap;

    struct MemSlices {
        versions: HashMap<String, u64>,
        actions: Vec<(String, ActionState)>,
    }
    impl SliceSource for MemSlices {
        fn version(&self, name: &str) -> Option<u64> {
            self.versions.get(name).copied()
        }
        fn action_states(&self) -> Vec<(String, ActionState)> {
            self.actions.clone()
        }
    }
    struct EmptyFrags;
    impl FragmentSource for EmptyFrags {
        fn render(&mut self, _id: &str, _slices: &[&str]) -> String {
            String::new()
        }
    }

    let html = r#"
<select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
        style="width:120px;height:22px;display:block;margin:8px">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
</select>
<button id="other">x</button>
"#;
    let mut eng = boot(html, &["set.lang"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();

    let mut slices = MemSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: vec![(
            "set.lang".into(),
            ActionState::Disabled {
                reason: "locked".into(),
            },
        )],
    };
    let mut frags = EmptyFrags;
    slices.versions.insert("actions".into(), 2);
    let _ = eng.sync(&slices, &mut frags).unwrap();

    let esc = key(&mut eng, "Escape", Modifiers::default());
    assert!(
        matches!(esc, InputOutcome::Unhandled(ref k) if k.key == "Escape"),
        "Esc after disable-close must be Unhandled, got {esc:?}"
    );
    let arrow = key(&mut eng, "ArrowDown", Modifiers::default());
    assert!(
        !matches!(arrow, InputOutcome::None)
            || matches!(arrow, InputOutcome::Unhandled(_))
            || matches!(arrow, InputOutcome::Action { .. }),
        "ArrowDown must not be swallowed by a disabled open list; got {arrow:?}"
    );

    // Close-via-disable pending damage must clear the list at 0 px vs full.
    let mut eng = boot(html, &["set.lang"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();
    let mut slices = MemSlices {
        versions: HashMap::from([("actions".into(), 2)]),
        actions: vec![(
            "set.lang".into(),
            ActionState::Disabled {
                reason: "locked".into(),
            },
        )],
    };
    let mut frags = EmptyFrags;
    let _ = eng.sync(&slices, &mut frags).unwrap();
    let partial = eng.paint(vp()).unwrap();
    assert!(!partial.skipped, "disable-close must paint");
    assert!(
        !eng.last_paint_stats().full_repaint,
        "disable-close must use pending damage, not full; stats={:?}",
        eng.last_paint_stats()
    );
    let mut eng_full = boot(html, &["set.lang"]);
    eng_full.test_set_focus("lang");
    open_select_alt_down(&mut eng_full);
    let _ = eng_full.paint(vp()).unwrap();
    slices.versions.insert("actions".into(), 2);
    let mut frags = EmptyFrags;
    let _ = eng_full.sync(&slices, &mut frags).unwrap();
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_partial_eq_full(
        partial.rgba.as_ref().unwrap(),
        full.rgba.as_ref().unwrap(),
        "select disable-close",
    );
}

#[test]
fn open_select_closes_on_focus_change() {
    let html = r#"
<select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
        style="width:120px;height:22px;display:block;margin:8px">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
</select>
<button id="other" hx-post="noop">x</button>
"#;
    let mut eng = boot(html, &["set.lang", "noop"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();

    // Tab away (or direct focus) must close the list.
    eng.test_set_focus("other");
    let esc = key(&mut eng, "Escape", Modifiers::default());
    assert!(
        matches!(esc, InputOutcome::Unhandled(ref k) if k.key == "Escape"),
        "Esc after focus-away close must be Unhandled, got {esc:?}"
    );
    // Enter on the button must work (not swallowed by open select).
    let out = key(&mut eng, "Enter", Modifiers::default());
    assert!(
        matches!(
            out,
            InputOutcome::Action {
                name: ref n,
                ..
            } if n == "noop"
        ),
        "Enter after focus-away must activate button, got {out:?}"
    );

    // Close-via-focus-loss pending damage must clear the list at 0 px vs full.
    let mut eng = boot(html, &["set.lang", "noop"]);
    eng.test_set_focus("lang");
    open_select_alt_down(&mut eng);
    let _ = eng.paint(vp()).unwrap();
    eng.test_set_focus("other");
    let partial = eng.paint(vp()).unwrap();
    assert!(!partial.skipped, "focus-away close must paint");
    assert!(
        !eng.last_paint_stats().full_repaint,
        "focus-away close must use pending damage, not full; stats={:?}",
        eng.last_paint_stats()
    );
    let mut eng_full = boot(html, &["set.lang", "noop"]);
    eng_full.test_set_focus("lang");
    open_select_alt_down(&mut eng_full);
    let _ = eng_full.paint(vp()).unwrap();
    eng_full.test_set_focus("other");
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_partial_eq_full(
        partial.rgba.as_ref().unwrap(),
        full.rgba.as_ref().unwrap(),
        "select focus-away close",
    );
}

// ---------------------------------------------------------------------------
// Transform-aware open select (Pause-style translate(-50%,-50%) panel)
// ---------------------------------------------------------------------------

fn select_in_translated_panel_html() -> &'static str {
    r#"
<div id="panel" style="position:absolute;left:50%;top:50%;width:200px;height:100px;
     transform:translate(-50%,-50%);background:#c0c0c0">
  <select id="lang" name="language" hx-post="set.lang" hx-trigger="change"
          style="width:160px;height:22px;display:block;margin:12px">
    <option value="en" selected="">English</option>
    <option value="de">Deutsch</option>
    <option value="fr">Français</option>
  </select>
</div>
"#
}

fn vp_at(scale: f64) -> Viewport {
    Viewport {
        width: (400.0 * scale).round() as u32,
        height: (300.0 * scale).round() as u32,
        scale,
        holes: vec![],
    }
}

fn boot_at(html: &str, actions: &[&str], scale: f64) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(actions);
    let wrapped = format!("<!DOCTYPE html><html><body style=\"margin:0;width:400px;height:300px\">{html}</body></html>");
    eng.load_html(&wrapped).expect("load");
    let _ = eng.paint(vp_at(scale)).unwrap();
    eng
}

/// Open list placement + click targets must follow transform-aware geometry
/// at scales 1 / 1.25 / 2. Mutation (absolute_position in control_rect_of)
/// makes this fail — see agent verification notes.
#[test]
fn select_open_in_translated_panel_parity_and_clicks() {
    const ROW_H: f32 = 18.0;
    for scale in [1.0_f64, 1.25, 2.0] {
        let html = select_in_translated_panel_html();
        // --- pixel parity: open list partial vs forced full ---
        let mut eng = boot_at(html, &["set.lang"], scale);
        eng.test_set_focus("lang");
        open_select_alt_down(&mut eng);
        let partial = eng.paint(vp_at(scale)).unwrap();
        assert!(
            !partial.skipped && partial.rgba.is_some(),
            "scale={scale}: open must paint"
        );
        assert!(
            !eng.last_paint_stats().full_repaint,
            "scale={scale}: open must be partial, stats={:?}",
            eng.last_paint_stats()
        );

        let mut eng_full = boot_at(html, &["set.lang"], scale);
        eng_full.test_set_focus("lang");
        open_select_alt_down(&mut eng_full);
        eng_full.mark_theme_or_stylesheet_changed();
        let full = eng_full.paint(vp_at(scale)).unwrap();
        assert_partial_eq_full(
            partial.rgba.as_ref().unwrap(),
            full.rgba.as_ref().unwrap(),
            &format!("select open in translated panel scale={scale}"),
        );

        // --- click targets: pick row 1 (Deutsch) using transform-aware box ---
        let mut eng = boot_at(html, &["set.lang"], scale);
        eng.test_set_focus("lang");
        open_select_alt_down(&mut eng);
        let _ = eng.paint(vp_at(scale)).unwrap();
        let (cx, cy, cw, ch) = eng
            .test_element_box("lang")
            .expect("lang box after transform");
        // List sits directly below the control; row 1 center.
        let click_x = cx + cw * 0.5;
        let click_y = cy + ch + 1.0 + ROW_H * 1.5;
        // Sanity: transform must have moved the control off the untransformed
        // top-left (margin-only) spot — otherwise the helper is unused.
        assert!(
            cx > 50.0 && cy > 50.0,
            "scale={scale}: expected centered select box, got ({cx},{cy},{cw},{ch})"
        );
        let out = click(&mut eng, click_x, click_y);
        let args = action_change(&out).unwrap_or_else(|| {
            panic!("scale={scale}: click at ({click_x},{click_y}) on translated list must fire Change, got {out:?}; box=({cx},{cy},{cw},{ch})")
        });
        assert_eq!(
            args_get(args, "language"),
            Some("de"),
            "scale={scale}: row 1 click must select Deutsch"
        );
    }
}
