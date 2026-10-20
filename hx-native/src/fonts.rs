//! Bundled fonts (no system font discovery — deterministic headless paint).

use std::sync::{Arc, OnceLock};

use blitz_dom::{decode_font_bytes, FontContext};
use parley::fontique::{Blob, Collection, CollectionOptions, GenericFamily, SourceCache};

/// DejaVu Sans (bundled). License: see `assets/fonts/LICENSE-DejaVu.txt`.
pub const FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

/// One decoded DejaVu face shared across all ellipsis shapers / documents.
fn decoded_font_bytes() -> Arc<Vec<u8>> {
    static DECODED: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
    DECODED
        .get_or_init(|| Arc::new(decode_font_bytes(FONT_REGULAR).into_owned()))
        .clone()
}

/// Build a FontContext with system fonts disabled and our bundled face registered.
///
/// Font bytes are shared via [`Arc`] across every call (documents + ellipsis shapers).
pub fn bundled_font_context() -> FontContext {
    let decoded = decoded_font_bytes();
    let mut ctx = FontContext {
        source_cache: SourceCache::new_shared(),
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        }),
    };
    let registered = ctx
        .collection
        .register_fonts(Blob::new(decoded as Arc<_>), None);
    let family_ids: Vec<_> = registered.iter().map(|(id, _)| *id).collect();
    for generic in [
        GenericFamily::SansSerif,
        GenericFamily::Serif,
        GenericFamily::Monospace,
        GenericFamily::SystemUi,
    ] {
        ctx.collection
            .append_generic_families(generic, family_ids.iter().copied());
    }
    ctx
}
