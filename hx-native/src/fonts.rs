//! Bundled fonts (no system font discovery — deterministic headless paint).
//!
//! [`bundled_font_context`] copies Blitz 0.3.0-beta.4 `build_single_font_ctx`
//! (listed under pin sensitivity in [`crate::ellipsis`]).

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

/// One parley [`Blob`] for the bundled face. `Blob::new` assigns a unique id;
/// cloning keeps that id so the glyph cache sees a single font source.
fn shared_font_blob() -> Blob<u8> {
    static BLOB: OnceLock<Blob<u8>> = OnceLock::new();
    BLOB.get_or_init(|| Blob::new(decoded_font_bytes() as Arc<_>))
        .clone()
}

/// Process-wide shared [`SourceCache`] so document + ellipsis shaper FontContexts
/// share glyph data keyed by the shared Blob id.
fn shared_source_cache() -> SourceCache {
    static CACHE: OnceLock<SourceCache> = OnceLock::new();
    CACHE.get_or_init(SourceCache::new_shared).clone()
}

/// Build a FontContext with system fonts disabled and our bundled face registered.
///
/// Font bytes and Blob id are shared across every call (documents + ellipsis shapers).
pub fn bundled_font_context() -> FontContext {
    let mut ctx = FontContext {
        source_cache: shared_source_cache(),
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        }),
    };
    let registered = ctx.collection.register_fonts(shared_font_blob(), None);
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
