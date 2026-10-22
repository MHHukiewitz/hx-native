//! Issue #35: engine-drawn range slider, plus the typed number field's
//! `text-align` and four-sided `--field` bevel.
//!
//! Blitz beta.4 paints no track or thumb for `<input type=range>` and drops
//! `::-webkit-slider-*`; the engine draws a sunken groove and a raised thumb
//! whose centre sits on the value. Every colour comes from the crate-owned
//! `--hx-*` variables; [`HOST_MAP`] is the Win95 mapping a host such as
//! Mainnet supplies in its own CSS (the engine itself has neutral defaults). Range and number stay in sync
//! through the host (a `change` re-renders both), never inside the engine.

use std::collections::HashMap;

use hx_native::*;

const ACTIONS: &[&str] = &["set.d", "settings.set"];

const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const FACE: [u8; 4] = [0xC0, 0xC0, 0xC0, 0xFF];
const SH: [u8; 4] = [0x80, 0x80, 0x80, 0xFF];
/// `--hx-bevel-dk` in [`HOST_MAP`].
const DK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];
const PANEL: [u8; 4] = [0xDF, 0xDF, 0xDF, 0xFF];
/// The `FIELD` bevel's own dark colour (box-shadow literal, not a token).
const FIELD_DK: [u8; 4] = [0x0A, 0x0A, 0x0A, 0xFF];
const INK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];

/// The host's `--hx-*` mapping (Win95 look; lives in host CSS, never in the
/// engine). Every [`page`] carries it; [`page_bare`] does not.
const HOST_MAP: &str = ":root{--hx-face:#c0c0c0;--hx-ink:#000;--hx-bevel-hi:#fff;--hx-bevel-lt:#dfdfdf;--hx-bevel-sh:#808080;--hx-bevel-dk:#000;--hx-track:#c0c0c0;--hx-track-alt:#fff;--hx-select-bg:#fff;--hx-select-ink:#000;--hx-select-hl:#000080}";

const FIELD: &str = "box-shadow:inset -1px -1px #fff,inset 1px 1px #808080,inset -2px -2px #dfdfdf,inset 2px 2px #0a0a0a";

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    }
}

fn vp() -> Viewport {
    Viewport::with_scale(400, 200, 1.0)
}

/// Page with a white body; `body` is raw markup. Sliders `.trk` are 111 px
/// wide (100 px of thumb travel) at x = 20.
fn page(css: &str, body: &str) -> String {
    page_bare(&format!("{HOST_MAP}\n{css}"), body)
}

/// [`page`] without the host's `--hx-*` mapping: the engine defaults show.
fn page_bare(css: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{width:400px;height:200px;overflow:hidden;background:#fff;font-family:"DejaVu Sans",sans-serif;font-size:11px;color:#000}}
.trk{{position:absolute;left:20px;width:111px;height:20px;appearance:none;-webkit-appearance:none;background:transparent}}
.trk::-webkit-slider-runnable-track{{height:4px;background:red}}
.trk::-webkit-slider-thumb{{width:40px;background:red}}
{css}
</style></head><body>{body}</body></html>"#
    )
}

fn boot(html: &str) -> Engine {
    let mut eng = Engine::new(cfg());
    eng.declare_actions(ACTIONS);
    eng.load_html(html).expect("load");
    eng.paint(vp()).expect("first paint");
    eng
}

fn down(x: f32, y: f32) -> InputEvent {
    InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    }
}
fn up(x: f32, y: f32) -> InputEvent {
    InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    }
}
fn mv(x: f32, y: f32) -> InputEvent {
    InputEvent::PointerMove {
        x,
        y,
        mods: Modifiers::default(),
    }
}
fn key(k: &str) -> InputEvent {
    key_mods(k, Modifiers::default())
}
fn key_mods(k: &str, mods: Modifiers) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods,
        repeat: false,
        pressed: true,
    })
}

type R = (f32, f32, f32, f32);

fn slider(eng: &Engine, id: &str) -> (R, R, R, bool) {
    eng.test_slider(id).expect(id)
}

fn thumb_centre(eng: &Engine, id: &str) -> (f32, f32) {
    let t = slider(eng, id).2;
    (t.0 + t.2 / 2.0, t.1 + t.3 / 2.0)
}

fn value(eng: &Engine, id: &str) -> String {
    eng.attr(id, "value").unwrap_or_default()
}

fn px(img: &RgbaImage, x: f32, y: f32) -> [u8; 4] {
    let i = ((y as u32 * img.width + x as u32) * 4) as usize;
    [
        img.pixels[i],
        img.pixels[i + 1],
        img.pixels[i + 2],
        img.pixels[i + 3],
    ]
}

fn frame(eng: &mut Engine) -> RgbaImage {
    let _ = eng.paint(vp()).expect("paint");
    eng.last_frame().expect("frame").clone()
}

fn full_frame(eng: &mut Engine) -> RgbaImage {
    eng.mark_theme_or_stylesheet_changed();
    eng.paint(vp()).unwrap().rgba.expect("full rgba")
}

type ActionView<'a> = (&'a str, &'a [(String, String)], &'a str);

/// The host finished the action's request (so the next `change` is not
/// held behind it).
fn settle(eng: &mut Engine, out: &InputOutcome) {
    if let InputOutcome::Action { request, .. } = out {
        eng.complete(*request, Ok(()));
    }
}

fn action(out: &InputOutcome) -> Option<ActionView<'_>> {
    match out {
        InputOutcome::Action {
            name,
            args,
            source_id,
            ..
        } => Some((name.as_str(), args.as_slice(), source_id.as_str())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------

#[test]
fn thumb_position_from_value_at_0_50_100_percent() {
    let mut eng = boot(&page(
        "#r0{top:10px}#r50{top:40px}#r100{top:70px}",
        r#"<input id="r0" class="trk" type="range" min="0" max="10" value="0">
<input id="r50" class="trk" type="range" min="0" max="10" value="5">
<input id="r100" class="trk" type="range" min="0" max="10" value="10">"#,
    ));
    for (id, top, thumb_x) in [
        ("r0", 10.0, 20.0),
        ("r50", 40.0, 70.0),
        ("r100", 70.0, 120.0),
    ] {
        let (content, track, thumb, disabled) = slider(&eng, id);
        assert_eq!(content, (20.0, top, 111.0, 20.0), "{id} content box");
        assert_eq!(thumb, (thumb_x, top, 11.0, 20.0), "{id} thumb from value");
        assert_eq!(track, (20.0, top + 8.0, 111.0, 4.0), "{id} 4 px track");
        assert!(!disabled);
    }
    let img = frame(&mut eng);
    // r50: raised thumb (hi top-left, dk bottom-right, face inside).
    assert_eq!(px(&img, 70.0, 40.0), WHITE, "thumb hi corner");
    assert_eq!(px(&img, 80.0, 59.0), DK, "thumb dk corner");
    assert_eq!(px(&img, 75.0, 50.0), FACE, "thumb face");
    // r0: sunken track away from the thumb, rows sh / dk / panel / hi.
    assert_eq!(px(&img, 100.0, 18.0), SH, "track row 1");
    assert_eq!(px(&img, 100.0, 19.0), DK, "track row 2");
    assert_eq!(px(&img, 100.0, 20.0), PANEL, "track row 3");
    assert_eq!(px(&img, 100.0, 21.0), WHITE, "track row 4 (hi)");
    // Blitz's own input box is gone (UA border 0, transparent), and the
    // mock's ::-webkit-slider-* red never paints.
    assert_eq!(px(&img, 100.0, 12.0), WHITE, "no Blitz box above track");
    assert_eq!(px(&img, 20.0, 42.0), WHITE, "no Blitz border at box edge");
    assert!(
        !img.pixels
            .chunks(4)
            .any(|p| p[0] > 200 && p[1] < 50 && p[2] < 50),
        "pseudo-element CSS is ignored"
    );
}

#[test]
fn unstyled_range_gets_a_default_size() {
    let eng = boot(&page("", r#"<input id="r" type="range" value="50">"#));
    let (content, _t, thumb, _) = slider(&eng, "r");
    assert_eq!((content.2, content.3), (129.0, 20.0));
    assert_eq!(thumb.0, content.0 + 59.0, "50% of 118 px travel");
}

fn drag_page(value: &str, extra: &str) -> String {
    page(
        "#d{top:40px}",
        &format!(
            r#"<form id="f"><input id="d" class="trk" type="range" name="d" min="0" max="100" step="10" value="{value}" hx-post="set.d" hx-trigger="change" {extra}></form>"#
        ),
    )
}

#[test]
fn drag_maps_to_a_stepped_value_and_a_thumb_press_grabs_in_place() {
    let mut eng = boot(&drag_page("50", ""));
    let (cx, cy) = thumb_centre(&eng, "d");
    assert_eq!(cx, 75.5);
    // Press 4 px right of the thumb centre: the thumb is grabbed, no jump.
    let out = eng.input(down(cx + 4.0, cy));
    assert!(action(&out).is_none());
    assert_eq!(value(&eng, "d"), "50", "press on the thumb keeps the value");
    // Move 23 px: centre at 98.5 → 73 → snapped to the step-10 grid.
    let out = eng.input(mv(cx + 27.0, cy));
    assert!(action(&out).is_none(), "never change mid-drag");
    assert_eq!(
        value(&eng, "d"),
        "70",
        "stepped value under the thumb centre"
    );
    let _ = frame(&mut eng);
    assert_eq!(slider(&eng, "d").2 .0, 90.0, "thumb at 70%");
    let out = eng.input(up(cx + 27.0, cy));
    let (name, args, src) = action(&out).expect("change on release");
    assert_eq!((name, src), ("set.d", "d"));
    assert_eq!(args, &[("d".to_string(), "70".to_string())]);
}

#[test]
fn a_groove_press_pages_one_block_toward_the_pointer_and_never_drags() {
    // 0..100 step 10: block = max(10, tenth 10 snapped to 10) = 10.
    let mut eng = boot(&drag_page("0", ""));
    // 63 % of the travel, right of the thumb (at 20..31): one page, no jump.
    let x = 20.0 + 5.5 + 63.0;
    let out = eng.input(down(x, 50.0));
    assert!(action(&out).is_none());
    assert_eq!(value(&eng, "d"), "10", "one block, not the pointer value");
    // Moving the held pointer does not drag the thumb after a groove press.
    let _ = eng.input(mv(x + 20.0, 50.0));
    assert_eq!(value(&eng, "d"), "10", "a groove press never drags");
    let out = eng.input(up(x + 20.0, 50.0));
    let (_, args, _) = action(&out).expect("one change on release");
    assert_eq!(args, &[("d".to_string(), "10".to_string())]);
    settle(&mut eng, &out);
    let _ = frame(&mut eng);
    assert_eq!(slider(&eng, "d").2 .0, 30.0, "thumb at 10%");
    // A press left of the thumb pages back down.
    let _ = eng.input(down(22.0, 50.0));
    assert_eq!(value(&eng, "d"), "0");
    let out = eng.input(up(22.0, 50.0));
    assert_eq!(
        action(&out).expect("change").1,
        &[("d".to_string(), "0".to_string())]
    );
    settle(&mut eng, &out);
    // At min, a press left of the thumb moves nothing and fires no change.
    let _ = eng.input(down(20.0, 50.0));
    let out = eng.input(up(20.0, 50.0));
    assert!(action(&out).is_none(), "clamped page: no change");
}

#[test]
fn the_page_block_is_a_tenth_of_the_range_snapped_to_the_step_but_at_least_one_step() {
    for (step, expect) in [
        ("3", ["9", "18"]),
        ("30", ["30", "60"]),
        ("0.5", ["10", "20"]),
    ] {
        let html = page(
            "#d{top:40px}",
            &format!(
                r#"<input id="d" class="trk" type="range" min="0" max="100" step="{step}" value="0">"#
            ),
        );
        let mut eng = boot(&html);
        for want in expect {
            let _ = eng.input(down(125.0, 50.0));
            let _ = eng.input(up(125.0, 50.0));
            assert_eq!(value(&eng, "d"), want, "step {step}");
        }
    }
}

#[test]
fn keys_step_and_home_end_jump_while_focused_without_scrolling() {
    // The slider sits at the top of a scrolled-to-top, overflowing panel:
    // Home / End must move the slider, not the panel (#26 scroll keys).
    let html = page(
        "#sc{position:absolute;left:0;top:0;width:200px;height:100px;overflow-y:auto}#d{top:20px}#tall{height:400px}",
        r#"<div id="sc"><form id="f"><input id="d" class="trk" type="range" name="d" min="0" max="100" step="10" value="50" hx-post="set.d" hx-trigger="change"></form><div id="tall"></div></div>"#,
    );
    let mut eng = boot(&html);
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(up(cx, cy));
    assert_eq!(eng.focus_state().focused_id.as_deref(), Some("d"));
    let mut step = |k: &str, want: &str| {
        let out = eng.input(key(k));
        let (_, args, _) = action(&out).unwrap_or_else(|| panic!("{k} fires change, got {out:?}"));
        assert_eq!(args, &[("d".to_string(), want.to_string())], "{k}");
        settle(&mut eng, &out);
    };
    step("ArrowRight", "60");
    step("ArrowDown", "50");
    step("End", "100");
    step("Home", "0");
    assert_eq!(
        eng.input(key("Home")),
        InputOutcome::None,
        "Home at min: consumed, no change"
    );
    assert_eq!(
        eng.test_scroll_top("sc").unwrap().0,
        0.0,
        "panel never scrolled"
    );
    let _ = eng.input(key("End"));
    assert_eq!(
        eng.test_scroll_top("sc").unwrap().0,
        0.0,
        "End moved the slider only"
    );
    let _ = frame(&mut eng);
    let (c, _, t, _) = slider(&eng, "d");
    assert_eq!(t.0, c.0 + c.2 - 11.0, "thumb at the right end");
}

#[derive(Clone, Default)]
struct Slices {
    versions: HashMap<String, u64>,
    limits: Vec<(String, String, InputLimits)>,
}

impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        self.limits.clone()
    }
}

#[derive(Default)]
struct Frags {
    bodies: HashMap<String, String>,
}

impl FragmentSource for Frags {
    fn render(&mut self, id: &str, _slices: &[&str]) -> String {
        self.bodies.get(id).cloned().unwrap_or_default()
    }
}

#[test]
fn slider_honours_the_issue16_limits_from_the_actions_slice() {
    // Markup carries 0..100 step 10 (allowed since round 2); the slice says
    // 2..8 step 2 and wins wherever both are present.
    let mut eng = boot(&drag_page("50", ""));
    let slices = Slices {
        versions: HashMap::from([("actions".into(), 1)]),
        limits: vec![(
            "set.d".into(),
            "d".into(),
            InputLimits::new(2.0, 8.0, Some(2.0)),
        )],
    };
    eng.sync(&slices, &mut Frags::default()).expect("sync");
    let _ = frame(&mut eng);
    assert_eq!(eng.attr("d", "max").as_deref(), Some("8"));
    let (c, _, t, _) = slider(&eng, "d");
    assert_eq!(t.0, c.0 + 100.0, "50 clamps to 8 = 100%");
    // Drag far left: clamps to the slice min.
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(mv(0.0, cy));
    let out = eng.input(up(0.0, cy));
    assert_eq!(
        action(&out).expect("change").1,
        &[("d".to_string(), "2".to_string())]
    );
    settle(&mut eng, &out);
    // A groove press right of the thumb pages one slice block: the tenth
    // (0.6) snaps to 0 steps, so the block is one slice step (2): 2 → 4.
    // The markup's own step 10 would have given 10.
    let x = 20.0 + 5.5 + 40.0;
    let _ = eng.input(down(x, cy));
    let out = eng.input(up(x, cy));
    assert_eq!(
        action(&out).expect("change").1,
        &[("d".to_string(), "4".to_string())]
    );
    settle(&mut eng, &out);
    let out = eng.input(key("End"));
    assert_eq!(
        action(&out).expect("change").1,
        &[("d".to_string(), "8".to_string())]
    );
}

#[test]
fn disabled_slider_ignores_input_and_paints_greyed() {
    let mut eng = boot(&drag_page("50", "disabled"));
    let img = frame(&mut eng);
    let (_, _, t, disabled) = slider(&eng, "d");
    assert!(disabled);
    let (cx, cy) = thumb_centre(&eng, "d");
    let (a, b) = (px(&img, cx, cy), px(&img, cx + 1.0, cy));
    assert_ne!(a, b, "dithered thumb face");
    assert!([a, b].contains(&WHITE) && [a, b].contains(&FACE));
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(mv(cx + 40.0, cy));
    let out = eng.input(up(cx + 40.0, cy));
    assert!(action(&out).is_none(), "no change from a disabled slider");
    let _ = eng.input(down(t.0 + 80.0, cy));
    let _ = eng.input(up(t.0 + 80.0, cy));
    for k in ["ArrowRight", "End", "Home"] {
        assert!(action(&eng.input(key(k))).is_none(), "{k}");
    }
    assert_eq!(value(&eng, "d"), "50");
    // Enabled twin: solid face.
    let mut on = boot(&drag_page("50", ""));
    let img = frame(&mut on);
    assert_eq!(px(&img, cx, cy), FACE);
    assert_eq!(px(&img, cx + 1.0, cy), FACE);
}

#[test]
fn slider_drag_damage_stays_in_the_box_and_partial_equals_full() {
    let mut eng = boot(&drag_page("0", ""));
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    for dx in [17.0, 41.0, 88.0] {
        let _ = eng.input(mv(cx + dx, cy));
        let out = eng.paint(vp()).expect("paint");
        assert!(!out.skipped);
        assert!(!eng.last_paint_stats().full_repaint, "partial frame");
        assert!(
            out.damage.iter().all(|r| r.x >= 19.0
                && r.y >= 39.0
                && r.x + r.width <= 132.0
                && r.y + r.height <= 61.0),
            "damage inside the slider box: {:?}",
            out.damage
        );
        let partial = eng.last_frame().unwrap().clone();
        let full = full_frame(&mut eng);
        assert_eq!(partial.pixels, full.pixels, "partial == full at dx={dx}");
    }
    let _ = eng.input(up(cx + 88.0, cy));
}

// ---------------------------------------------------------------------------
// Number field: text-align and the four-sided `--field` bevel.

fn field_page(align: &str) -> String {
    page(
        &format!(
            ".val{{position:absolute;left:20px;top:20px;width:60px;height:20px;padding:2px 6px;line-height:16px;text-align:{align};background:#fff;border:0;color:#000;{FIELD}}}body{{background:#c0c0c0}}"
        ),
        r#"<form id="f"><input id="n" class="val" type="number" name="n" min="1" max="99" step="1" value="7" hx-post="set.d" hx-trigger="change"></form>"#,
    )
}

/// Leftmost / rightmost x of dark text pixels inside the field's content box.
fn ink_span(img: &RgbaImage) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for y in 23..37 {
        for x in 23..77 {
            let p = px(img, x as f32, y as f32);
            if p[0] < 110 && p[1] < 110 && p[2] < 110 {
                lo = lo.min(x as f32);
                hi = hi.max(x as f32);
            }
        }
    }
    (lo, hi)
}

#[test]
fn number_field_text_align_right_puts_digits_at_the_right_edge() {
    let mut eng = boot(&field_page("right"));
    let img = frame(&mut eng);
    let (lo, hi) = ink_span(&img);
    assert!(
        (70.0..=74.0).contains(&hi),
        "right edge of digits near content right 74: {hi}"
    );
    assert!(lo > 50.0, "digit not at the left: {lo}");
    // Left-aligned twin stays left.
    let mut left = boot(&field_page("left"));
    let img = frame(&mut left);
    let (lo, _) = ink_span(&img);
    assert!(lo <= 30.0, "left-aligned digit at the left: {lo}");
    // Caret hit-testing follows the alignment: a click right of the digit
    // puts the caret after it.
    let _ = eng.input(down(76.0, 30.0));
    let _ = eng.input(up(76.0, 30.0));
    let _ = eng.input(InputEvent::Text("8".into()));
    let out = eng.input(key("Enter"));
    assert_eq!(
        action(&out).expect("change").1,
        &[("n".to_string(), "78".to_string())]
    );
}

#[test]
fn number_field_field_bevel_shows_on_all_four_sides() {
    let mut eng = boot(&field_page("right"));
    let img = frame(&mut eng);
    // Box 20..80 x 20..40. Mid edge samples away from the corners.
    assert_eq!(px(&img, 40.0, 20.0), SH, "top outer = sh");
    assert_eq!(px(&img, 40.0, 21.0), FIELD_DK, "top inner = dk");
    assert_eq!(px(&img, 40.0, 39.0), WHITE, "bottom outer = hi");
    assert_eq!(px(&img, 40.0, 38.0), PANEL, "bottom inner = panel");
    assert_eq!(px(&img, 20.0, 30.0), SH, "left outer = sh");
    assert_eq!(px(&img, 21.0, 30.0), FIELD_DK, "left inner = dk");
    assert_eq!(px(&img, 79.0, 30.0), WHITE, "right outer = hi");
    assert_eq!(px(&img, 78.0, 30.0), PANEL, "right inner = panel");
    assert_eq!(px(&img, 40.0, 25.0), WHITE, "field face");
    // Focused + typing keeps the bevel and partial == full.
    let _ = eng.input(down(50.0, 30.0));
    let _ = eng.input(up(50.0, 30.0));
    let _ = eng.input(InputEvent::Text("1".into()));
    let partial = frame(&mut eng);
    let full = full_frame(&mut eng);
    assert_eq!(partial.pixels, full.pixels);
    assert_eq!(px(&full, 79.0, 30.0), WHITE);
}

// ---------------------------------------------------------------------------
// Range + number in sync through the host (the engine does not link them).

fn row_html(v: &str) -> String {
    format!(
        r#"<input id="r" class="trk" type="range" name="hours" min="1" max="24" step="1" value="{v}" hx-post="settings.set" hx-trigger="change"><input id="n" class="val" type="number" name="hours" min="1" max="24" step="1" value="{v}" hx-post="settings.set" hx-trigger="change">"#
    )
}

/// Host: both inputs share the name, so `args` carries both values; the
/// trigger's own pair comes first (form_args rule (1)), so the changed
/// control's value is the first `hours`.
fn host_value(out: &InputOutcome) -> String {
    let (name, args, src) = action(out).unwrap_or_else(|| panic!("change, got {out:?}"));
    assert_eq!(name, "settings.set");
    assert!(src == "r" || src == "n", "{src}");
    let all = args_get_all(args, "hours");
    assert_eq!(all.len(), 2, "both inputs post: {args:?}");
    all[0].to_string()
}

#[test]
fn range_and_number_sync_end_to_end_through_a_host_swap() {
    let html = page(
        "#r{top:40px}.val{position:absolute;left:150px;top:40px;width:50px;height:20px;padding:2px 6px;text-align:right;border:0}",
        r##"<form id="f"><section id="sec-row" hx-trigger="slice:settings" hx-target="#row" hx-swap="innerHTML"><div id="row"></div></section></form>"##,
    );
    let mut eng = boot(&html);
    let mut slices = Slices {
        versions: HashMap::from([("settings".into(), 1), ("actions".into(), 1)]),
        limits: Vec::new(),
    };
    let mut frags = Frags::default();
    frags.bodies.insert("sec-row".into(), row_html("6"));
    eng.sync(&slices, &mut frags).expect("sync");
    let _ = frame(&mut eng);
    let thumb_x = |eng: &Engine, v: f32| {
        let c = slider(eng, "r").0;
        c.0 + ((v - 1.0) / 23.0 * (c.2 - 11.0)).round()
    };
    assert_eq!(slider(&eng, "r").2 .0, thumb_x(&eng, 6.0));

    // 1. Drag the range to 12: change → host re-renders both.
    let (cx, cy) = thumb_centre(&eng, "r");
    let target = thumb_x(&eng, 12.0) + 5.5;
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(mv(target, cy));
    let out = eng.input(up(target, cy));
    settle(&mut eng, &out);
    let v = host_value(&out);
    assert_eq!(v, "12");
    *slices.versions.get_mut("settings").unwrap() += 1;
    frags.bodies.insert("sec-row".into(), row_html(&v));
    eng.sync(&slices, &mut frags).expect("sync");
    let _ = frame(&mut eng);
    assert_eq!(
        (value(&eng, "r"), value(&eng, "n")),
        ("12".into(), "12".into())
    );

    // 2. Type 20 into the number field: change → host re-renders both, the
    // range thumb follows.
    let nb = eng.test_element_box("n").unwrap();
    let _ = eng.input(down(nb.0 + nb.2 / 2.0, nb.1 + nb.3 / 2.0));
    let _ = eng.input(up(nb.0 + nb.2 / 2.0, nb.1 + nb.3 / 2.0));
    let _ = eng.input(key_mods(
        "a",
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    ));
    let _ = eng.input(InputEvent::Text("20".into()));
    let out = eng.input(key("Enter"));
    settle(&mut eng, &out);
    let v = host_value(&out);
    assert_eq!(v, "20");
    *slices.versions.get_mut("settings").unwrap() += 1;
    frags.bodies.insert("sec-row".into(), row_html(&v));
    eng.sync(&slices, &mut frags).expect("sync");
    let partial = frame(&mut eng);
    assert_eq!(
        (value(&eng, "r"), value(&eng, "n")),
        ("20".into(), "20".into())
    );
    assert_eq!(
        slider(&eng, "r").2 .0,
        thumb_x(&eng, 20.0),
        "thumb followed the swap"
    );
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "swap frame partial == full");
}

/// Byte-identical frames; on mismatch, report the differing pixel bounds.
fn assert_same(a: &RgbaImage, b: &RgbaImage, what: &str) {
    if a.pixels == b.pixels {
        return;
    }
    let w = a.width as usize;
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (usize::MAX, usize::MAX, 0, 0, 0);
    for (i, (p, q)) in a.pixels.chunks(4).zip(b.pixels.chunks(4)).enumerate() {
        if p != q {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            n += 1;
        }
    }
    panic!("{what}: {n} px differ in x {x0}..={x1}, y {y0}..={y1}");
}

// ---------------------------------------------------------------------------
// `--hx-*` variables: colours come from the computed custom properties on the
// control, so a host swaps looks at runtime with plain CSS.

/// A Win98-ish skin (every variable set) or none (the page's [`HOST_MAP`]),
/// around a slider and an `overflow-y: scroll` container.
fn skin_html(vars: &str) -> String {
    format!(
        r#"<div style="{vars}"><input id="r" class="trk" type="range" min="0" max="10" value="5" style="top:40px"><div id="sc" style="position:absolute;left:200px;top:10px;width:120px;height:80px;overflow-y:scroll"><div style="height:300px"></div></div></div>"#
    )
}

const SKIN: &str = "--hx-face:rgb(212, 208, 200);--hx-bevel-hi:#fefefe;--hx-bevel-lt:#e0e0e0;--hx-bevel-sh:#7f7f7f;--hx-bevel-dk:#404040;--hx-ink:#102030;--hx-track:#a0a0a0;--hx-track-alt:none";

#[test]
fn look_tokens_colour_the_slider_and_bar_and_a_token_change_repaints() {
    let html = page(
        "",
        r##"<section id="sec-skin" hx-trigger="slice:settings" hx-target="#skin" hx-swap="innerHTML"><div id="skin"></div></section>"##,
    );
    let mut eng = boot(&html);
    let mut slices = Slices {
        versions: HashMap::from([("settings".into(), 1)]),
        limits: Vec::new(),
    };
    let mut frags = Frags::default();
    frags.bodies.insert("sec-skin".into(), skin_html(SKIN));
    eng.sync(&slices, &mut frags).expect("sync");
    let img = frame(&mut eng);
    let skin_face = [212, 208, 200, 0xFF];
    let skin_dk = [0x40, 0x40, 0x40, 0xFF];
    // Slider: thumb (70..81 x 40..60) face, edges; groove rows.
    assert_eq!(px(&img, 75.0, 50.0), skin_face, "thumb face = --hx-face");
    assert_eq!(
        px(&img, 70.0, 40.0),
        [0xFE, 0xFE, 0xFE, 0xFF],
        "thumb --hx-bevel-hi"
    );
    assert_eq!(px(&img, 80.0, 59.0), skin_dk, "thumb --hx-bevel-dk");
    assert_eq!(
        px(&img, 79.0, 58.0),
        [0x7F, 0x7F, 0x7F, 0xFF],
        "thumb --hx-bevel-sh"
    );
    assert_eq!(
        px(&img, 71.0, 41.0),
        [0xE0, 0xE0, 0xE0, 0xFF],
        "thumb --hx-bevel-lt"
    );
    assert_eq!(
        px(&img, 100.0, 49.0),
        skin_dk,
        "groove row 2 = --hx-bevel-dk"
    );
    // Bar: x 304..320; up button 10..26, track solid --hx-track.
    assert_eq!(px(&img, 319.0, 25.0), skin_dk, "up button --hx-bevel-dk");
    assert_eq!(
        px(&img, 308.0, 14.0),
        skin_face,
        "up button face = --hx-face"
    );
    assert_eq!(
        px(&img, 310.0, 60.0),
        [0xA0, 0xA0, 0xA0, 0xFF],
        "--hx-track"
    );
    assert_eq!(
        px(&img, 311.0, 60.0),
        [0xA0, 0xA0, 0xA0, 0xFF],
        "--hx-track-alt: none = solid"
    );

    // Host drops the skin: the :root mapping applies again, repainted
    // on a partial frame that equals a full repaint.
    *slices.versions.get_mut("settings").unwrap() += 1;
    frags.bodies.insert("sec-skin".into(), skin_html(""));
    eng.sync(&slices, &mut frags).expect("sync");
    let partial = frame(&mut eng);
    assert_eq!(px(&partial, 75.0, 50.0), FACE, "thumb face from :root");
    assert_eq!(px(&partial, 80.0, 59.0), DK, "--hx-bevel-dk from :root");
    assert_eq!(px(&partial, 70.0, 40.0), WHITE, "--hx-bevel-hi from :root");
    assert_eq!(px(&partial, 71.0, 41.0), PANEL, "--hx-bevel-lt from :root");
    assert_eq!(
        px(&partial, 319.0, 25.0),
        DK,
        "bar --hx-bevel-dk from :root"
    );
    // :root sets --hx-track-alt: the 1 px --hx-track / --hx-track-alt checker.
    let (a, b) = (px(&partial, 310.0, 60.0), px(&partial, 311.0, 60.0));
    assert!(
        (a == FACE && b == WHITE) || (a == WHITE && b == FACE),
        "checker track: {a:?} {b:?}"
    );
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "token change partial == full");
}

#[test]
fn a_swapped_theme_stylesheet_repaints_slider_and_bar_without_touching_them() {
    // The look swap a host would do at runtime: only a `<style>` overriding
    // the `:root` `--hx-*` variables is swapped; the slider and scroller nodes are untouched.
    let html = page(
        "",
        r##"<section id="sec-theme" hx-trigger="slice:settings" hx-target="#theme" hx-swap="innerHTML"><div id="theme"></div></section><input id="r" class="trk" type="range" min="0" max="10" value="5" style="top:40px"><div id="sc" style="position:absolute;left:200px;top:10px;width:120px;height:80px;overflow-y:scroll"><div style="height:300px"></div></div>"##,
    );
    let mut eng = boot(&html);
    let img = frame(&mut eng);
    assert_eq!(
        px(&img, 75.0, 50.0),
        FACE,
        "page :root mapping before the theme"
    );
    assert_eq!(px(&img, 319.0, 25.0), DK);
    let mut slices = Slices {
        versions: HashMap::from([("settings".into(), 1)]),
        limits: Vec::new(),
    };
    let mut frags = Frags::default();
    frags.bodies.insert(
        "sec-theme".into(),
        format!("<style>:root{{{SKIN}}}</style>"),
    );
    eng.sync(&slices, &mut frags).expect("sync");
    let partial = frame(&mut eng);
    assert_eq!(
        px(&partial, 75.0, 50.0),
        [212, 208, 200, 0xFF],
        "thumb face"
    );
    assert_eq!(
        px(&partial, 80.0, 59.0),
        [0x40, 0x40, 0x40, 0xFF],
        "thumb dk"
    );
    assert_eq!(
        px(&partial, 319.0, 25.0),
        [0x40, 0x40, 0x40, 0xFF],
        "bar dk"
    );
    assert_eq!(
        px(&partial, 310.0, 60.0),
        [0xA0, 0xA0, 0xA0, 0xFF],
        "bar track"
    );
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "theme swap partial == full");
    // And back: the theme is removed, the page's mapping returns.
    *slices.versions.get_mut("settings").unwrap() += 1;
    frags.bodies.insert("sec-theme".into(), String::new());
    eng.sync(&slices, &mut frags).expect("sync");
    let partial = frame(&mut eng);
    assert_eq!(px(&partial, 75.0, 50.0), FACE);
    assert_eq!(px(&partial, 319.0, 25.0), DK);
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "theme removal partial == full");
}

#[test]
fn focused_slider_shows_a_dotted_focus_rectangle_and_no_blitz_ring() {
    let mut eng = boot(&drag_page("50", ""));
    let before = frame(&mut eng);
    assert_eq!(px(&before, 22.0, 40.0), WHITE, "no dots while unfocused");
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let out = eng.input(up(cx, cy));
    assert!(
        action(&out).is_none(),
        "a click that moves nothing fires no change"
    );
    assert_eq!(eng.focus_state().focused_id.as_deref(), Some("d"));
    let partial = frame(&mut eng);
    // Content box 20..131 x 40..60: dots on every other pixel along the edge.
    assert_eq!(px(&partial, 22.0, 40.0), INK, "top edge dot");
    assert_eq!(px(&partial, 23.0, 40.0), WHITE, "top edge gap");
    assert_eq!(px(&partial, 20.0, 44.0), INK, "left edge dot");
    assert_eq!(px(&partial, 20.0, 45.0), WHITE, "left edge gap");
    assert_eq!(px(&partial, 22.0, 59.0), INK, "bottom edge dot");
    assert_eq!(px(&partial, 130.0, 44.0), INK, "right edge dot");
    // The thumb sits on top of the rectangle; Blitz's blue ring is suppressed.
    assert_eq!(px(&partial, 75.0, 50.0), FACE);
    assert_eq!(px(&partial, 18.0, 50.0), WHITE, "no ring outside the box");
    assert_eq!(px(&partial, 75.0, 38.0), WHITE, "no ring above the box");
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "focus frame partial == full");
}

#[test]
fn mock34b_padded_box_gives_an_18px_thumb_on_a_centred_groove() {
    // Mock 34b (9fc61473): height 24, padding-bottom 6 for the tick row.
    let mut eng = boot(&page(
        "#t{top:40px;height:24px;padding:0 0 6px;box-sizing:border-box;width:110px}",
        r#"<input id="t" class="trk" type="range" min="0" max="100" value="0">"#,
    ));
    let (content, track, thumb, _) = slider(&eng, "t");
    assert_eq!(content, (20.0, 40.0, 110.0, 18.0));
    assert_eq!(
        thumb,
        (20.0, 40.0, 11.0, 18.0),
        "thumb fills the content height"
    );
    assert_eq!(
        track,
        (20.0, 47.0, 110.0, 4.0),
        "groove centred under the thumb"
    );
    let _ = frame(&mut eng);
}

// ---------------------------------------------------------------------------
// The mock 34b fixture (Mainnet 9fc61473): 12 sliders, each beside a real
// `<input type=number>` with the same name, min, max and step.

const MOCK34B: &str = include_str!("fixtures/settings34_advanced.html");

fn vp34() -> Viewport {
    Viewport::with_scale(1280, 720, 1.0)
}

#[test]
fn mock34b_fixture_draws_every_slider_beside_a_real_value_field() {
    let mut eng = Engine::new(cfg());
    eng.declare_actions(&[
        "menu.back",
        "settings.tab",
        "settings.apply",
        "settings.engine.reset",
    ]);
    eng.load_html(MOCK34B).expect("load");
    let img = eng.paint(vp34()).unwrap().rgba.expect("rgba");
    let id = "set-engine-sim-tick-wall-ms";
    let (content, track, thumb, disabled) = slider(&eng, id);
    assert!(!disabled);
    assert_eq!(
        (content.2, content.3),
        (110.0, 18.0),
        "24 px box minus 6 px tick row"
    );
    // 1000 in [50, 2000]: 950 / 1950 of 99 px travel = 48.2 → 48.
    assert_eq!(thumb, (content.0 + 48.0, content.1, 11.0, 18.0));
    assert_eq!(track.1, content.1 + 7.0);
    assert_eq!(px(&img, thumb.0 + 5.0, thumb.1 + 9.0), FACE, "thumb face");
    assert_eq!(
        px(&img, thumb.0 + 10.0, thumb.1 + 17.0),
        DK,
        "thumb --hx-bevel-dk"
    );
    // The mock's own tick row (author background, painted by Blitz) stays,
    // in the 4 px under the content box.
    // Blitz beta.4 places the tile 4 px further in than Chrome and drops the
    // 11th tick (x + 9, 19, … 99 instead of 5, 15, … 105), so count ticks.
    let ticks = (0..110)
        .filter(|dx| px(&img, content.0 + *dx as f32, content.1 + 20.0) == DK)
        .count();
    assert!(ticks >= 10, "tick row under the groove: {ticks}");
    let val = format!("{id}-val");
    assert_eq!(eng.attr(&val, "type").as_deref(), Some("number"));
    assert_eq!(
        eng.attr(&val, "name").as_deref(),
        Some("engine.sim_tick_wall_ms")
    );
    for a in ["min", "max", "step"] {
        assert_eq!(eng.attr(&val, a), eng.attr(id, a), "{a} matches the slider");
    }
    // Drag two steps right on a partial frame inside the scroller.
    let (cx, cy) = thumb_centre(&eng, id);
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(mv(cx + 40.0, cy));
    let _ = eng.input(up(cx + 40.0, cy));
    let partial = eng.paint(vp34()).unwrap();
    assert!(!partial.skipped && !eng.last_paint_stats().full_repaint);
    let partial = eng.last_frame().unwrap().clone();
    // The thumb was drawn at 48 px (48.2 rounded); grabbed at its centre and
    // moved 40 px: 50 + (88 / 99) × 1950 = 1783.3 → step 10 → 1780.
    assert_eq!(value(&eng, id), "1780");
    eng.mark_theme_or_stylesheet_changed();
    let full = eng.paint(vp34()).unwrap().rgba.expect("full");
    assert_same(&partial, &full, "mock 34b slider drag partial == full");
}

// ---------------------------------------------------------------------------
// Neutral engine defaults and the `<select>` dropdown.

const SELECT_HTML: &str = r#"<select id="sel" name="s" style="position:absolute;left:20px;top:120px;width:120px;height:22px"><option value="a" selected="">Alpha</option><option value="b">Beta</option></select>"#;

#[test]
fn without_host_variables_every_engine_drawn_control_uses_the_neutral_greys() {
    let html = page_bare("", &format!(r#"{}{SELECT_HTML}"#, skin_html("")));
    let mut eng = boot(&html);
    let img = frame(&mut eng);
    let face = [0xD4, 0xD4, 0xD4, 0xFF];
    let dk = [0x40, 0x40, 0x40, 0xFF];
    let sh = [0x8C, 0x8C, 0x8C, 0xFF];
    assert_eq!(px(&img, 75.0, 50.0), face, "thumb face default");
    assert_eq!(px(&img, 80.0, 59.0), dk, "thumb dk default");
    assert_eq!(px(&img, 70.0, 40.0), WHITE, "thumb hi default");
    assert_eq!(px(&img, 319.0, 25.0), dk, "bar dk default");
    assert_eq!(px(&img, 308.0, 14.0), face, "bar face default");
    let track = [0xE8, 0xE8, 0xE8, 0xFF];
    assert_eq!(px(&img, 310.0, 60.0), track, "solid track default");
    assert_eq!(
        px(&img, 311.0, 60.0),
        track,
        "no checker without --hx-track-alt"
    );
    assert_eq!(px(&img, 126.0, 124.0), face, "select button face default");
    assert_eq!(
        px(&img, 20.0, 130.0),
        sh,
        "select UA border = --hx-bevel-sh default"
    );
}

/// The 2026-10-21 styling rule: engine-drawn controls read only the crate's
/// `--hx-*` variables. A host that defines only its own token names (the
/// Mainnet fixture's `--bg` / `--edge-*` / `--ink` / `--field`, plus look-alike
/// `--face` / `--track` names without the prefix) changes no engine-drawn
/// pixel: the frame equals one with no host variables at all.
#[test]
fn host_tokens_without_the_hx_prefix_change_no_engine_drawn_pixel() {
    let body = format!(r#"{}{SELECT_HTML}"#, skin_html(""));
    let mut bare = boot(&page_bare("", &body));
    bare.test_set_focus("r");
    let want = frame(&mut bare);
    let tokens = ":root{--bg:#ff0000;--ink:#00ff00;--edge-hi:#0000ff;--edge-lt:#ff00ff;--edge-sh:#00ffff;--edge-dk:#ffff00;--field:inset 1px 1px #ff0000;--face:#ff0000;--bevel-hi:#00ff00;--track:#0000ff;--thumb-w:40px;--groove-h:12px;--select-hl:#ff0000}";
    let mut themed = boot(&page_bare(tokens, &body));
    themed.test_set_focus("r");
    let got = frame(&mut themed);
    assert!(
        want.pixels == got.pixels,
        "non-`--hx-*` host tokens must not change slider, scrollbar or select pixels"
    );
}

#[test]
fn overriding_hx_variables_recolours_the_select_button_and_open_list() {
    let html = page(
        "",
        &format!(
            r##"<section id="sec-theme" hx-trigger="slice:settings" hx-target="#theme" hx-swap="innerHTML"><div id="theme"></div></section>{SELECT_HTML}"##
        ),
    );
    let mut eng = boot(&html);
    let img = frame(&mut eng);
    assert_eq!(px(&img, 126.0, 124.0), FACE, "button --hx-face from :root");
    assert_eq!(
        px(&img, 20.0, 130.0),
        SH,
        "UA border --hx-bevel-sh from :root"
    );
    // Open the list (focus + Alt+ArrowDown): row 0 highlighted, row 1 plain.
    eng.test_set_focus("sel");
    let _ = eng.input(key_mods(
        "ArrowDown",
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ));
    let img = frame(&mut eng);
    let navy = [0x00, 0x00, 0x80, 0xFF];
    assert_eq!(px(&img, 136.0, 150.0), navy, "highlight --hx-select-hl");
    assert_eq!(px(&img, 136.0, 170.0), WHITE, "row --hx-select-bg");

    // Runtime theme swap: only a `<style>` overriding the variables.
    let slices = Slices {
        versions: HashMap::from([("settings".into(), 1)]),
        limits: Vec::new(),
    };
    let mut frags = Frags::default();
    frags.bodies.insert(
        "sec-theme".into(),
        "<style>:root{--hx-face:#d4d0c8;--hx-select-hl:#3a6ea5;--hx-select-bg:#ffffe0;--hx-bevel-sh:#7f7f7f}</style>".into(),
    );
    eng.sync(&slices, &mut frags).expect("sync");
    let partial = frame(&mut eng);
    assert_eq!(
        px(&partial, 126.0, 124.0),
        [0xD4, 0xD0, 0xC8, 0xFF],
        "button face follows --hx-face"
    );
    assert_eq!(
        px(&partial, 136.0, 150.0),
        [0x3A, 0x6E, 0xA5, 0xFF],
        "highlight follows --hx-select-hl"
    );
    assert_eq!(
        px(&partial, 136.0, 170.0),
        [0xFF, 0xFF, 0xE0, 0xFF],
        "row follows --hx-select-bg"
    );
    assert_eq!(
        px(&partial, 20.0, 130.0),
        [0x7F, 0x7F, 0x7F, 0xFF],
        "UA border follows --hx-bevel-sh"
    );
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "select theme swap partial == full");
}

// ---------------------------------------------------------------------------
// Graph review of round 2 (7bbff3e).

fn graph_range(attrs: &str) -> String {
    page(
        "#d{top:40px}",
        &format!(
            r#"<form id="f"><input id="d" class="trk" type="range" name="d" {attrs} hx-post="set.d" hx-trigger="change"></form>"#
        ),
    )
}

fn graph_slices(v: u64, limits: Vec<(String, String, InputLimits)>) -> Slices {
    Slices {
        versions: HashMap::from([("actions".into(), v), ("settings".into(), v)]),
        limits,
    }
}

/// A groove page past a max that is off the step grid stays on the grid
/// (HTML: the last valid value below max), like End and a full drag.
#[test]
fn a_page_past_an_off_grid_max_stays_on_the_step_grid() {
    let mut eng = boot(&graph_range(r#"min="0" max="10" step="3" value="9""#));
    let (c, _, t, _) = slider(&eng, "d");
    let (x, y) = ((t.0 + t.2 + 3.0).min(c.0 + c.2 - 1.0), c.1 + c.3 / 2.0);
    let _ = eng.input(down(x, y));
    let out = eng.input(up(x, y));
    assert!(action(&out).is_none(), "9 is the top of the grid: {out:?}");
    assert_eq!(value(&eng, "d"), "9");
    let _ = eng.input(down(c.0 + 1.0, y));
    let out = eng.input(up(c.0 + 1.0, y));
    assert_eq!(
        action(&out).expect("change").1,
        &[("d".to_string(), "6".to_string())]
    );
}

/// Graph review of #37 r4: a `min` with more decimals than `step`
/// (`0.5..10 step 1`): drag, groove page, PageUp and End stay on the grid
/// from `min`, as the arrow keys do.
#[test]
fn a_fractional_min_keeps_drag_page_and_end_on_its_grid() {
    let mut eng = boot(&graph_range(r#"min="0.5" max="10" step="1" value="0.5""#));
    eng.test_set_focus("d");
    let mut seen = Vec::new();
    let k = |eng: &mut Engine, k: &str| {
        let out = eng.input(key(k));
        settle(eng, &out);
        value(eng, "d")
    };
    seen.push(k(&mut eng, "PageUp"));
    seen.push(k(&mut eng, "End"));
    seen.push(k(&mut eng, "PageDown"));
    assert_eq!(seen, ["1.5", "9.5", "8.5"]);
    let c = slider(&eng, "d").0;
    let y = c.1 + c.3 / 2.0;
    let _ = eng.input(down(c.0 + 1.0, y));
    let out = eng.input(up(c.0 + 1.0, y));
    settle(&mut eng, &out);
    assert_eq!(value(&eng, "d"), "7.5", "groove page");
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let mut x = c.0 - 10.0;
    while x <= c.0 + c.2 + 10.0 {
        let _ = eng.input(mv(x, cy));
        let v: f64 = value(&eng, "d").parse().unwrap();
        assert!(
            (0.5..=9.5).contains(&v) && (v - 0.5).fract() == 0.0,
            "drag at x {x} gave {v}"
        );
        x += 1.0;
    }
    let out = eng.input(up(c.0 + c.2 + 10.0, cy));
    settle(&mut eng, &out);
    assert_eq!(value(&eng, "d"), "9.5");
}

/// A #16 limits change moves the thumb without touching `value`; the old
/// thumb is repainted (partial == full).
#[test]
fn a_limits_change_moves_the_thumb_and_partial_equals_full() {
    let mut eng = boot(&graph_range(r#"min="0" max="100" step="10" value="50""#));
    let lim = |max: f64| {
        vec![(
            "set.d".to_string(),
            "d".to_string(),
            InputLimits::new(0.0, max, Some(10.0)),
        )]
    };
    eng.sync(&graph_slices(1, lim(100.0)), &mut Frags::default())
        .unwrap();
    let _ = frame(&mut eng);
    eng.sync(&graph_slices(2, lim(200.0)), &mut Frags::default())
        .unwrap();
    let partial = frame(&mut eng);
    let (c, _, t, _) = slider(&eng, "d");
    assert_eq!((value(&eng, "d").as_str(), t.0), ("50", c.0 + 25.0));
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "limits change partial == full");
}

/// Tab away from a focused slider clears its focus dots (partial == full).
#[test]
fn tab_away_from_a_slider_clears_the_focus_dots() {
    let html = page(
        "#d{top:40px}#b{position:absolute;left:200px;top:40px;width:60px;height:20px}",
        r#"<form id="f"><input id="d" class="trk" type="range" name="d" min="0" max="100" step="10" value="50"><button id="b" type="button">B</button></form>"#,
    );
    let mut eng = boot(&html);
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(up(cx, cy));
    assert_eq!(px(&frame(&mut eng), 22.0, 40.0), INK, "dots while focused");
    let _ = eng.input(key("Tab"));
    assert_eq!(eng.focus_state().focused_id.as_deref(), Some("b"));
    let partial = frame(&mut eng);
    assert_eq!(px(&partial, 22.0, 40.0), WHITE, "dots gone");
    let full = full_frame(&mut eng);
    assert_same(&partial, &full, "tab-away partial == full");
}

/// A right-aligned field that arrives by swap is aligned too (the
/// post-resolve pass, not only the load-time one).
#[test]
fn a_swapped_in_right_aligned_field_is_aligned() {
    let html = page(
        ".val{position:absolute;left:20px;top:20px;width:60px;height:20px;padding:2px 6px;line-height:16px;text-align:right;background:#fff;border:0;color:#000}body{background:#c0c0c0}",
        r##"<section id="sec" hx-trigger="slice:settings" hx-target="#t" hx-swap="innerHTML"><div id="t"></div></section>"##,
    );
    let mut eng = boot(&html);
    let mut frags = Frags::default();
    frags.bodies.insert(
        "sec".into(),
        r#"<input id="n" class="val" type="number" name="n" value="7">"#.into(),
    );
    eng.sync(&graph_slices(1, vec![]), &mut frags).unwrap();
    let (lo, hi) = ink_span(&frame(&mut eng));
    assert!((70.0..=74.0).contains(&hi) && lo > 50.0, "{lo}..{hi}");
}

/// The bevel replay paints square bands, so a rounded field keeps Blitz's
/// own paint: the rounded-off corner shows what is behind it.
#[test]
fn a_rounded_field_keeps_blitz_paint_at_its_corners() {
    let html = page(
        &format!(".val{{position:absolute;left:20px;top:20px;width:60px;height:20px;padding:2px 6px;background:#fff;border:0;border-radius:8px;{FIELD}}}body{{background:#00ff00}}"),
        r#"<input id="n" class="val" type="text" value="7">"#,
    );
    let mut eng = boot(&html);
    let img = frame(&mut eng);
    assert_eq!(px(&img, 20.0, 20.0), [0, 0xFF, 0, 0xFF]);
}

// ---------------------------------------------------------------------------
// Round 3: off-grid max for every pointer / key path, and PageUp / PageDown
// ahead of #34's scroll keys.

/// Drag past the end, End, and a groove page all stop on the highest grid
/// value at or below an off-grid max (0..10 step 3: 9, never 10).
#[test]
fn drag_and_end_stop_on_the_grid_below_an_off_grid_max() {
    let mut eng = boot(&graph_range(r#"min="0" max="10" step="3" value="0""#));
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(mv(cx + 1000.0, cy));
    assert_eq!(value(&eng, "d"), "9", "drag past the end");
    let out = eng.input(up(cx + 1000.0, cy));
    assert_eq!(
        action(&out).expect("change").1,
        &[("d".to_string(), "9".to_string())]
    );
    let _ = eng.input(key("Home"));
    assert_eq!(value(&eng, "d"), "0");
    let _ = eng.input(key("End"));
    assert_eq!(value(&eng, "d"), "9", "End");
    // Every pixel of a drag lands on the grid.
    let (c, _, _, _) = slider(&eng, "d");
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let mut x = c.0 - 5.0;
    while x < c.0 + c.2 + 5.0 {
        let _ = eng.input(mv(x, cy));
        let v: f64 = value(&eng, "d").parse().unwrap();
        assert!((0.0..=9.0).contains(&v) && v % 3.0 == 0.0, "x={x}: {v}");
        x += 1.0;
    }
    let _ = eng.input(up(x, cy));
    // 0..10 step 4: the top grid value is 8; rounding 10 / 4 = 2.5 up would
    // give 12, clamped to the off-grid 10 without the grid-top rule.
    let mut eng = boot(&graph_range(r#"min="0" max="10" step="4" value="0""#));
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(mv(cx + 1000.0, cy));
    assert_eq!(value(&eng, "d"), "8", "drag past the end, step 4");
    let _ = eng.input(up(cx + 1000.0, cy));
    let _ = eng.input(key("Home"));
    let _ = eng.input(key("End"));
    assert_eq!(value(&eng, "d"), "8", "End, step 4");
}

/// PageUp / PageDown on a focused slider page one block (max(step,
/// (max-min)/10) on the grid), and never past the grid top.
#[test]
fn page_keys_page_a_focused_slider() {
    let mut eng = boot(&graph_range(r#"min="0" max="100" step="5" value="50""#));
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(up(cx, cy));
    let out = eng.input(key("PageDown"));
    assert_eq!(value(&eng, "d"), "40");
    assert_eq!(
        action(&out).expect("change").1,
        &[("d".to_string(), "40".to_string())]
    );
    let _ = eng.input(key("PageUp"));
    let _ = eng.input(key("PageUp"));
    assert_eq!(value(&eng, "d"), "60");
    let mut eng = boot(&graph_range(r#"min="0" max="10" step="3" value="6""#));
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(up(cx, cy));
    let _ = eng.input(key("PageUp"));
    assert_eq!(value(&eng, "d"), "9");
    let _ = eng.input(key("PageUp"));
    assert_eq!(value(&eng, "d"), "9", "grid top under max 10");
}

/// A slider inside a scroll container: PageDown moves the slider by one
/// block and does not scroll the container (slider keys run before #34's
/// scroll keys, as number-box arrows do). Even at the slider's bottom the
/// key stays with the slider.
#[test]
fn page_down_on_a_slider_in_a_scroll_container_moves_the_slider_not_the_list() {
    let html = page(
        "#box{position:absolute;left:0;top:0;width:300px;height:100px;overflow-y:auto}#d{top:20px}#fill{height:600px}",
        r#"<div id="box"><form id="f"><input id="d" class="trk" type="range" name="d" min="0" max="100" step="10" value="50" hx-post="set.d" hx-trigger="change"></form><div id="fill"></div></div>"#,
    );
    let mut eng = boot(&html);
    let _ = frame(&mut eng);
    let (top0, max) = eng.test_scroll_top("box").expect("box scrolls");
    assert!(max > 0.0, "the container can scroll");
    let (cx, cy) = thumb_centre(&eng, "d");
    let _ = eng.input(down(cx, cy));
    let _ = eng.input(up(cx, cy));
    let out = eng.input(key("PageDown"));
    assert_eq!(value(&eng, "d"), "40", "one block down");
    assert!(action(&out).is_some(), "change fires: {out:?}");
    assert_eq!(
        eng.test_scroll_top("box").unwrap().0,
        top0,
        "list did not scroll"
    );
    for _ in 0..8 {
        let _ = eng.input(key("PageDown"));
    }
    assert_eq!(value(&eng, "d"), "0");
    assert_eq!(
        eng.test_scroll_top("box").unwrap().0,
        top0,
        "at the slider's bottom PageDown still does not scroll"
    );
    let _ = eng.input(key("PageUp"));
    assert_eq!(value(&eng, "d"), "10");
    assert_eq!(eng.test_scroll_top("box").unwrap().0, top0);
}
