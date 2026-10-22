//! Step 1 (issue #8): partial repaint — byte-identical vs full, damage locality,
//! resize full-repaint, and batched style/layout.

#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hx_native::*;

fn boot(html: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(&[
        "time.pause",
        "time.resume",
        "time.set_speed",
        "win.open",
        "act.do",
    ]);
    eng.load_html(html).expect("load");
    eng
}

struct MemSlices {
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
}

impl MemSlices {
    fn new() -> Self {
        let mut versions = HashMap::new();
        versions.insert("time".into(), 1);
        versions.insert("actions".into(), 1);
        versions.insert("hud".into(), 1);
        versions.insert("ticker".into(), 1);
        versions.insert("glog".into(), 1);
        Self {
            versions,
            actions: Vec::new(),
        }
    }
    fn bump(&mut self, name: &str) {
        *self.versions.entry(name.into()).or_insert(0) += 1;
    }
}

impl SliceSource for MemSlices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
}

struct SpyFrags {
    calls: Arc<Mutex<usize>>,
    bodies: HashMap<String, String>,
}

impl SpyFrags {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(0)),
            bodies: HashMap::new(),
        }
    }
    fn set(&mut self, section: &str, html: &str) {
        self.bodies.insert(section.into(), html.into());
    }
}

impl FragmentSource for SpyFrags {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        *self.calls.lock().unwrap() += 1;
        self.bodies
            .get(section_id)
            .cloned()
            .unwrap_or_else(|| format!("<!-- {section_id} -->"))
    }
}

/// Page with a ticker lane, chips, and a toggled window — mirrors the probe scenarios.
fn script_page() -> String {
    r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* { box-sizing: border-box; margin: 0; padding: 0; }
body { width: 800px; height: 400px; background: #1a1f2a; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; }
#strip { position: absolute; left: 0; top: 0; width: 800px; height: 80px; background: #222833; }
#chips { position: absolute; left: 8px; top: 8px; height: 36px; }
#chips button { height: 32px; margin-right: 4px; padding: 0 10px; background: #2d3648;
  color: #e8ecf4; border: 1px solid #556; position: relative; }
#chips button.on { background: #3d7ea6; }
#chips button[data-hx-reason]:hover::after {
  content: attr(data-hx-reason); position: absolute; left: 0; top: 36px;
  white-space: nowrap; color: #000; background: #ffffe1; border: 1px solid #000; z-index: 5;
}
#date { position: absolute; left: 8px; top: 48px; font-size: 14px; height: 20px; }
#ticker-lane {
  position: absolute; left: 0; top: 360px; width: 800px; height: 32px;
  overflow: hidden; background: #141820;
}
#ticker-inner { position: absolute; left: 0; top: 4px; white-space: nowrap; font-size: 14px; }
#win {
  position: absolute; left: 40px; top: 100px; width: 420px; height: 200px;
  background: #2a3140; border: 1px solid #667; box-shadow: 8px 8px 16px rgba(0,0,0,0.45);
  display: none;
}
#win.open { display: block; }
#win table { width: 100%; border-collapse: collapse; font-size: 13px; }
#win td { padding: 4px 8px; border-bottom: 1px solid #445; }
#act { position: absolute; left: 600px; top: 20px; width: 120px; height: 32px;
  background: #2d3648; color: #e8ecf4; border: 1px solid #556; }
#act:disabled { opacity: 0.4; }
#log { position: absolute; left: 600px; top: 100px; width: 180px; height: 120px;
  overflow-y: auto; background: #202634; font-size: 13px; line-height: 20px; }
#glog { position: absolute; left: 600px; top: 240px; width: 180px; height: 100px;
  overflow-y: auto; background: #2a3140; font-size: 13px; line-height: 20px; padding-right: 4px; }
#glog div { background: #3d7ea6; }
#vol { position: absolute; left: 40px; top: 320px; width: 200px; height: 20px; }
#num { position: absolute; left: 260px; top: 320px; width: 60px; height: 20px; padding: 2px 6px;
  line-height: 16px; text-align: right; background: #fff; color: #000; border: 0; font-size: 11px;
  box-shadow: inset -1px -1px #fff, inset 1px 1px #808080, inset -2px -2px #dfdfdf, inset 2px 2px #0a0a0a; }
</style></head><body>
<section id="sec-chips" hx-trigger="slice:time" hx-target="#chips" hx-swap="innerHTML">
  <div id="chips"></div>
</section>
<section id="sec-date" hx-trigger="slice:time" hx-target="#date" hx-swap="innerHTML">
  <div id="date">DATE</div>
</section>
<section id="sec-ticker" hx-trigger="slice:ticker" hx-target="#ticker-inner" hx-swap="innerHTML">
  <div id="ticker-lane"><div id="ticker-inner">news</div></div>
</section>
<section id="sec-win" hx-trigger="slice:hud" hx-target="#win" hx-swap="innerHTML">
  <div id="win"></div>
</section>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
<section id="sec-glog" hx-trigger="slice:glog" hx-target="#glog" hx-swap="innerHTML">
  <div id="glog"></div>
</section>
<input id="vol" type="range" name="vol" value="20" min="0" max="100" step="5">
<input id="num" type="number" name="num" value="42">
<div id="log">"##
        .to_string()
        + &(0..15)
            .map(|i| format!("<div id=\"log-{i}\">Log entry {i}</div>"))
            .collect::<String>()
        + r##"</div>
</body></html>"##
}

fn chips_html(active: &str) -> String {
    let btns = ["1x", "2x", "Week"];
    let mut s = String::new();
    for b in btns {
        let on = if b == active { " on" } else { "" };
        s.push_str(&format!(
            r#"<button id="chip-{b}" class="{on}" hx-post="time.set_speed" hx-trigger="click">{b}</button>"#
        ));
    }
    s
}

fn date_html(n: u32) -> String {
    format!(r#"<span id="date-label">DATE 2026-01-{n:02}</span>"#)
}

fn ticker_html(offset_px: i32) -> String {
    format!(
        r#"<span id="ticker-text" style="margin-left:{offset_px}px">Breaking · Week tick · Sim update · News crawl</span>"#
    )
}

fn win_html(open: bool) -> String {
    let cls = if open { "open" } else { "" };
    format!(
        r#"<div id="win-panel" class="{cls}" style="position:absolute;left:40px;top:100px;width:420px;height:200px;background:#2a3140;border:1px solid #667;box-shadow:8px 8px 16px rgba(0,0,0,0.45);{disp}">
<table><tr><td id="cell-a">Alpha</td><td>1</td></tr>
<tr><td>Beta</td><td id="cell-b">2</td></tr></table></div>"#,
        disp = if open {
            "display:block;"
        } else {
            "display:none;"
        },
        cls = cls,
    )
}

/// Physical viewport for a page authored at `css_w`×`css_h` CSS pixels.
fn vp_css(css_w: u32, css_h: u32, scale: f64) -> Viewport {
    let w = ((css_w as f64) * scale).round() as u32;
    let h = ((css_h as f64) * scale).round() as u32;
    Viewport::with_scale(w, h, scale)
}

fn full_surface_damage(out: &PaintOutput, w: u32, h: u32) -> bool {
    out.damage.len() == 1
        && out.damage[0].x == 0.0
        && out.damage[0].y == 0.0
        && out.damage[0].width == w as f32
        && out.damage[0].height == h as f32
}

fn damage_union(out: &PaintOutput) -> Option<(f32, f32, f32, f32)> {
    let mut iter = out.damage.iter();
    let first = iter.next()?;
    let mut x0 = first.x;
    let mut y0 = first.y;
    let mut x1 = first.x + first.width;
    let mut y1 = first.y + first.height;
    for r in iter {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.width);
        y1 = y1.max(r.y + r.height);
    }
    Some((x0, y0, x1, y1))
}

#[test]
fn n_swaps_no_hover_change_exactly_one_style_layout_pass() {
    let mut eng = boot(&script_page());
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("sec-chips", &chips_html("1x"));
    frags.set("sec-date", &date_html(1));
    frags.set("sec-ticker", &ticker_html(0));
    frags.set("sec-win", &win_html(false));

    eng.sync(&slices, &mut frags).unwrap();
    let before = eng.style_layout_pass_count();
    eng.paint(vp_css(800, 400, 1.0)).unwrap();
    let after_first = eng.style_layout_pass_count();
    assert!(after_first > before, "first paint must resolve");

    // One tick: N section swaps, no pointer hover — exactly one style/layout pass.
    slices.bump("time");
    slices.bump("ticker");
    slices.bump("hud");
    frags.set("sec-chips", &chips_html("Week"));
    frags.set("sec-date", &date_html(2));
    frags.set("sec-ticker", &ticker_html(-40));
    frags.set("sec-win", &win_html(true));
    let dirty = eng.sync(&slices, &mut frags).unwrap();
    assert!(
        dirty.sections.len() >= 3,
        "expected several section swaps, got {:?}",
        dirty.sections
    );
    let mid = eng.style_layout_pass_count();
    assert_eq!(mid, after_first, "sync must not run style/layout");

    eng.paint(vp_css(800, 400, 1.0)).unwrap();
    let end = eng.style_layout_pass_count();
    assert_eq!(
        end,
        mid + 1,
        "N swaps with no hover change must use exactly 1 style/layout pass, mid={mid} end={end}"
    );
}

#[test]
fn swap_moving_hovered_node_at_most_two_style_layout_passes() {
    // Hover a chip, then swap chips under the pointer so the hovered node changes.
    // Graph: follow-up pass only when hover changes, and at most 2 passes total.
    let mut eng = boot(&script_page());
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("sec-chips", &chips_html("1x"));
    frags.set("sec-date", &date_html(1));
    frags.set("sec-ticker", &ticker_html(0));
    frags.set("sec-win", &win_html(false));
    eng.sync(&slices, &mut frags).unwrap();
    let view = vp_css(800, 400, 1.0);
    let _ = eng.paint(view.clone()).unwrap();

    // Hover the first chip (left side of #chips).
    let hover = eng.input(InputEvent::PointerMove {
        x: 40.0,
        y: 20.0,
        mods: Modifiers::default(),
    });
    assert!(
        matches!(hover, InputOutcome::Hover { id: Some(ref id) } if id.starts_with("chip-")),
        "expected chip hover, got {hover:?}"
    );
    let after_hover = eng.style_layout_pass_count();
    eng.paint(view.clone()).unwrap();
    let mid = eng.style_layout_pass_count();
    assert!(
        mid == after_hover + 1 || mid == after_hover + 2,
        "hover paint at most 2 passes, after_hover={after_hover} mid={mid}"
    );

    // Swap chips while pointer stays put — hovered node id is replaced.
    slices.bump("time");
    frags.set("sec-chips", &chips_html("Week"));
    frags.set("sec-date", &date_html(3));
    eng.sync(&slices, &mut frags).unwrap();
    let before_swap_paint = eng.style_layout_pass_count();
    assert_eq!(before_swap_paint, mid, "sync must not run style/layout");
    eng.paint(view).unwrap();
    let end = eng.style_layout_pass_count();
    let passes = end - before_swap_paint;
    assert!(
        (1..=2).contains(&passes),
        "swap under hover must use 1 or 2 passes (hover change allows follow-up), got {passes} (before={before_swap_paint} end={end})"
    );
}

#[test]
fn resize_triggers_full_repaint() {
    let mut eng = boot(&script_page());
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("sec-chips", &chips_html("1x"));
    frags.set("sec-date", &date_html(1));
    frags.set("sec-ticker", &ticker_html(0));
    frags.set("sec-win", &win_html(false));
    eng.sync(&slices, &mut frags).unwrap();

    let first = eng.paint(vp_css(800, 400, 1.0)).unwrap();
    assert!(full_surface_damage(&first, 800, 400));
    assert!(eng.last_paint_stats().full_repaint);

    // Clean frame then resize.
    let skip = eng.paint(vp_css(800, 400, 1.0)).unwrap();
    assert!(skip.skipped);
    assert!(skip.damage.is_empty());
    assert!(skip.rgba.is_none());

    let resized = eng.paint(vp_css(1024, 400, 1.0)).unwrap();
    assert!(!resized.skipped);
    assert!(full_surface_damage(&resized, 1024, 400));
    assert!(eng.last_paint_stats().full_repaint);
}

fn run_scripted_sequence(scale: f64) {
    let mut eng = boot(&script_page());
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("sec-chips", &chips_html("1x"));
    frags.set("sec-date", &date_html(1));
    frags.set("sec-ticker", &ticker_html(0));
    frags.set("sec-win", &win_html(false));
    frags.set("sec-glog", &glog_html(2));
    eng.sync(&slices, &mut frags).unwrap();

    let css_w = 800u32;
    let css_h = 400u32;
    let view = vp_css(css_w, css_h, scale);
    let w = view.width;
    let h = view.height;

    // Step 0: first paint — full.
    let out0 = eng.paint(view.clone()).unwrap();
    assert!(!out0.skipped);
    assert!(full_surface_damage(&out0, w, h));

    // Step 1: ticker crawl — damage stays in the ticker lane.
    slices.bump("ticker");
    frags.set("sec-ticker", &ticker_html(-24));
    eng.sync(&slices, &mut frags).unwrap();
    let tick = eng.paint(view.clone()).unwrap();
    assert!(!tick.skipped);
    assert!(
        !eng.last_paint_stats().full_repaint,
        "ticker crawl must not full-repaint at scale={scale}"
    );
    let (_x0, y0, _x1, y1) = damage_union(&tick).expect("ticker damage");
    // Lane is at y=360 CSS px → device y = 360 * scale; height ~32.
    let lane_top = 360.0 * scale as f32 - 16.0;
    let lane_bot = 400.0 * scale as f32 + 4.0;
    assert!(
        y0 >= lane_top && y1 <= lane_bot,
        "ticker must damage only its lane, got y0={y0} y1={y1} lane_top={lane_top} scale={scale} damage={:?}",
        tick.damage
    );
    assert!(
        y0 > 100.0 * scale as f32,
        "ticker damage must not include the top strip"
    );

    // Byte-identical check after ticker step.
    {
        let mut eng_a = boot(&script_page());
        let mut s_a = MemSlices::new();
        let mut f_a = SpyFrags::new();
        f_a.set("sec-chips", &chips_html("1x"));
        f_a.set("sec-date", &date_html(1));
        f_a.set("sec-ticker", &ticker_html(0));
        f_a.set("sec-win", &win_html(false));
        eng_a.sync(&s_a, &mut f_a).unwrap();
        let _ = eng_a.paint(view.clone()).unwrap();
        s_a.bump("ticker");
        f_a.set("sec-ticker", &ticker_html(-24));
        eng_a.sync(&s_a, &mut f_a).unwrap();
        let partial = eng_a.paint(view.clone()).unwrap();

        let mut eng_b = boot(&script_page());
        let s_b = MemSlices::new();
        let mut f_b = SpyFrags::new();
        f_b.set("sec-chips", &chips_html("1x"));
        f_b.set("sec-date", &date_html(1));
        f_b.set("sec-ticker", &ticker_html(-24));
        f_b.set("sec-win", &win_html(false));
        eng_b.sync(&s_b, &mut f_b).unwrap();
        eng_b.mark_theme_or_stylesheet_changed();
        let full = eng_b.paint(view.clone()).unwrap();
        assert_eq!(
            partial.rgba.as_ref().unwrap().pixels,
            full.rgba.as_ref().unwrap().pixels,
            "ticker step byte-identical at scale={scale}"
        );
    }

    // Step 2: hover chip in and out — damage only old/new hovered nodes.
    let hover_in = eng.input(InputEvent::PointerMove {
        x: 40.0,
        y: 20.0,
        mods: Modifiers::default(),
    });
    assert!(matches!(hover_in, InputOutcome::Hover { .. }));
    // Grey a chip so tooltip paints.
    slices.actions.push((
        "time.set_speed".into(),
        ActionState::Disabled {
            reason: "Sim busy".into(),
        },
    ));
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    let hovered = eng.paint(view.clone()).unwrap();
    assert!(!hovered.skipped);
    if !eng.last_paint_stats().full_repaint {
        let (x0, y0, x1, y1) = damage_union(&hovered).expect("hover damage");
        let area = (x1 - x0) * (y1 - y0);
        let surface = (w as f32) * (h as f32);
        assert!(
            area < surface * 0.25,
            "hover damage should be local, area={area} surface={surface}"
        );
        assert!(y1 < 200.0 * scale as f32, "hover must stay near chips");
    }

    let leave = eng.input(InputEvent::PointerLeave);
    assert!(matches!(leave, InputOutcome::Hover { id: None }));
    let left = eng.paint(view.clone()).unwrap();
    assert!(!left.skipped);

    // Step 3: action greying change.
    slices.actions.clear();
    slices.actions.push((
        "act.do".into(),
        ActionState::Disabled {
            reason: "Blocked".into(),
        },
    ));
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    let grey = eng.paint(view.clone()).unwrap();
    assert!(!grey.skipped);

    // Step 4: chip slice swap.
    slices.bump("time");
    frags.set("sec-chips", &chips_html("Week"));
    frags.set("sec-date", &date_html(3));
    eng.sync(&slices, &mut frags).unwrap();
    let swap = eng.paint(view.clone()).unwrap();
    assert!(!swap.skipped);

    // Step 5: window open during sim tick (hud bump opens window).
    slices.bump("hud");
    frags.set("sec-ticker", &ticker_html(-48));
    frags.set("sec-win", &win_html(true));
    eng.sync(&slices, &mut frags).unwrap();
    let win = eng.paint(view.clone()).unwrap();
    assert!(!win.skipped);

    // Step 6 (issue #26): wheel-scroll the `#log` container — damage stays
    // inside its box, and the final state below must still match a full
    // repaint of a fresh engine scrolled the same way.
    let scrolled = eng.input(log_wheel());
    assert_eq!(
        scrolled,
        InputOutcome::Scrolled {
            id: Some("log".into())
        }
    );
    let sc = eng.paint(view.clone()).unwrap();
    assert!(!sc.skipped);
    assert!(!eng.last_paint_stats().full_repaint);
    let (x0, y0, x1, y1) = damage_union(&sc).expect("scroll damage");
    let s = scale as f32;
    assert!(
        x0 >= 600.0 * s - 1.0
            && y0 >= 100.0 * s - 1.0
            && x1 <= 780.0 * s + 1.0
            && y1 <= 220.0 * s + 1.0,
        "scroll damage must stay in #log at scale={scale}: {:?}",
        sc.damage
    );

    // Step 7 (issue #26 round 3): `#glog` first overflows on a partial
    // frame — the engine reserves its 16 px gutter (one extra layout pass)
    // and damage stays inside the container.
    slices.bump("glog");
    frags.set("sec-glog", &glog_html(12));
    eng.sync(&slices, &mut frags).unwrap();
    let p0 = eng.style_layout_pass_count();
    let gut = eng.paint(view.clone()).unwrap();
    assert!(!gut.skipped);
    assert!(!eng.last_paint_stats().full_repaint);
    assert_eq!(
        eng.style_layout_pass_count() - p0,
        2,
        "one extra pass for the gutter"
    );
    // x stays in the `#glog` column; y may run past it because the swapped
    // section's own ink includes Blitz's scrollable overflow of its subtree
    // (pre-existing over-damage on swaps, never under-damage).
    let (x0, y0, x1, _y1) = damage_union(&gut).expect("gutter damage");
    assert!(
        x0 >= (600.0 - 5.0) * s && y0 >= (240.0 - 5.0) * s && x1 <= (780.0 + 5.0) * s,
        "gutter damage must stay in the #glog column at scale={scale}: {:?}",
        gut.damage
    );
    let g = eng.test_element_box("glog").unwrap();
    let row = eng.test_element_box("glog-0").unwrap();
    assert_eq!(
        row.0 + row.2,
        g.0 + g.2 - 20.0,
        "author 4 px + 16 px gutter"
    );

    // Step 8 (issue #35): drag the engine-drawn slider thumb. Mid-drag and
    // release frames damage only the slider box; the value snaps to step 5.
    // The release at the drag position moves nothing, so it costs no frame.
    let vol = slider_drag(&mut eng, |e| {
        let dr = e.paint(view.clone()).unwrap();
        (!dr.skipped).then(|| {
            assert!(!e.last_paint_stats().full_repaint);
            dr
        })
    });
    assert!(vol[0].is_some(), "mid-drag frame paints");
    assert!(vol[1].is_none(), "an unmoved release skips its frame");
    for out in vol.iter().flatten() {
        let (x0, y0, x1, y1) = damage_union(out).expect("slider damage");
        assert!(
            x0 >= 40.0 * s - 1.0
                && y0 >= 320.0 * s - 1.0
                && x1 <= 240.0 * s + 1.0
                && y1 <= 340.0 * s + 1.0,
            "slider damage must stay in #vol at scale={scale}: {:?}",
            out.damage
        );
    }
    assert_eq!(eng.attr("vol", "value").as_deref(), Some("50"));

    // Final byte-identical: current eng last_frame vs fresh full load of same fragments.
    {
        let expected = eng.last_frame().unwrap().pixels.clone();
        let mut eng_f = boot(&script_page());
        let s_f = MemSlices {
            versions: slices.versions.clone(),
            actions: slices.actions.clone(),
        };
        let mut f_f = SpyFrags::new();
        for (k, v) in frags.bodies.iter() {
            f_f.set(k, v);
        }
        eng_f.sync(&s_f, &mut f_f).unwrap();
        let _ = eng_f.paint(view.clone()).unwrap();
        let _ = eng_f.input(log_wheel());
        let _ = slider_drag(&mut eng_f, |_| ());
        eng_f.mark_theme_or_stylesheet_changed();
        let full = eng_f.paint(view).unwrap();
        assert_eq!(
            expected,
            full.rgba.as_ref().unwrap().pixels,
            "final scripted state byte-identical at scale={scale}"
        );
    }
}

/// Issue #26 round 3: `n` rows for `#glog` (12 overflow its 100 px).
fn glog_html(n: usize) -> String {
    (0..n)
        .map(|i| format!(r#"<div id="glog-{i}">Entry {i}</div>"#))
        .collect()
}

/// Issue #26: one wheel notch down over `#log` (CSS px).
/// Issue #35: press `#vol`'s thumb at its centre, move 60 CSS px right,
/// release; `between` runs after the move and after the release.
fn slider_drag<T>(eng: &mut Engine, mut between: impl FnMut(&mut Engine) -> T) -> Vec<T> {
    let (_c, _t, th, _d) = eng.test_slider("vol").expect("vol slider");
    let (x, y) = (th.0 + th.2 / 2.0, th.1 + th.3 / 2.0);
    let ev = |kind: u8, x: f32| match kind {
        0 => InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        1 => InputEvent::PointerMove {
            x,
            y,
            mods: Modifiers::default(),
        },
        _ => InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
    };
    let _ = eng.input(ev(0, x));
    let _ = eng.input(ev(1, x + 60.0));
    let a = between(eng);
    let _ = eng.input(ev(2, x + 60.0));
    let b = between(eng);
    vec![a, b]
}

fn log_wheel() -> InputEvent {
    InputEvent::Wheel {
        pos: (690.0, 160.0),
        delta: (0.0, -1.0),
        unit: WheelUnit::Line,
    }
}

#[test]
fn scripted_sequence_byte_identical_scale_1() {
    run_scripted_sequence(1.0);
}

#[test]
fn scripted_sequence_byte_identical_scale_1_25() {
    run_scripted_sequence(1.25);
}

#[test]
fn skipped_frame_empty_damage_and_no_rgba() {
    let mut eng = boot(&script_page());
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("sec-chips", &chips_html("1x"));
    frags.set("sec-date", &date_html(1));
    frags.set("sec-ticker", &ticker_html(0));
    frags.set("sec-win", &win_html(false));
    eng.sync(&slices, &mut frags).unwrap();
    let _ = eng.paint(vp_css(800, 400, 1.0)).unwrap();
    let skip = eng.paint(vp_css(800, 400, 1.0)).unwrap();
    assert!(skip.skipped);
    assert!(skip.rgba.is_none());
    assert!(skip.damage.is_empty());
}
