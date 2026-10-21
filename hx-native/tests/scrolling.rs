//! Issue #26: scroll containers (`overflow-y: auto | scroll`).
//!
//! Driven by Mainnet mock 34 (Settings, Advanced tab): a 480x356 dialog on a
//! 1280x720 surface whose `#set-panel-advanced.panel.scroll` is taller than
//! the window. See `tests/fixtures/settings34_advanced.html`.

use std::collections::HashMap;
use std::time::Duration;

use hx_native::*;

const MOCK34: &str = include_str!("fixtures/settings34_advanced.html");
const ADV_PANEL: &str = include_str!("fixtures/settings34_advanced_panel.html");
const PANEL: &str = "set-panel-advanced";
const ACTIONS: &[&str] = &[
    "menu.back",
    "settings.tab",
    "settings.apply",
    "settings.engine.reset",
    "act.do",
];

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    }
}

fn vp34() -> Viewport {
    Viewport::with_scale(1280, 720, 1.0)
}

fn boot_with(html: &str, vp: Viewport) -> Engine {
    let mut eng = Engine::new(cfg());
    eng.declare_actions(ACTIONS);
    eng.load_html(html).expect("load");
    eng.paint(vp).expect("first paint");
    eng
}

fn boot34() -> Engine {
    boot_with(MOCK34, vp34())
}

fn top(eng: &Engine, id: &str) -> f32 {
    eng.test_scroll_top(id).expect(id).0
}

fn max(eng: &Engine, id: &str) -> f32 {
    eng.test_scroll_top(id).expect(id).1
}

fn bx(eng: &Engine, id: &str) -> (f32, f32, f32, f32) {
    eng.test_element_box(id).expect(id)
}

fn center(eng: &Engine, id: &str) -> (f32, f32) {
    let b = bx(eng, id);
    (b.0 + b.2 * 0.5, b.1 + b.3 * 0.5)
}

fn wheel(x: f32, y: f32, dy: f32, unit: WheelUnit) -> InputEvent {
    InputEvent::Wheel {
        pos: (x, y),
        delta: (0.0, dy),
        unit,
    }
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
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    })
}

fn scrolled(id: &str) -> InputOutcome {
    InputOutcome::Scrolled {
        id: Some(id.into()),
    }
}

fn frame(eng: &mut Engine, vp: Viewport) -> (PaintOutput, RgbaImage) {
    let out = eng.paint(vp).expect("paint");
    let img = out
        .rgba
        .clone()
        .unwrap_or_else(|| eng.last_frame().expect("frame").clone());
    (out, img)
}

fn full_frame(eng: &mut Engine, vp: Viewport) -> RgbaImage {
    eng.mark_theme_or_stylesheet_changed();
    eng.paint(vp).unwrap().rgba.expect("full repaint rgba")
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

fn region(img: &RgbaImage, b: (f32, f32, f32, f32)) -> Vec<u8> {
    let mut out = Vec::new();
    for y in b.1 as u32..(b.1 + b.3) as u32 {
        for x in b.0 as u32..(b.0 + b.2) as u32 {
            let i = ((y * img.width + x) * 4) as usize;
            out.extend_from_slice(&img.pixels[i..i + 4]);
        }
    }
    out
}

/// Every damage rect lies inside `b` (CSS = device at scale 1), with 1 px
/// of snap slack.
fn damage_inside(out: &PaintOutput, b: (f32, f32, f32, f32)) -> bool {
    !out.damage.is_empty()
        && out.damage.iter().all(|r| {
            r.x >= b.0 - 1.0
                && r.y >= b.1 - 1.0
                && r.x + r.width <= b.0 + b.2 + 1.0
                && r.y + r.height <= b.1 + b.3 + 1.0
        })
}

struct Src {
    versions: HashMap<String, u64>,
    bodies: HashMap<String, String>,
}

impl Src {
    fn new(slices: &[&str]) -> Self {
        let mut versions: HashMap<String, u64> =
            slices.iter().map(|s| (s.to_string(), 1)).collect();
        versions.insert("actions".into(), 1);
        Self {
            versions,
            bodies: HashMap::new(),
        }
    }
    fn set(&mut self, slice: &str, section: &str, html: &str) {
        *self.versions.entry(slice.into()).or_insert(0) += 1;
        self.bodies.insert(section.into(), html.into());
    }
}

impl SliceSource for Src {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
}

impl FragmentSource for Src {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        self.bodies.get(section_id).cloned().unwrap_or_default()
    }
}

fn sync(eng: &mut Engine, src: &mut Src) {
    let slices = Src {
        versions: src.versions.clone(),
        bodies: HashMap::new(),
    };
    eng.sync(&slices, src).expect("sync");
}

fn rows(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!(r#"<button id="{prefix}{i}" class="r" hx-post="act.do" hx-trigger="click">Row {i}</button>"#))
        .collect()
}

fn small_vp() -> Viewport {
    Viewport::with_scale(400, 300, 1.0)
}

/// Small page: `#box` (rows, swapped outerHTML), `#log` (text only, no
/// element children, swapped innerHTML), `#fixed` (`overflow-y: scroll`
/// with content that fits).
fn small_page(box_rows: usize) -> String {
    let lines: String = (0..20).map(|i| format!("log line {i}\n")).collect();
    format!(
        r##"<!DOCTYPE html><html><head><style>
* {{ margin:0; padding:0; box-sizing:border-box; }}
body {{ width:400px; height:300px; overflow:hidden; background:#fff; font-family:"DejaVu Sans",sans-serif; font-size:12px; }}
.sc {{ position:absolute; overflow-y:auto; background:#e8e8e8; }}
#box {{ left:10px; top:10px; width:180px; height:120px; }}
.r {{ display:block; width:100%; height:20px; border:0; background:#ccd; text-align:left; font-size:12px; }}
.r:nth-child(odd) {{ background:#99a; }}
#log {{ left:210px; top:10px; width:180px; height:100px; white-space:pre; line-height:20px; color:#000; }}
#fixed {{ position:absolute; left:210px; top:150px; width:180px; height:100px; overflow-y:scroll; background:#eee; }}
#out {{ position:absolute; left:10px; top:250px; width:100px; height:20px; }}
</style></head><body>
<section id="sec-box" hx-trigger="slice:box" hx-target="#box" hx-swap="outerHTML"><div id="box" class="sc">{rows}</div></section>
<section id="sec-log" hx-trigger="slice:log" hx-target="#log" hx-swap="innerHTML"><div id="log" class="sc">{lines}</div></section>
<div id="fixed">fits</div>
<button id="out" hx-post="act.do" hx-trigger="click">Out</button>
</body></html>"##,
        rows = rows("b", box_rows),
    )
}

// ---------------------------------------------------------------------------
// Mock 34 (Settings, Advanced): geometry, wheel, clamp, damage, clicks.

#[test]
fn mock34_panel_scrolls_with_proportional_win95_bar() {
    let mut eng = boot34();
    let (_, img) = frame(&mut eng, vp34());
    let pb = bx(&eng, PANEL);
    assert_eq!(top(&eng, PANEL), 0.0);
    let m = max(&eng, PANEL);
    assert!(
        m > 100.0,
        "Advanced content must overflow the 480x356 window: max {m}"
    );

    let (upr, dnr, track, thumb) = eng.test_scrollbar(PANEL).expect("bar shown");
    // 16 px bar flush with the panel's right padding edge, full height.
    assert_eq!(upr, (pb.0 + pb.2 - 16.0, pb.1, 16.0, 16.0));
    assert_eq!(dnr, (pb.0 + pb.2 - 16.0, pb.1 + pb.3 - 16.0, 16.0, 16.0));
    assert_eq!(track.3, pb.3 - 32.0);
    let thumb = thumb.expect("thumb");
    let want = (track.3 * pb.3 / (pb.3 + m)).round();
    assert_eq!(thumb.3, want, "thumb sized to the visible share");
    assert_eq!(thumb.1, track.1, "thumb at top when offset is 0");

    // Pixels: raised bevel (--raise), ink triangle, 1 px checker track.
    assert_eq!(
        px(&img, upr.0, upr.1),
        [0xFF, 0xFF, 0xFF, 0xFF],
        "up arrow top-left = --hi"
    );
    assert_eq!(
        px(&img, upr.0 + 15.0, upr.1 + 15.0),
        [0x0A, 0x0A, 0x0A, 0xFF],
        "bottom-right = --dk"
    );
    assert_eq!(
        px(&img, upr.0 + 14.0, upr.1 + 14.0),
        [0x80, 0x80, 0x80, 0xFF],
        "inner = --sh"
    );
    assert_eq!(
        px(&img, upr.0 + 1.0, upr.1 + 1.0),
        [0xDF, 0xDF, 0xDF, 0xFF],
        "inner = --panel"
    );
    assert_eq!(
        px(&img, upr.0 + 7.0, upr.1 + 6.0),
        [0, 0, 0, 0xFF],
        "up triangle apex"
    );
    assert_eq!(
        px(&img, dnr.0 + 7.0, dnr.1 + 9.0),
        [0, 0, 0, 0xFF],
        "down triangle apex"
    );
    let ty = thumb.1 + thumb.3 + 4.0;
    let a = px(&img, track.0 + 4.0, ty);
    let b = px(&img, track.0 + 5.0, ty);
    let c = px(&img, track.0 + 5.0, ty + 1.0);
    assert_ne!(a, b, "checker alternates along x");
    assert_eq!(a, c, "checker alternates along y");
    assert!(
        [a, b].contains(&[0xFF, 0xFF, 0xFF, 0xFF]) && [a, b].contains(&[0xC0, 0xC0, 0xC0, 0xFF])
    );
    assert_eq!(
        px(&img, thumb.0, thumb.1),
        [0xFF, 0xFF, 0xFF, 0xFF],
        "thumb raised"
    );
    assert_eq!(
        px(&img, thumb.0 + 8.0, thumb.1 + 8.0),
        [0xC0, 0xC0, 0xC0, 0xFF],
        "thumb face --bg"
    );
}

#[test]
fn wheel_scrolls_innermost_and_clamps_at_both_ends() {
    let mut eng = boot34();
    let (x, y) = center(&eng, PANEL);
    let m = max(&eng, PANEL);

    assert_eq!(
        eng.input(wheel(x, y, -1.0, WheelUnit::Line)),
        scrolled(PANEL)
    );
    assert_eq!(
        top(&eng, PANEL),
        60.0,
        "one line = 3 rows of 20 px; negative y scrolls down"
    );
    assert_eq!(
        eng.input(wheel(x, y, 7.0, WheelUnit::Pixel)),
        scrolled(PANEL)
    );
    assert_eq!(top(&eng, PANEL), 53.0);
    assert_eq!(
        eng.input(wheel(x, y, -1.0, WheelUnit::Page)),
        scrolled(PANEL)
    );
    assert_eq!(top(&eng, PANEL), (53.0 + bx(&eng, PANEL).3 - 20.0).min(m));

    for _ in 0..20 {
        let _ = eng.input(wheel(x, y, -1.0, WheelUnit::Line));
    }
    assert_eq!(top(&eng, PANEL), m, "clamped at max");
    assert_eq!(
        eng.input(wheel(x, y, -1.0, WheelUnit::Line)),
        InputOutcome::None,
        "nothing can move: None lets the host scroll"
    );
    for _ in 0..20 {
        let _ = eng.input(wheel(x, y, 3.0, WheelUnit::Line));
    }
    assert_eq!(top(&eng, PANEL), 0.0, "clamped at 0");
    assert_eq!(
        eng.input(wheel(x, y, 1.0, WheelUnit::Line)),
        InputOutcome::None
    );

    // Outside any scroll container (title bar): None, nothing moves.
    let (tx, ty) = center(&eng, "set-tabs");
    assert_eq!(
        eng.input(wheel(tx, ty, -1.0, WheelUnit::Line)),
        InputOutcome::None
    );
    // Horizontal-only delta is ignored.
    assert_eq!(
        eng.input(InputEvent::Wheel {
            pos: (x, y),
            delta: (-5.0, 0.0),
            unit: WheelUnit::Pixel
        }),
        InputOutcome::None
    );
    assert_eq!(top(&eng, PANEL), 0.0);
}

#[test]
fn wheel_damage_stays_in_panel_and_partial_equals_full() {
    let mut eng = boot34();
    let (_, before) = frame(&mut eng, vp34());
    let pb = bx(&eng, PANEL);
    let (x, y) = center(&eng, PANEL);
    for step in 0..3 {
        assert_eq!(
            eng.input(wheel(x, y, -1.0, WheelUnit::Line)),
            scrolled(PANEL)
        );
        let (out, img) = frame(&mut eng, vp34());
        assert!(
            damage_inside(&out, pb),
            "step {step}: damage {:?} must stay inside the panel {pb:?}",
            out.damage
        );
        for id in ["set-tabs", "set-apply", "set-close"] {
            let b = bx(&eng, id);
            assert_eq!(region(&img, b), region(&before, b), "{id} pixels unchanged");
        }
        let full = full_frame(&mut eng, vp34());
        assert!(
            img.pixels == full.pixels,
            "step {step}: partial repaint must equal full"
        );
    }
    let (_, after) = frame(&mut eng, vp34());
    assert_ne!(
        region(&after, pb),
        region(&before, pb),
        "panel content moved"
    );
}

#[test]
fn clicks_hit_visible_rows_only() {
    let mut eng = boot34();
    let (rx, ry) = center(&eng, "set-engine-reset");
    let pb = bx(&eng, PANEL);
    assert!(ry > pb.1 + pb.3, "reset button starts scrolled out of view");
    // Clicking where the hidden button would be must not press it.
    let _ = eng.input(down(rx, ry));
    let out = eng.input(up(rx, ry));
    assert!(
        !matches!(&out, InputOutcome::Action { name, .. } if name == "settings.engine.reset"),
        "clipped content must not take a click: {out:?}"
    );

    let (x, y) = center(&eng, PANEL);
    while eng.input(wheel(x, y, -1.0, WheelUnit::Line)) != InputOutcome::None {}
    let _ = frame(&mut eng, vp34());
    let (rx, ry) = center(&eng, "set-engine-reset");
    assert!(ry > pb.1 && ry < pb.1 + pb.3, "now visible");
    let _ = eng.input(down(rx, ry));
    let out = eng.input(up(rx, ry));
    assert!(
        matches!(&out, InputOutcome::Action { name, .. } if name == "settings.engine.reset"),
        "{out:?}"
    );
}

#[test]
fn arrows_track_and_thumb_drag() {
    let mut eng = boot34();
    let m = max(&eng, PANEL);
    let ph = bx(&eng, PANEL).3;
    let (upr, dnr, track, thumb) = eng.test_scrollbar(PANEL).unwrap();
    let thumb = thumb.unwrap();

    // Down arrow: one row, pressed bevel until release, release never clicks.
    let (dx, dy) = (dnr.0 + 8.0, dnr.1 + 8.0);
    assert_eq!(eng.input(down(dx, dy)), scrolled(PANEL));
    assert_eq!(top(&eng, PANEL), 20.0);
    let (_, img) = frame(&mut eng, vp34());
    assert_eq!(
        px(&img, dnr.0, dnr.1),
        [0x0A, 0x0A, 0x0A, 0xFF],
        "pressed: --press top-left = --dk"
    );
    assert_eq!(eng.input(up(dx, dy)), InputOutcome::None);
    let (_, img) = frame(&mut eng, vp34());
    assert_eq!(
        px(&img, dnr.0, dnr.1),
        [0xFF, 0xFF, 0xFF, 0xFF],
        "released: raised again"
    );
    // Up arrow.
    let _ = eng.input(down(upr.0 + 8.0, upr.1 + 8.0));
    let _ = eng.input(up(upr.0 + 8.0, upr.1 + 8.0));
    assert_eq!(top(&eng, PANEL), 0.0);

    // Track below the thumb: one page (visible height minus a row).
    let ty = track.1 + track.3 - 4.0;
    assert_eq!(eng.input(down(track.0 + 8.0, ty)), scrolled(PANEL));
    let _ = eng.input(up(track.0 + 8.0, ty));
    assert_eq!(top(&eng, PANEL), (ph - 20.0).min(m));
    // Track above the thumb: back one page.
    let (_, _, _, t2) = eng.test_scrollbar(PANEL).unwrap();
    let t2 = t2.unwrap();
    assert!(t2.1 > track.1 + 2.0);
    let _ = eng.input(down(track.0 + 8.0, track.1 + 1.0));
    let _ = eng.input(up(track.0 + 8.0, track.1 + 1.0));
    assert_eq!(top(&eng, PANEL), 0.0);

    // Thumb drag: offset follows the thumb top proportionally.
    let (gx, gy) = (thumb.0 + 8.0, thumb.1 + 30.0);
    let _ = eng.input(down(gx, gy));
    let travel = track.3 - thumb.3;
    assert_eq!(eng.input(mv(gx, gy + 19.0)), scrolled(PANEL));
    let want = 19.0 / travel * m;
    assert!(
        (top(&eng, PANEL) - want).abs() < 0.01,
        "{} vs {want}",
        top(&eng, PANEL)
    );
    let (_, _, _, t3) = eng.test_scrollbar(PANEL).unwrap();
    assert_eq!(
        t3.unwrap().1,
        (track.1 + travel * top(&eng, PANEL) / m).round()
    );
    let _ = eng.input(mv(gx + 200.0, gy + 1000.0));
    assert_eq!(top(&eng, PANEL), m, "drag past the end clamps");
    let _ = eng.input(mv(gx, gy - 1000.0));
    assert_eq!(top(&eng, PANEL), 0.0);
    let _ = eng.input(mv(gx, gy + 10.0));
    let mid = top(&eng, PANEL);
    assert_eq!(
        eng.input(up(gx, gy + 10.0)),
        InputOutcome::None,
        "drag release never clicks"
    );
    // After release, moves no longer drag.
    let _ = eng.input(mv(gx, gy + 80.0));
    assert_eq!(top(&eng, PANEL), mid);

    let (_, img) = frame(&mut eng, vp34());
    let full = full_frame(&mut eng, vp34());
    assert!(
        img.pixels == full.pixels,
        "partial == full after bar interaction"
    );
}

#[test]
fn page_home_end_keys_need_focus_inside() {
    let mut eng = boot34();
    let m = max(&eng, PANEL);
    // No focus: keys are not scroll keys.
    assert!(matches!(
        eng.input(key("PageDown")),
        InputOutcome::Unhandled(_)
    ));
    assert_eq!(top(&eng, PANEL), 0.0);
    // Tab until focus is inside the panel.
    for _ in 0..10 {
        let _ = eng.input(key("Tab"));
        if eng.focus_state().focused_id.as_deref() == Some("set-dev-tools") {
            break;
        }
    }
    assert_eq!(
        eng.focus_state().focused_id.as_deref(),
        Some("set-dev-tools")
    );
    assert_eq!(eng.input(key("End")), scrolled(PANEL));
    assert_eq!(top(&eng, PANEL), m);
    assert_eq!(eng.input(key("Home")), scrolled(PANEL));
    assert_eq!(top(&eng, PANEL), 0.0);
    assert_eq!(eng.input(key("PageDown")), scrolled(PANEL));
    assert_eq!(top(&eng, PANEL), (bx(&eng, PANEL).3 - 20.0).min(m));
    assert_eq!(eng.input(key("PageUp")), scrolled(PANEL));
    assert_eq!(top(&eng, PANEL), 0.0);
    assert!(
        matches!(eng.input(key("PageUp")), InputOutcome::Unhandled(_)),
        "nothing to move"
    );
}

#[test]
fn tab_scrolls_focus_into_view() {
    let mut eng = boot34();
    let pb = bx(&eng, PANEL);
    for _ in 0..40 {
        let _ = eng.input(key("Tab"));
        if eng.focus_state().focused_id.as_deref() == Some("set-engine-reset") {
            break;
        }
    }
    assert_eq!(
        eng.focus_state().focused_id.as_deref(),
        Some("set-engine-reset")
    );
    let b = bx(&eng, "set-engine-reset");
    assert!(
        b.1 >= pb.1 && b.1 + b.3 <= pb.1 + pb.3,
        "{b:?} inside {pb:?}"
    );
    assert_eq!(top(&eng, PANEL), max(&eng, PANEL));
    // Shift+Tab back to the first knob scrolls back up ("nearest").
    for _ in 0..40 {
        let _ = eng.input(InputEvent::Key(KeyEvent {
            key: "Tab".into(),
            code: "Tab".into(),
            mods: Modifiers {
                shift: true,
                ..Default::default()
            },
            repeat: false,
            pressed: true,
        }));
        if eng.focus_state().focused_id.as_deref() == Some("set-dev-tools") {
            break;
        }
    }
    let b = bx(&eng, "set-dev-tools");
    assert!(b.1 >= pb.1, "{b:?} inside {pb:?}");
    let (_, img) = frame(&mut eng, vp34());
    let full = full_frame(&mut eng, vp34());
    assert!(img.pixels == full.pixels);
}

// ---------------------------------------------------------------------------
// Swaps: keep by id, clamp, reset on a new id.

#[test]
fn panel_swap_keeps_offset_by_id_and_resets_on_new_id() {
    let mut eng = boot34();
    let mut src = Src::new(&["settings"]);
    src.bodies.insert("set-panel".into(), ADV_PANEL.into());
    sync(&mut eng, &mut src);
    let _ = frame(&mut eng, vp34());
    let (x, y) = center(&eng, PANEL);
    let _ = eng.input(wheel(x, y, -1.0, WheelUnit::Line));
    let _ = eng.input(wheel(x, y, -1.0, WheelUnit::Line));
    assert_eq!(top(&eng, PANEL), 120.0);

    // Same panel re-rendered (slice tick): `#set-panel` innerHTML replaces the
    // `#set-panel-advanced` element with a new one of the same id.
    src.set("settings", "set-panel", ADV_PANEL);
    sync(&mut eng, &mut src);
    let (_, img) = frame(&mut eng, vp34());
    assert_eq!(
        top(&eng, PANEL),
        120.0,
        "offset kept across the swap (same id)"
    );
    let full = full_frame(&mut eng, vp34());
    assert!(
        img.pixels == full.pixels,
        "partial == full after a scrolled swap"
    );

    // Shorter content, same id: clamped to the new max.
    let short = ADV_PANEL.replacen("<div class=\"sub\">", "<div hidden class=\"sub\">", 1);
    let cut = short.find("<div hidden").unwrap();
    let short = format!("{}</div>\n", &short[..cut]);
    src.set("settings", "set-panel", &short);
    sync(&mut eng, &mut src);
    let (_, img) = frame(&mut eng, vp34());
    let m = max(&eng, PANEL);
    assert!(m < 120.0, "shorter content: max {m}");
    assert_eq!(top(&eng, PANEL), m, "clamped to the new content height");
    let full = full_frame(&mut eng, vp34());
    assert!(img.pixels == full.pixels);

    // Another tab (new id): starts at 0.
    let display = ADV_PANEL.replace(PANEL, "set-panel-display");
    src.set("settings", "set-panel", &display);
    sync(&mut eng, &mut src);
    let _ = frame(&mut eng, vp34());
    assert_eq!(top(&eng, "set-panel-display"), 0.0, "new id starts at 0");
}

#[test]
fn inner_and_outer_swaps_on_the_container_itself() {
    let html = small_page(30);
    let mut eng = boot_with(&html, small_vp());
    let mut src = Src::new(&["box", "log"]);
    let lines0: String = (0..20).map(|i| format!("log line {i}\n")).collect();
    src.bodies.insert("sec-log".into(), lines0);
    src.bodies.insert(
        "sec-box".into(),
        format!(r#"<div id="box" class="sc">{}</div>"#, rows("b", 30)),
    );
    sync(&mut eng, &mut src);
    let _ = frame(&mut eng, small_vp());
    let (bxc, byc) = center(&eng, "box");
    let (lx, ly) = center(&eng, "log");
    let _ = eng.input(wheel(bxc, byc, -100.0, WheelUnit::Pixel));
    let _ = eng.input(wheel(lx, ly, -150.0, WheelUnit::Pixel));
    assert_eq!(top(&eng, "box"), 100.0);
    assert_eq!(top(&eng, "log"), 150.0);
    let _ = frame(&mut eng, small_vp());

    // innerHTML on the container itself: same node, offset kept.
    let lines: String = (0..20).map(|i| format!("new line {i}\n")).collect();
    src.set("log", "sec-log", &lines);
    // outerHTML with the same id: kept.
    src.set(
        "box",
        "sec-box",
        &format!(r#"<div id="box" class="sc">{}</div>"#, rows("c", 30)),
    );
    sync(&mut eng, &mut src);
    let (_, img) = frame(&mut eng, small_vp());
    assert_eq!(top(&eng, "log"), 150.0);
    assert_eq!(top(&eng, "box"), 100.0);
    let full = full_frame(&mut eng, small_vp());
    assert!(img.pixels == full.pixels);

    // innerHTML shorter: clamped. outerHTML with a new id: 0.
    let lines: String = (0..8).map(|i| format!("short {i}\n")).collect();
    src.set("log", "sec-log", &lines);
    src.set("box", "sec-box", &format!(r#"<div id="box2" class="sc" style="left:10px;top:10px;width:180px;height:120px">{}</div>"#, rows("d", 30)));
    sync(&mut eng, &mut src);
    let (_, img) = frame(&mut eng, small_vp());
    assert_eq!(top(&eng, "log"), 60.0, "8 lines x 20 px - 100 px");
    assert_eq!(max(&eng, "log"), 60.0);
    assert_eq!(top(&eng, "box2"), 0.0);
    let full = full_frame(&mut eng, small_vp());
    assert!(img.pixels == full.pixels);
}

#[test]
fn text_only_container_scroll_repaints_its_box() {
    // No element children move, so only the container damage covers it.
    let html = small_page(3);
    let mut eng = boot_with(&html, small_vp());
    let lb = bx(&eng, "log");
    let (x, y) = center(&eng, "log");
    for _ in 0..3 {
        assert_eq!(
            eng.input(wheel(x, y, -1.0, WheelUnit::Line)),
            scrolled("log")
        );
        let (out, img) = frame(&mut eng, small_vp());
        assert!(damage_inside(&out, lb), "{:?}", out.damage);
        let full = full_frame(&mut eng, small_vp());
        assert!(img.pixels == full.pixels, "partial repaint must equal full");
    }
}

#[test]
fn bar_hidden_when_content_fits_and_disabled_for_overflow_scroll() {
    let html = small_page(3);
    let mut eng = boot_with(&html, small_vp());
    assert_eq!(max(&eng, "box"), 0.0);
    assert!(eng.test_scrollbar("box").is_none(), "auto + fits: no bar");
    let (_, _, _, thumb) = eng
        .test_scrollbar("fixed")
        .expect("overflow:scroll always shows a bar");
    assert!(thumb.is_none(), "nothing to scroll: no thumb");
    let (x, y) = center(&eng, "fixed");
    assert_eq!(
        eng.input(wheel(x, y, -1.0, WheelUnit::Line)),
        InputOutcome::None
    );
    let (bxc, byc) = center(&eng, "box");
    assert_eq!(
        eng.input(wheel(bxc, byc, -1.0, WheelUnit::Line)),
        InputOutcome::None
    );
    // Clicks on rows still work (no scrollbar eats them).
    let (rx, ry) = center(&eng, "b2");
    let _ = eng.input(down(rx, ry));
    assert!(matches!(eng.input(up(rx, ry)), InputOutcome::Action { .. }));
}

#[test]
fn paint_worker_matches_sync_engine_when_scrolling() {
    let steps: Vec<Vec<InputEvent>> = {
        let eng = boot34();
        let (x, y) = center(&eng, PANEL);
        let (_, dnr, _, thumb) = eng.test_scrollbar(PANEL).unwrap();
        let t = thumb.unwrap();
        vec![
            vec![wheel(x, y, -1.0, WheelUnit::Line)],
            vec![down(dnr.0 + 8.0, dnr.1 + 8.0)],
            vec![up(dnr.0 + 8.0, dnr.1 + 8.0)],
            vec![down(t.0 + 8.0, t.1 + 70.0), mv(t.0 + 8.0, t.1 + 90.0)],
            vec![up(t.0 + 8.0, t.1 + 90.0)],
            vec![wheel(x, y, 10.0, WheelUnit::Pixel)],
        ]
    };
    let mut eng = boot34();
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(ACTIONS).unwrap();
    worker.load_html(MOCK34).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, vp34()).unwrap();
    worker.flush_timeout(Duration::from_secs(10)).unwrap();
    for (i, step) in steps.iter().enumerate() {
        for ev in step {
            let _ = eng.input(ev.clone());
            worker.input(ev.clone()).unwrap();
        }
        let (_, img) = frame(&mut eng, vp34());
        worker.flush_timeout(Duration::from_secs(10)).unwrap();
        let wf = worker
            .latest_frame(SurfaceId::DEFAULT, 0)
            .expect("worker frame");
        let wimg = wf.output.rgba.as_ref().expect("worker rgba");
        assert!(
            wimg.pixels == img.pixels,
            "step {i}: worker vs sync must be byte-identical"
        );
    }
    assert!(top(&eng, PANEL) > 0.0);
}

#[test]
fn select_face_inside_scroller_is_clipped() {
    let html = r##"<!DOCTYPE html><html><head><style>
* { margin:0; padding:0; box-sizing:border-box; }
body { width:400px; height:300px; overflow:hidden; background:#fff; font-family:"DejaVu Sans",sans-serif; font-size:12px; }
#sc { position:absolute; left:20px; top:40px; width:200px; height:100px; overflow-y:auto; background:#ddd; }
#pick { display:block; width:150px; height:22px; margin-top:4px; }
.fill { height:300px; }
</style></head><body>
<div id="sc"><select id="pick" name="p"><option>One</option><option>Two</option></select><div class="fill"></div></div>
</body></html>"##;
    let mut eng = boot_with(html, small_vp());
    let (_, before) = frame(&mut eng, small_vp());
    let sc = bx(&eng, "sc");
    let above = (sc.0, sc.1 - 12.0, sc.2, 12.0);
    let (x, y) = center(&eng, "sc");
    assert_eq!(
        eng.input(wheel(x, y, -14.0, WheelUnit::Pixel)),
        scrolled("sc")
    );
    let (_, img) = frame(&mut eng, small_vp());
    let pick = bx(&eng, "pick");
    assert!(
        pick.1 < sc.1,
        "select face now straddles the container top: {pick:?}"
    );
    assert_eq!(
        region(&img, above),
        region(&before, above),
        "face never paints outside the container"
    );
    let full = full_frame(&mut eng, small_vp());
    assert!(img.pixels == full.pixels);
}

#[test]
fn nested_swap_shrinking_content_clamps_the_container() {
    // The swap target is *inside* the container, so nothing is recorded for
    // the container itself: only the after-layout clamp brings it back.
    let page = |n: usize| {
        format!(
            r##"<!DOCTYPE html><html><head><style>
* {{ margin:0; padding:0; box-sizing:border-box; }}
body {{ width:400px; height:300px; overflow:hidden; background:#fff; font-family:"DejaVu Sans",sans-serif; font-size:12px; }}
#sc2 {{ position:absolute; left:10px; top:10px; width:200px; height:100px; overflow-y:auto; background:#eee; }}
.r {{ display:block; width:100%; height:20px; border:0; background:#ccd; }}
.r:nth-child(odd) {{ background:#99a; }}
</style></head><body>
<div id="sc2"><section id="sec-in" hx-trigger="slice:in" hx-target="#in" hx-swap="innerHTML"><div id="in">{}</div></section></div>
</body></html>"##,
            rows("r", n)
        )
    };
    let mut eng = boot_with(&page(20), small_vp());
    let mut src = Src::new(&["in"]);
    src.bodies.insert("sec-in".into(), rows("r", 20));
    sync(&mut eng, &mut src);
    let _ = frame(&mut eng, small_vp());
    let (x, y) = center(&eng, "sc2");
    let _ = eng.input(wheel(x, y, -250.0, WheelUnit::Pixel));
    assert_eq!(top(&eng, "sc2"), 250.0);
    let _ = frame(&mut eng, small_vp());
    src.set("in", "sec-in", &rows("s", 8));
    sync(&mut eng, &mut src);
    let (_, img) = frame(&mut eng, small_vp());
    assert_eq!(max(&eng, "sc2"), 60.0);
    assert_eq!(top(&eng, "sc2"), 60.0, "clamped to the shorter content");
    let full = full_frame(&mut eng, small_vp());
    assert!(img.pixels == full.pixels);
}
