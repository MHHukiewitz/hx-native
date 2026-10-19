//! CPU paint via `anyrender_vello_cpu` (G1 path — no wgpu).

use anyrender::render_to_buffer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;

use crate::error::EngineError;
use crate::types::RgbaImage;

pub fn paint_rgba(
    doc: &mut HtmlDocument,
    width: u32,
    height: u32,
    scale: f64,
) -> Result<RgbaImage, EngineError> {
    if width == 0 || height == 0 {
        return Err(EngineError::Paint("viewport size must be non-zero".into()));
    }
    let pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            paint_scene(scene, doc, scale, width, height, 0, 0);
        },
        width,
        height,
    );
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}
