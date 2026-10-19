//! Surface memory estimates used by the hidden-surface budget.
//!
//! These are deliberately simple so the budget check stays cheap (no heap walk).
//! The `surface_probe` example checks them against a counting allocator.

/// Bytes per RGBA8 pixel in the engine's CPU buffer.
pub const RGBA_BYTES_PER_PIXEL: u64 = 4;

/// Fixed heap cost of one parsed Blitz document (stylist, font context, UA
/// stylesheet, node arena) independent of its HTML size.
pub const DOCUMENT_ESTIMATE_BASE_BYTES: u64 = 896 * 1024;

/// Additional heap bytes per byte of source HTML (DOM nodes with computed
/// style + layout, attributes). Markup-heavy HTML lands within a few percent;
/// CSS-heavy HTML (large `<style>` blocks) is overestimated, which errs toward
/// evicting early.
pub const DOCUMENT_ESTIMATE_PER_HTML_BYTE: u64 = 296;

/// Estimated heap bytes of a parsed document built from `html`, after style,
/// layout and first raster. Based on the HTML passed to `open_surface`; section
/// fragments swapped in later by `sync` are not re-measured.
pub fn estimate_document_bytes(html: &str) -> u64 {
    DOCUMENT_ESTIMATE_BASE_BYTES
        .saturating_add((html.len() as u64).saturating_mul(DOCUMENT_ESTIMATE_PER_HTML_BYTE))
}

/// Bytes of the engine's RGBA8 buffer for a `width` x `height` device-pixel surface.
pub fn estimate_rgba_bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(RGBA_BYTES_PER_PIXEL)
}

/// Estimated engine-side bytes of one live surface: RGBA buffer + document.
/// Excludes the host's GPU texture.
pub fn estimate_surface_engine_bytes(width: u32, height: u32, html: &str) -> u64 {
    estimate_rgba_bytes(width, height).saturating_add(estimate_document_bytes(html))
}
