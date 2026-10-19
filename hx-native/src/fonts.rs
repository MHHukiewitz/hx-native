//! Bundled fonts (no system font discovery — deterministic headless paint).

use blitz_dom::{build_single_font_ctx, FontContext};

/// DejaVu Sans (bundled). License: see `assets/fonts/LICENSE-DejaVu.txt`.
pub const FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

/// Build a FontContext with system fonts disabled and our bundled face registered.
pub fn bundled_font_context() -> FontContext {
    build_single_font_ctx(FONT_REGULAR)
}
