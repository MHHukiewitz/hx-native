//! Week-speed paint bench for issue #8 step 2 (multi-surface).
//!
//! Runs a scripted sequence at 720p, 1080p and 1440p in Exact mode: ticker,
//! hover, action greying, slice swaps, and opening a window during a slice
//! update. Reports mean / p95 / p99 overall and **per event type**, so the
//! slowest 1% can be attributed. The scripted sequence deliberately cold-opens
//! a non-pinned window every 8th frame; a "steady-play" row excludes those.
//!
//! Usage: `cargo run -p hx-native --example paint_bench --release`

use std::collections::HashMap;

use hx_native::*;

const WARMUP: usize = 10;
const FRAMES: usize = 200;

const CHROME: SurfaceId = SurfaceId(1);
const WIN: SurfaceId = SurfaceId(2);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum EventKind {
    /// Ticker / actions / chips slice sync + paint.
    SliceUpdate,
    /// Pointer move or leave on chrome.
    Hover,
    /// `open_surface(visible:true)` of a non-pinned window during a slice tick.
    ColdOpen,
    /// Hide window (`set_visible(false)`); may include eviction under pressure.
    HideWindow,
    /// Dedicated path: open hidden then show (pre-drawn).
    PredrawnShow,
    /// Dedicated path: rebuild after eviction then paint.
    EvictedReopen,
    /// Frame that performed at least one surface eviction (pressure run).
    Eviction,
}

impl EventKind {
    fn label(self) -> &'static str {
        match self {
            EventKind::SliceUpdate => "slice update",
            EventKind::Hover => "hover",
            EventKind::ColdOpen => "cold open",
            EventKind::HideWindow => "hide window",
            EventKind::PredrawnShow => "pre-drawn show",
            EventKind::EvictedReopen => "evicted reopen",
            EventKind::Eviction => "eviction",
        }
    }
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

struct Frags {
    bodies: HashMap<String, String>,
}

impl Frags {
    fn new() -> Self {
        Self {
            bodies: HashMap::new(),
        }
    }
    fn set(&mut self, section: &str, html: &str) {
        self.bodies.insert(section.into(), html.into());
    }
}

impl FragmentSource for Frags {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        self.bodies
            .get(section_id)
            .cloned()
            .unwrap_or_else(|| format!("<!-- {section_id} -->"))
    }
}

fn chrome_page(css_w: u32, css_h: u32) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: {css_w}px; height: {css_h}px; background: #1a1f2a; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; }}
#strip {{ position: absolute; left: 0; top: 0; width: {css_w}px; height: 80px; background: #222833; }}
#chips {{ position: absolute; left: 8px; top: 8px; height: 36px; }}
#chips button {{ height: 32px; margin-right: 4px; padding: 0 12px; background: #2d3648;
  color: #e8ecf4; border: 1px solid #556; position: relative; }}
#chips button.on {{ background: #3d7ea6; }}
#date {{ position: absolute; left: 8px; top: 48px; font-size: 14px; }}
#ticker-lane {{
  position: absolute; left: 0; top: {ticker_top}px; width: {css_w}px; height: 32px;
  overflow: hidden; background: #141820;
}}
#ticker-inner {{ position: absolute; left: 0; top: 4px; white-space: nowrap; font-size: 14px; }}
#act {{ position: absolute; left: {act_x}px; top: 20px; width: 120px; height: 32px;
  background: #2d3648; color: #e8ecf4; border: 1px solid #556; }}
</style></head><body>
<div id="strip"></div>
<section id="sec-chips" hx-trigger="slice:time" hx-target="#chips" hx-swap="innerHTML">
  <div id="chips"></div>
</section>
<section id="sec-date" hx-trigger="slice:time" hx-target="#date" hx-swap="innerHTML">
  <div id="date">DATE</div>
</section>
<section id="sec-ticker" hx-trigger="slice:ticker" hx-target="#ticker-inner" hx-swap="innerHTML">
  <div id="ticker-lane"><div id="ticker-inner">news</div></div>
</section>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##,
        css_w = css_w,
        css_h = css_h,
        ticker_top = css_h.saturating_sub(40),
        act_x = css_w.saturating_sub(160),
    )
}

fn win_page(cell: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 560px; height: 360px; background: #2a3140; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; border: 1px solid #667; }}
table {{ width: 100%; border-collapse: collapse; font-size: 14px; }}
td {{ padding: 6px 10px; border-bottom: 1px solid #445; }}
</style></head><body>
<table>
<tr><td>{cell}</td><td>42</td></tr>
<tr><td>Beta</td><td>7</td></tr>
<tr><td>Gamma</td><td>3</td></tr>
</table>
</body></html>"##
    )
}

fn chips(active: &str) -> String {
    let mut s = String::new();
    for b in ["1x", "2x", "5x", "Week", "Month"] {
        let on = if b == active { " on" } else { "" };
        s.push_str(&format!(
            r#"<button id="chip-{b}" class="{on}" hx-post="time.set_speed" hx-trigger="click">{b}</button>"#
        ));
    }
    s
}

fn date(n: u32) -> String {
    format!(r#"<span id="date-label">DATE 2026-01-{n:02} · Week</span>"#)
}

fn ticker(offset: i32) -> String {
    format!(
        r#"<span id="ticker-text" style="margin-left:{offset}px">Breaking · Port delay · Week tick · Sim · Markets · News crawl · Extra headline</span>"#
    )
}

fn pct(samples: &[u64], p: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut v = samples.to_vec();
    v.sort_unstable();
    let idx = ((v.len() as f64) * p) as usize;
    v[idx.min(v.len() - 1)]
}

fn mean(samples: &[u64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<u64>() as f64 / samples.len() as f64
}

fn report_row(label: &str, samples: &[u64]) {
    if samples.is_empty() {
        println!("| {label} | 0 | — | — | — |");
        return;
    }
    println!(
        "| {label} | {} | {:.3} | {:.3} | {:.3} |",
        samples.len(),
        mean(samples) / 1e6,
        pct(samples, 0.95) as f64 / 1e6,
        pct(samples, 0.99) as f64 / 1e6,
    );
}

#[derive(Default)]
struct Acc {
    /// (kind, frame_ns) for every measured frame in the scripted sequence.
    frames: Vec<(EventKind, u64)>,
    merge_to_bbox: u32,
    full_repaint: u32,
    full_area_fallback: u32,
    evictions: u64,
}

impl Acc {
    fn record(&mut self, eng: &Engine, kind: EventKind, frame_ns: u64) {
        self.frames.push((kind, frame_ns));
        let s = eng.last_paint_stats();
        if s.merged_to_bbox {
            self.merge_to_bbox += 1;
        }
        if s.full_repaint {
            self.full_repaint += 1;
        }
        if s.full_repaint_area_fallback {
            self.full_area_fallback += 1;
        }
        let mem = eng.surface_memory_stats();
        self.evictions = mem.eviction_count;
    }

    fn times(&self) -> Vec<u64> {
        self.frames.iter().map(|(_, t)| *t).collect()
    }

    fn times_excluding(&self, skip: EventKind) -> Vec<u64> {
        self.frames
            .iter()
            .filter(|(k, _)| *k != skip)
            .map(|(_, t)| *t)
            .collect()
    }

    fn by_kind(&self, kind: EventKind) -> Vec<u64> {
        self.frames
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, t)| *t)
            .collect()
    }

    /// Name what sits in the overall slowest 1% (by event kind).
    fn p99_drivers(&self) -> String {
        let all = self.times();
        if all.is_empty() {
            return "n/a".into();
        }
        let threshold = pct(&all, 0.99);
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for (k, t) in &self.frames {
            if *t >= threshold {
                *counts.entry(k.label()).or_insert(0) += 1;
            }
        }
        let mut parts: Vec<_> = counts.into_iter().collect();
        parts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        parts
            .iter()
            .map(|(name, n)| format!("{name}×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn report(&mut self, label: &str, eng: &Engine) {
        let all = self.times();
        let steady = self.times_excluding(EventKind::ColdOpen);
        let mem = eng.surface_memory_stats();
        println!();
        println!("### {label}");
        println!(
            "Overall: mean={:.3} ms  p95={:.3} ms  p99={:.3} ms  frames={}  merge_to_bbox={}  full_repaint={}  area_fallback={}  evictions={}  pinned={:.2} MiB  live={:.2} MiB",
            mean(&all) / 1e6,
            pct(&all, 0.95) as f64 / 1e6,
            pct(&all, 0.99) as f64 / 1e6,
            all.len(),
            self.merge_to_bbox,
            self.full_repaint,
            self.full_area_fallback,
            self.evictions,
            mem.pinned_bytes as f64 / (1024.0 * 1024.0),
            mem.live_bytes as f64 / (1024.0 * 1024.0),
        );
        println!(
            "Steady-play (excludes deliberate cold opens): mean={:.3} ms  p95={:.3} ms  p99={:.3} ms  frames={}",
            mean(&steady) / 1e6,
            pct(&steady, 0.95) as f64 / 1e6,
            pct(&steady, 0.99) as f64 / 1e6,
            steady.len(),
        );
        println!(
            "p99 drivers (≥ overall p99 threshold): {}",
            self.p99_drivers()
        );
        println!("| Event | n | mean ms | p95 ms | p99 ms |");
        println!("|---|---|---|---|---|");
        for kind in [
            EventKind::SliceUpdate,
            EventKind::Hover,
            EventKind::ColdOpen,
            EventKind::HideWindow,
        ] {
            report_row(kind.label(), &self.by_kind(kind));
        }
        report_row("steady-play (no cold open)", &steady);
        report_row("all frames", &all);
    }
}

/// Scripted Week-speed frame. Every 8th step cold-opens a non-pinned window
/// during a slice update (deliberate; Mainnet Pause family is pinned/pre-drawn).
fn step_sequence(
    i: usize,
    eng: &mut Engine,
    slices: &mut MemSlices,
    frags: &mut Frags,
    win_view: &Viewport,
) -> (EventKind, u64) {
    let t0 = std::time::Instant::now();
    let kind = match i % 8 {
        0 => {
            slices.bump("ticker");
            frags.set("sec-ticker", &ticker(-((i as i32 % 40) * 3)));
            eng.sync_surface(CHROME, slices, frags).unwrap();
            EventKind::SliceUpdate
        }
        1 => {
            let _ = eng.input_surface(
                CHROME,
                InputEvent::PointerMove {
                    x: 48.0,
                    y: 22.0,
                    mods: Modifiers::default(),
                },
            );
            EventKind::Hover
        }
        2 => {
            let _ = eng.input_surface(CHROME, InputEvent::PointerLeave);
            EventKind::Hover
        }
        3 => {
            slices.actions = vec![(
                "act.do".into(),
                ActionState::Disabled {
                    reason: "Sim holding".into(),
                },
            )];
            slices.bump("actions");
            eng.sync_surface(CHROME, slices, frags).unwrap();
            EventKind::SliceUpdate
        }
        4 => {
            slices.actions.clear();
            slices.bump("actions");
            eng.sync_surface(CHROME, slices, frags).unwrap();
            EventKind::SliceUpdate
        }
        5 => {
            let active = ["1x", "2x", "Week", "Month"][i % 4];
            slices.bump("time");
            frags.set("sec-chips", &chips(active));
            frags.set("sec-date", &date((i % 28) as u32 + 1));
            eng.sync_surface(CHROME, slices, frags).unwrap();
            EventKind::SliceUpdate
        }
        6 => {
            // Cold open of a non-pinned window during a sim tick.
            slices.bump("time");
            frags.set("sec-chips", &chips("Week"));
            frags.set("sec-date", &date(15));
            eng.sync_surface(CHROME, slices, frags).unwrap();
            let _ = eng.close_surface(WIN);
            eng.open_surface(
                WIN,
                &win_page(&format!("Cell {i}")),
                win_view.clone(),
                SurfaceOpts {
                    visible: true,
                    pinned: false,
                },
            )
            .unwrap();
            EventKind::ColdOpen
        }
        _ => {
            if eng.surface_info(WIN).is_some() {
                eng.set_visible(WIN, false).unwrap();
            }
            EventKind::HideWindow
        }
    };
    for id in eng.dirty_surfaces() {
        let _ = eng.paint_surface(id).unwrap();
    }
    if !eng.dirty_surfaces().contains(&CHROME) {
        let _ = eng.paint_surface(CHROME).unwrap();
    }
    (kind, t0.elapsed().as_nanos() as u64)
}

fn boot_engine(phys_w: u32, phys_h: u32, budget: u64) -> (Engine, MemSlices, Frags, Viewport) {
    let css_w = phys_w;
    let css_h = phys_h;
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
        hidden_surface_budget_bytes: budget,
        fonts: Vec::new(),
        key_releases: false,
    });
    eng.declare_actions(&["time.set_speed", "act.do", "win.open"]);
    let chrome_view = Viewport::with_scale(phys_w, phys_h, 1.0);
    eng.open_surface(
        CHROME,
        &chrome_page(css_w, css_h),
        chrome_view,
        SurfaceOpts {
            visible: true,
            pinned: true,
        },
    )
    .unwrap();

    let slices = MemSlices::new();
    let mut frags = Frags::new();
    frags.set("sec-chips", &chips("1x"));
    frags.set("sec-date", &date(1));
    frags.set("sec-ticker", &ticker(0));
    eng.sync_surface(CHROME, &slices, &mut frags).unwrap();
    let _ = eng.paint_surface(CHROME).unwrap();

    let win_view = Viewport::with_scale(560, 360, 1.0);
    (eng, slices, frags, win_view)
}

fn bench_resolution(label: &str, phys_w: u32, phys_h: u32) {
    let (mut eng, mut slices, mut frags, win_view) =
        boot_engine(phys_w, phys_h, DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);

    let mut acc = Acc::default();
    for i in 0..(WARMUP + FRAMES) {
        let (kind, frame_ns) = step_sequence(i, &mut eng, &mut slices, &mut frags, &win_view);
        if i >= WARMUP {
            acc.record(&eng, kind, frame_ns);
        }
    }
    acc.report(label, &eng);
}

fn bench_open_paths(phys_w: u32, phys_h: u32) {
    let win_view = Viewport::with_scale(560, 360, 1.0);
    let html = win_page("open-path");
    const N: usize = 40;
    const W: usize = 5;

    let mut cold = Vec::new();
    let mut predrawn = Vec::new();
    let mut reopened = Vec::new();

    for i in 0..(W + N) {
        let mut eng = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        eng.declare_actions(&["act.do"]);
        let t0 = std::time::Instant::now();
        eng.open_surface(
            WIN,
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: true,
                pinned: false,
            },
        )
        .unwrap();
        let _ = eng.paint_surface(WIN).unwrap();
        let cold_ns = t0.elapsed().as_nanos() as u64;

        let mut eng2 = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        eng2.declare_actions(&["act.do"]);
        eng2.open_surface(
            WIN,
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: false,
                pinned: false,
            },
        )
        .unwrap();
        let t1 = std::time::Instant::now();
        eng2.set_visible(WIN, true).unwrap();
        let _ = eng2.paint_surface(WIN).unwrap();
        let pre_ns = t1.elapsed().as_nanos() as u64;

        let tiny = estimate_surface_engine_bytes(560, 360, &html) / 2;
        let mut eng3 = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            paint_mode: PaintMode::Exact,
            hidden_surface_budget_bytes: tiny.max(1),
            ..Default::default()
        });
        eng3.declare_actions(&["act.do"]);
        eng3.open_surface(
            WIN,
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: false,
                pinned: false,
            },
        )
        .unwrap();
        eng3.open_surface(
            SurfaceId(9),
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: false,
                pinned: false,
            },
        )
        .unwrap();
        assert!(eng3.surface_info(WIN).unwrap().evicted);
        let t2 = std::time::Instant::now();
        eng3.set_visible(WIN, true).unwrap();
        let _ = eng3.paint_surface(WIN).unwrap();
        let reopen_ns = t2.elapsed().as_nanos() as u64;

        if i >= W {
            cold.push(cold_ns);
            predrawn.push(pre_ns);
            reopened.push(reopen_ns);
        }
        let _ = (eng, eng2, eng3, phys_w, phys_h);
    }

    println!();
    println!("### Dedicated open paths (560×360 window, {phys_w}×{phys_h} chrome context unused)");
    println!("| Event | n | mean ms | p95 ms | p99 ms |");
    println!("|---|---|---|---|---|");
    report_row(EventKind::ColdOpen.label(), &cold);
    report_row(EventKind::PredrawnShow.label(), &predrawn);
    report_row(EventKind::EvictedReopen.label(), &reopened);
}

fn bench_eviction_pressure(phys_w: u32, phys_h: u32) {
    let html = win_page("evac");
    let one = estimate_surface_engine_bytes(560, 360, &html);
    let (mut eng, mut slices, mut frags, win_view) = boot_engine(phys_w, phys_h, one + 1024);

    let mut eviction_frames = Vec::new();
    let mut other_frames = Vec::new();
    let mut prev_evictions = eng.surface_memory_stats().eviction_count;

    for i in 0..(WARMUP + FRAMES) {
        let before = eng.surface_memory_stats().eviction_count;
        // Cycle hidden windows to force LRU eviction.
        if i % 3 == 0 {
            let sid = SurfaceId(100 + (i % 5) as u64);
            let _ = eng.close_surface(sid);
            eng.open_surface(
                sid,
                &html,
                win_view.clone(),
                SurfaceOpts {
                    visible: false,
                    pinned: false,
                },
            )
            .unwrap();
        }
        let (kind, frame_ns) = step_sequence(i, &mut eng, &mut slices, &mut frags, &win_view);
        let after = eng.surface_memory_stats().eviction_count;
        if i >= WARMUP {
            if after > before || after > prev_evictions {
                eviction_frames.push(frame_ns);
            } else {
                other_frames.push((kind, frame_ns));
            }
            prev_evictions = after;
        }
    }

    println!();
    println!("### Eviction-pressure run (budget ≈ one mid window, {phys_w}×{phys_h})");
    println!(
        "evictions total={}  last_eviction={:.3} ms",
        eng.surface_memory_stats().eviction_count,
        eng.surface_memory_stats().last_eviction_ns as f64 / 1e6
    );
    println!("| Event | n | mean ms | p95 ms | p99 ms |");
    println!("|---|---|---|---|---|");
    report_row(EventKind::Eviction.label(), &eviction_frames);
    let mut by: HashMap<EventKind, Vec<u64>> = HashMap::new();
    for (k, t) in other_frames {
        by.entry(k).or_default().push(t);
    }
    for kind in [
        EventKind::SliceUpdate,
        EventKind::Hover,
        EventKind::ColdOpen,
        EventKind::HideWindow,
    ] {
        report_row(
            kind.label(),
            by.get(&kind).map(|v| v.as_slice()).unwrap_or(&[]),
        );
    }
}

fn gmode_why_html(reason: &str, ellipsis: bool) -> String {
    let overflow = if ellipsis {
        "white-space: nowrap; overflow: hidden; text-overflow: ellipsis;"
    } else {
        "white-space: nowrap; overflow: hidden;"
    };
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 320px; height: 80px; background: #1a1f2a; color: #9aa3b5;
  font-family: "DejaVu Sans", sans-serif; font-size: 13px; }}
#gmode-why {{
  position: absolute; left: 8px; top: 24px;
  width: 200px; height: 20px; line-height: 20px;
  {overflow}
}}
</style></head><body>
<div id="gmode-why">{reason}</div>
<section id="sec-why" hx-trigger="slice:why" hx-target="#gmode-why" hx-swap="innerHTML"></section>
</body></html>"##
    )
}

fn ellipsis_cfg() -> EngineConfig {
    EngineConfig {
        paint_mode: PaintMode::Exact,
        max_paint_ns: 50_000_000,
        ..EngineConfig::default()
    }
}

fn mean_p95(samples: &mut [u64]) -> (f64, u64) {
    samples.sort_unstable();
    let mean = samples.iter().sum::<u64>() as f64 / samples.len() as f64;
    let p95 = samples[((samples.len() as f64) * 0.95) as usize];
    (mean, p95)
}

fn print_row(label: &str, samples: &mut [u64]) {
    let (mean, p95) = mean_p95(samples);
    println!(
        "  {label}: mean={:.3} ms  p95={:.3} ms  n={}",
        mean / 1e6,
        p95 as f64 / 1e6,
        samples.len()
    );
}

/// Issue #12: idle with/without ellipsis, text-change frame, 2000-char string.
fn bench_text_overflow_ellipsis() {
    const N: usize = 80;
    const W: usize = 10;
    let view = Viewport::new(320, 80);
    let reason = "Disabled: not enough credit on the active settlement rail for this order size";
    let reason_b = "Paused: settlement rail busy — retry after the next confirmation window opens";
    let long_2k: String = "A".repeat(2000);

    println!();
    println!("### text-overflow: ellipsis (issue #12, 320×80 Exact, 200px #gmode-why)");

    // --- Idle frames (static document, skipped paints) ---
    let mut with_e = Engine::new(ellipsis_cfg());
    with_e.declare_actions(&["act.do"]);
    with_e.load_html(&gmode_why_html(reason, true)).unwrap();
    let _ = with_e.paint(view.clone()).unwrap(); // fill truncation cache
    let mut idle_with = Vec::with_capacity(N);
    for i in 0..(W + N) {
        let t0 = std::time::Instant::now();
        let out = with_e.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        assert!(out.skipped, "idle with-ellipsis must skip");
        if i >= W {
            idle_with.push(ns);
        }
    }
    print_row("idle frame (with ellipsis)", &mut idle_with);

    let mut without = Engine::new(ellipsis_cfg());
    without.declare_actions(&["act.do"]);
    without.load_html(&gmode_why_html(reason, false)).unwrap();
    let _ = without.paint(view.clone()).unwrap();
    let mut idle_without = Vec::with_capacity(N);
    for i in 0..(W + N) {
        let t0 = std::time::Instant::now();
        let out = without.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        assert!(out.skipped, "idle without-ellipsis must skip");
        if i >= W {
            idle_without.push(ns);
        }
    }
    print_row("idle frame (without ellipsis)", &mut idle_without);

    // --- Forced dirty paint: cache hit vs no-ellipsis baseline ---
    let mut dirty_with = Vec::with_capacity(N);
    for i in 0..(W + N) {
        with_e.mark_theme_or_stylesheet_changed();
        let t0 = std::time::Instant::now();
        let out = with_e.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        assert!(!out.skipped);
        if i >= W {
            dirty_with.push(ns);
        }
    }
    print_row("dirty paint (with ellipsis, cache hit)", &mut dirty_with);

    let mut dirty_without = Vec::with_capacity(N);
    for i in 0..(W + N) {
        without.mark_theme_or_stylesheet_changed();
        let t0 = std::time::Instant::now();
        let out = without.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        assert!(!out.skipped);
        if i >= W {
            dirty_without.push(ns);
        }
    }
    print_row("dirty paint (without ellipsis)", &mut dirty_without);

    // --- Text-change frame: new string → truncation cache miss + reshape ---
    struct WhySlices {
        v: u64,
    }
    impl SliceSource for WhySlices {
        fn version(&self, name: &str) -> Option<u64> {
            if name == "why" {
                Some(self.v)
            } else {
                None
            }
        }
        fn action_states(&self) -> Vec<(String, ActionState)> {
            Vec::new()
        }
    }
    struct WhyFrags {
        body: String,
    }
    impl FragmentSource for WhyFrags {
        fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
            if section_id == "sec-why" {
                self.body.clone()
            } else {
                String::new()
            }
        }
    }

    let mut change_samples = Vec::with_capacity(N);
    for i in 0..(W + N) {
        let mut eng = Engine::new(ellipsis_cfg());
        eng.declare_actions(&["act.do"]);
        eng.load_html(&gmode_why_html(reason, true)).unwrap();
        let mut slices = WhySlices { v: 1 };
        let mut frags = WhyFrags {
            body: reason.to_string(),
        };
        let _ = eng.sync(&slices, &mut frags).unwrap();
        let _ = eng.paint(view.clone()).unwrap(); // warm cache

        let next = if i % 2 == 0 { reason_b } else { reason };
        slices.v += 1;
        frags.body = next.to_string();
        let _ = eng.sync(&slices, &mut frags).unwrap();
        let t0 = std::time::Instant::now();
        let out = eng.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        assert!(!out.skipped);
        if i >= W {
            change_samples.push(ns);
        }
    }
    print_row("text-change frame (cache miss)", &mut change_samples);

    // --- 2000-character string on the same 200px box ---
    let mut long_samples = Vec::with_capacity(N);
    for i in 0..(W + N) {
        let mut eng = Engine::new(ellipsis_cfg());
        eng.declare_actions(&["act.do"]);
        eng.load_html(&gmode_why_html(&long_2k, true)).unwrap();
        let t0 = std::time::Instant::now();
        let out = eng.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        assert!(!out.skipped);
        if i >= W {
            long_samples.push(ns);
        }
    }
    print_row(
        "2000-char first paint (truncate+reshape)",
        &mut long_samples,
    );

    // Steady dirty paints after the 2000-char cache is warm.
    let mut eng_long = Engine::new(ellipsis_cfg());
    eng_long.declare_actions(&["act.do"]);
    eng_long.load_html(&gmode_why_html(&long_2k, true)).unwrap();
    let _ = eng_long.paint(view.clone()).unwrap();
    let mut long_idle_dirty = Vec::with_capacity(N);
    for i in 0..(W + N) {
        eng_long.mark_theme_or_stylesheet_changed();
        let t0 = std::time::Instant::now();
        let _ = eng_long.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        if i >= W {
            long_idle_dirty.push(ns);
        }
    }
    print_row("2000-char dirty paint (cache hit)", &mut long_idle_dirty);

    // --- String that changes every frame (always cache miss) ---
    let mut every_frame = Vec::with_capacity(N);
    let mut eng_chg = Engine::new(ellipsis_cfg());
    eng_chg.declare_actions(&["act.do"]);
    eng_chg.load_html(&gmode_why_html(reason, true)).unwrap();
    let mut slices = WhySlices { v: 1 };
    let mut frags = WhyFrags {
        body: reason.to_string(),
    };
    let _ = eng_chg.sync(&slices, &mut frags).unwrap();
    let _ = eng_chg.paint(view.clone()).unwrap();
    for i in 0..(W + N) {
        slices.v += 1;
        frags.body = format!(
            "tick {i}: Disabled: not enough credit on the active settlement rail for this order size"
        );
        let _ = eng_chg.sync(&slices, &mut frags).unwrap();
        let t0 = std::time::Instant::now();
        let out = eng_chg.paint(view.clone()).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        assert!(!out.skipped);
        if i >= W {
            every_frame.push(ns);
        }
    }
    print_row("text changes every frame (always miss)", &mut every_frame);
}

fn main() {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown CPU".into());
    println!("paint_bench step 2 (Exact mode, multi-surface) — CPU: {cpu}");
    println!("warmup={WARMUP} measured_frames={FRAMES}");
    println!(
        "Note: the scripted sequence cold-opens a non-pinned window on every 8th frame \
         (case 6). Mainnet Pause-family windows are pinned/pre-drawn; steady-play p99 \
         excludes those cold opens."
    );
    bench_resolution("720p  (1280x720)", 1280, 720);
    bench_resolution("1080p (1920x1080)", 1920, 1080);
    bench_resolution("1440p (2560x1440)", 2560, 1440);
    bench_open_paths(1920, 1080);
    bench_eviction_pressure(1920, 1080);
    bench_text_overflow_ellipsis();
}
