//! #52 merge of main (#116): a page whose load hands out a `data:` SVG runs
//! the extra #23 resolve pass, and that pass must also carry the #116 radio
//! baseline shift. The radio sits on the baseline at every scale, before
//! and after a relayout, with the icon on the same line.
use hx_native::*;

const SCALES: [f64; 3] = [1.0, 1.5, 2.0];
const ICON: &str = "%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16'%3E%3Crect width='16' height='16' fill='%23ff0'/%3E%3C/svg%3E";

fn viewport(scale: f64, w: u32, h: u32) -> Viewport {
    Viewport::with_scale(
        (w as f64 * scale).round() as u32,
        (h as f64 * scale).round() as u32,
        scale,
    )
}

fn page() -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff;\
         font:16px \"DejaVu Sans\"}}</style></head><body>\
         <p style=\"margin:10px\"><span id=\"i\" style=\"display:inline-block;width:16px;height:16px;\
         background:url(&quot;data:image/svg+xml,{ICON}&quot;)\"></span> Term \
         <span id=\"p\" style=\"display:inline-block;width:1px;height:0;margin:0\"></span>\
         <input type=\"radio\" id=\"r\" name=\"g\" style=\"margin:0 0 7px 0\"><label>12</label></p>\
         </body></html>"
    )
}

fn bottom(e: &Engine, id: &str) -> f32 {
    let (_, y, _, h) = e.test_element_box(id).unwrap();
    y + h
}

#[test]
fn radio_stays_on_the_baseline_when_a_data_svg_adds_a_resolve_pass() {
    for scale in SCALES {
        let mut e = Engine::new(EngineConfig {
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        let vp = viewport(scale, 300, 80);
        e.open_surface(
            SurfaceId::DEFAULT,
            &page(),
            vp.clone(),
            SurfaceOpts {
                visible: true,
                pinned: true,
            },
        )
        .unwrap();
        // The load ran the second (image) pass.
        assert_eq!(e.resolve_count(), 2, "scale {scale}: data: SVG pass");
        let img = e.paint(vp).unwrap().rgba.expect("rgba");
        let (r, p) = (bottom(&e, "r"), bottom(&e, "p"));
        assert!(
            (r - p).abs() < 0.05,
            "scale {scale}: radio {r} vs baseline {p}"
        );
        // The icon painted: its 16x16 CSS box is yellow (1 device px inset,
        // since the box edge is fractional at 1.5).
        let (x, y, _, _) = e.test_element_box("i").unwrap();
        let (x0, y0) = (
            (x as f64 * scale).round() as u32,
            (y as f64 * scale).round() as u32,
        );
        let side = (16.0 * scale).round() as u32;
        let mut yellow = 0;
        for py in y0 + 1..y0 + side - 1 {
            for px in x0 + 1..x0 + side - 1 {
                let i = ((py * img.width + px) * 4) as usize;
                if img.pixels[i..i + 3] == [255, 255, 0] {
                    yellow += 1;
                }
            }
        }
        assert_eq!(
            yellow,
            (side - 2) * (side - 2),
            "scale {scale}: icon pixels"
        );
        // A relayout (resize) keeps the shift exact and does not accumulate.
        let vp2 = viewport(scale, 260, 80);
        e.paint(vp2).unwrap();
        let (r, p) = (bottom(&e, "r"), bottom(&e, "p"));
        assert!(
            (r - p).abs() < 0.05,
            "scale {scale} resized: radio {r} vs baseline {p}"
        );
    }
}
