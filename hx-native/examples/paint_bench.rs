//! Week-speed paint bench for issue #8 step 1.
//!
//! Runs a scripted sequence (ticker, hover, action greying, slice swaps, window
//! open during a sim tick) at 720p, 1080p and 1440p. Reports mean / p95 / p99
//! frame time and how often merge-to-bbox and full-repaint fallbacks fire.
//!
//! Usage: `cargo run -p hx-native --example paint_bench --release`

use std::collections::HashMap;

use hx_native::*;

const WARMUP: usize = 10;
const FRAMES: usize = 200;

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

fn page(css_w: u32, css_h: u32) -> String {
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
#chips button[data-hx-reason]:hover::after {{
  content: attr(data-hx-reason); position: absolute; left: 0; top: 36px;
  white-space: nowrap; color: #000; background: #ffffe1; border: 1px solid #000; z-index: 5;
}}
#date {{ position: absolute; left: 8px; top: 48px; font-size: 14px; }}
#ticker-lane {{
  position: absolute; left: 0; top: {ticker_top}px; width: {css_w}px; height: 32px;
  overflow: hidden; background: #141820;
}}
#ticker-inner {{ position: absolute; left: 0; top: 4px; white-space: nowrap; font-size: 14px; }}
#act {{ position: absolute; left: {act_x}px; top: 20px; width: 120px; height: 32px;
  background: #2d3648; color: #e8ecf4; border: 1px solid #556; }}
#act:disabled {{ opacity: 0.4; }}
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
<section id="sec-win" hx-trigger="slice:hud" hx-target="#win-slot" hx-swap="innerHTML">
  <div id="win-slot"></div>
</section>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##,
        css_w = css_w,
        css_h = css_h,
        ticker_top = css_h.saturating_sub(40),
        act_x = css_w.saturating_sub(160),
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

fn win(open: bool, cell: &str) -> String {
    if !open {
        return r#"<div id="win" style="display:none"></div>"#.into();
    }
    format!(
        r#"<div id="win" style="position:absolute;left:80px;top:120px;width:560px;height:360px;background:#2a3140;border:1px solid #667;box-shadow:10px 10px 24px rgba(0,0,0,0.5);display:block">
<table style="width:100%;border-collapse:collapse;font-size:14px">
<tr><td style="padding:6px 10px;border-bottom:1px solid #445">{cell}</td><td>42</td></tr>
<tr><td style="padding:6px 10px;border-bottom:1px solid #445">Beta</td><td>7</td></tr>
<tr><td style="padding:6px 10px;border-bottom:1px solid #445">Gamma</td><td>3</td></tr>
</table></div>"#
    )
}

#[derive(Default)]
struct Acc {
    times_ns: Vec<u64>,
    merge_to_bbox: u32,
    full_repaint: u32,
    full_area_fallback: u32,
    frames: u32,
}

impl Acc {
    fn record(&mut self, eng: &Engine, paint_ns: u64) {
        self.times_ns.push(paint_ns);
        self.frames += 1;
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
    }

    fn report(&mut self, label: &str) {
        self.times_ns.sort_unstable();
        let n = self.times_ns.len();
        let mean = self.times_ns.iter().sum::<u64>() as f64 / n as f64;
        let p95 = self.times_ns[((n as f64) * 0.95) as usize].min(self.times_ns[n - 1]);
        let p99 = self.times_ns[((n as f64) * 0.99) as usize].min(self.times_ns[n - 1]);
        println!(
            "{label}: mean={:.3} ms  p95={:.3} ms  p99={:.3} ms  frames={}  merge_to_bbox={}  full_repaint={}  area_fallback={}",
            mean / 1e6,
            p95 as f64 / 1e6,
            p99 as f64 / 1e6,
            self.frames,
            self.merge_to_bbox,
            self.full_repaint,
            self.full_area_fallback,
        );
    }
}

/// One Week-speed frame pattern: rotate through the scripted scenarios.
/// Returns engine `paint_ns` for the paint call.
fn step_sequence(
    i: usize,
    eng: &mut Engine,
    slices: &mut MemSlices,
    frags: &mut Frags,
    view: &Viewport,
) -> u64 {
    match i % 8 {
        0 => {
            slices.bump("ticker");
            frags.set("sec-ticker", &ticker(-((i as i32 % 40) * 3)));
            eng.sync(slices, frags).unwrap();
        }
        1 => {
            let _ = eng.input(InputEvent::PointerMove {
                x: 48.0,
                y: 22.0,
                mods: Modifiers::default(),
            });
        }
        2 => {
            let _ = eng.input(InputEvent::PointerLeave);
        }
        3 => {
            slices.actions = vec![(
                "act.do".into(),
                ActionState::Disabled {
                    reason: "Sim holding".into(),
                },
            )];
            slices.bump("actions");
            eng.sync(slices, frags).unwrap();
        }
        4 => {
            slices.actions.clear();
            slices.bump("actions");
            eng.sync(slices, frags).unwrap();
        }
        5 => {
            let active = ["1x", "2x", "Week", "Month"][i % 4];
            slices.bump("time");
            frags.set("sec-chips", &chips(active));
            frags.set("sec-date", &date((i % 28) as u32 + 1));
            eng.sync(slices, frags).unwrap();
        }
        6 => {
            // Window opens during a sim tick (time + hud together).
            slices.bump("time");
            slices.bump("hud");
            frags.set("sec-chips", &chips("Week"));
            frags.set("sec-date", &date(15));
            frags.set("sec-win", &win(true, &format!("Cell {i}")));
            eng.sync(slices, frags).unwrap();
        }
        _ => {
            slices.bump("hud");
            frags.set("sec-win", &win(false, ""));
            eng.sync(slices, frags).unwrap();
        }
    }
    let _out = eng.paint(view.clone()).unwrap();
    eng.last_timings().paint_ns
}

fn bench_resolution(label: &str, phys_w: u32, phys_h: u32) {
    // Author CSS at 1.0 logical = physical for the bench (scale 1.0).
    let css_w = phys_w;
    let css_h = phys_h;
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
    });
    eng.declare_actions(&["time.set_speed", "act.do", "win.open"]);
    eng.load_html(&page(css_w, css_h)).unwrap();

    let mut slices = MemSlices::new();
    let mut frags = Frags::new();
    frags.set("sec-chips", &chips("1x"));
    frags.set("sec-date", &date(1));
    frags.set("sec-ticker", &ticker(0));
    frags.set("sec-win", &win(false, ""));
    eng.sync(&slices, &mut frags).unwrap();

    let view = Viewport::with_scale(phys_w, phys_h, 1.0);

    let mut acc = Acc::default();
    for i in 0..(WARMUP + FRAMES) {
        let paint_ns = step_sequence(i, &mut eng, &mut slices, &mut frags, &view);
        if i >= WARMUP {
            acc.record(&eng, paint_ns);
        }
    }
    acc.report(label);
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
    println!("paint_bench (Exact mode) — CPU: {cpu}");
    println!("warmup={WARMUP} measured_frames={FRAMES}");
    bench_resolution("720p  (1280x720)", 1280, 720);
    bench_resolution("1080p (1920x1080)", 1920, 1080);
    bench_resolution("1440p (2560x1440)", 2560, 1440);
}
