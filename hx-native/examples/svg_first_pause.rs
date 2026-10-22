//! Issue #23: measure the first-SVG pause and how much of it the automatic
//! warm-up in `Engine::new` hides. Run each mode in a fresh process:
//!
//! ```text
//! cargo run -p hx-native --example svg_first_pause -- scan   # the whole stall, on this thread
//! cargo run -p hx-native --example svg_first_pause -- race   # first SVG load right after Engine::new
//! cargo run -p hx-native --example svg_first_pause -- warm   # first SVG load after the warm-up finished
//! ```
use std::time::{Duration, Instant};

use hx_native::{svg_warm_up, svg_warm_up_done, Engine, EngineConfig, Viewport};

const PAGE: &str = r#"<!DOCTYPE html><html><head><style>html,body{margin:0;background:#fff}</style></head><body><svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path d="M7 0h2v2H7z"/></svg><p>x</p></body></html>"#;

fn first_svg_paint(e: &mut Engine) -> Duration {
    let t0 = Instant::now();
    e.load_html(PAGE).expect("load");
    e.paint(Viewport::with_scale(64, 32, 1.0)).expect("paint");
    t0.elapsed()
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "race".into());
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    match mode.as_str() {
        "scan" => println!(
            "scan: svg_warm_up on the calling thread took {:.1} ms",
            ms(svg_warm_up())
        ),
        "race" => {
            let mut e = Engine::new(EngineConfig::default());
            let first = first_svg_paint(&mut e);
            let second = first_svg_paint(&mut e);
            println!(
                "race: first SVG load+paint {:.1} ms, second {:.1} ms",
                ms(first),
                ms(second)
            );
        }
        "warm" => {
            let mut e = Engine::new(EngineConfig::default());
            let t0 = Instant::now();
            while !svg_warm_up_done() {
                std::thread::sleep(Duration::from_millis(1));
            }
            let waited = t0.elapsed();
            let first = first_svg_paint(&mut e);
            let second = first_svg_paint(&mut e);
            println!(
                "warm: warm-up {:.1} ms (waited {:.1} ms), first SVG load+paint {:.1} ms, second {:.1} ms",
                ms(svg_warm_up()),
                ms(waited),
                ms(first),
                ms(second)
            );
        }
        other => panic!("unknown mode {other}"),
    }
}
