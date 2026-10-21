//! Bundled + host-supplied fonts (no system font discovery: deterministic
//! headless paint).
//!
//! [`bundled_font_context`] copies Blitz 0.3.0-beta.4 `build_single_font_ctx`
//! (listed under pin sensitivity in [`crate::ellipsis`]) and then registers
//! the host faces from [`EngineConfig::fonts`](crate::EngineConfig::fonts)
//! (issue #28) the way Blitz registers an `@font-face` with
//! `FontFaceOverrides`: `Collection::register_fonts` with a
//! `FontInfoOverride` carrying the host's family, weight and style.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use blitz_dom::{decode_font_bytes, FontContext};
use parley::fontique::{
    Blob, Collection, CollectionOptions, FontInfoOverride, FontWeight, GenericFamily, SourceCache,
};

use crate::error::EngineError;
use crate::types::{FontFace, FontStyle};

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

/// One validated host face: decoded once, one [`Blob`] (one Blob id) shared
/// by every FontContext the engine builds, like the bundled face.
#[derive(Clone)]
struct HostFace {
    family: String,
    weight: u16,
    style: FontStyle,
    blob: Blob<u8>,
}

impl HostFace {
    fn info_override(&self) -> FontInfoOverride<'_> {
        FontInfoOverride {
            family_name: Some(&self.family),
            weight: Some(FontWeight::new(self.weight as f32)),
            style: Some(match self.style {
                FontStyle::Normal => parley::fontique::FontStyle::Normal,
                FontStyle::Italic => parley::fontique::FontStyle::Italic,
                FontStyle::Oblique => parley::fontique::FontStyle::Oblique(None),
            }),
            ..Default::default()
        }
    }
}

/// The validated host font set of one engine (issue #28). Built once from
/// [`EngineConfig::fonts`](crate::EngineConfig::fonts); every document,
/// fragment and ellipsis-shaper FontContext of that engine registers the
/// same Blobs in the same order.
#[derive(Clone, Default)]
pub(crate) struct HostFonts {
    faces: Vec<HostFace>,
}

impl HostFonts {
    /// Validate and decode `faces` in order. Never panics: every rejection is
    /// an [`EngineError::InvalidFont`] naming the entry.
    pub(crate) fn build(faces: &[FontFace]) -> Result<Arc<Self>, EngineError> {
        let mut seen: HashSet<(String, u16, FontStyle)> = HashSet::new();
        let mut out = Vec::with_capacity(faces.len());
        for (index, f) in faces.iter().enumerate() {
            let fail = |reason: &str| EngineError::InvalidFont {
                index,
                family: f.family.clone(),
                reason: reason.to_string(),
            };
            let family = f.family.trim();
            if family.is_empty() {
                return Err(fail("family name is empty"));
            }
            if !(1..=1000).contains(&f.weight) {
                return Err(fail("weight must be in 1..=1000"));
            }
            if !seen.insert((family.to_lowercase(), f.weight, f.style)) {
                return Err(fail(
                    "duplicate of an earlier face (same family, case-insensitive, weight and style)",
                ));
            }
            let decoded = decode_font_bytes(&f.bytes).into_owned();
            let face = HostFace {
                family: family.to_string(),
                weight: f.weight,
                style: f.style,
                blob: Blob::new(Arc::new(decoded) as Arc<_>),
            };
            // Probe-register into a scratch collection: fontique returns no
            // families for bytes it cannot read (garbage, a WOFF/WOFF2 that
            // failed to decompress, an empty file).
            let mut probe = Collection::new(CollectionOptions {
                shared: false,
                system_fonts: false,
            });
            let registered = probe.register_fonts(face.blob.clone(), Some(face.info_override()));
            if registered.iter().all(|(_, infos)| infos.is_empty()) {
                return Err(fail("bytes are not a readable TTF/OTF/WOFF/WOFF2 font"));
            }
            out.push(face);
        }
        Ok(Arc::new(Self { faces: out }))
    }
}

/// Build a FontContext with system fonts disabled, the bundled DejaVu face
/// registered as every generic family, then `host`'s faces in
/// `EngineConfig::fonts` order under their host family / weight / style.
///
/// Font bytes and Blob ids are shared across every call (documents +
/// ellipsis shapers) — one Blob id per face for the glyph cache.
pub(crate) fn bundled_font_context(host: &HostFonts) -> FontContext {
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
    for face in &host.faces {
        ctx.collection
            .register_fonts(face.blob.clone(), Some(face.info_override()));
    }
    ctx
}
