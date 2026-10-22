//! Bundled + host-supplied fonts (no system font discovery: deterministic
//! headless paint).
//!
//! [`bundled_font_context`] copies Blitz 0.3.0-beta.4 `build_single_font_ctx`
//! (listed under pin sensitivity in [`crate::ellipsis`]) and then registers
//! the host faces from [`EngineConfig::fonts`](crate::EngineConfig::fonts)
//! (issue #28) the way Blitz registers an `@font-face` with
//! `FontFaceOverrides`: `Collection::register_fonts` with a
//! `FontInfoOverride` carrying the host's family, weight and style.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use blitz_dom::{decode_font_bytes, FontContext};
use parley::fontique::{
    Blob, Collection, CollectionOptions, FontInfoOverride, FontWeight, GenericFamily, Script,
    SourceCache,
};

use crate::error::EngineError;
use crate::types::{FontFace, FontStyle};

/// DejaVu Sans (bundled). License: see `assets/fonts/LICENSE-DejaVu.txt`.
pub const FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

/// DejaVu Sans Bold (bundled, issue #56), same licence (upstream 2.37 text,
/// including the Arev notice: `assets/fonts/LICENSE-DejaVu-2.37.txt`). Blitz 0.3.0-beta.4
/// never paints parley's synthetic bold, so without a real 700 face every
/// `font-weight: bold` in the fallback family painted regular. With it, bold
/// text in an unknown family or a generic family resolves to a real bold.
pub const FONT_BOLD: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans-Bold.ttf");

/// Family, weights and style of the bundled faces as fontique registers
/// them. Each counts as an implicit entry in the host duplicate check (issue
/// #28, crate-owner decision; bold added by #56): a host face that ties with
/// one would be silently shadowed (fontique keeps the first face
/// registered), so it is rejected. The check compares family, weight and
/// style only, **not width**: a condensed (or expanded) face registered as
/// `"DejaVu Sans"` 400 or 700 normal is refused too, even though fontique
/// would keep it as a separate face.
pub(crate) const BUNDLED_FAMILY: &str = "DejaVu Sans";
pub(crate) const BUNDLED_WEIGHTS: [u16; 2] = [400, 700];
pub(crate) const BUNDLED_STYLE: FontStyle = FontStyle::Normal;

/// Scripts the bundled DejaVu faces are registered for as fallback (ISO 15924
/// tags; `Zyyy` is Common: digits, punctuation, symbols; `Zinh` is
/// Inherited: combining marks).
const BUNDLED_FALLBACK_SCRIPTS: &[&[u8; 4]] = &[
    b"Latn", b"Grek", b"Cyrl", b"Zyyy", b"Zinh", b"Armn", b"Geor", b"Hebr",
];

/// `InvalidFont` reason for a font collection (a `ttcf` tag on the decoded
/// bytes, or a file fontique reads as more than one face).
const COLLECTION_REASON: &str =
    "font collections (.ttc/.otc, `ttcf` tag) and other multi-face files \
     are not supported; pass one single-face file per FontFace";

/// Case-insensitive family key, lowercased per character exactly like
/// fontique's family-name lookup (`char::to_lowercase`, not
/// `str::to_lowercase`): a word-final `Σ` becomes `σ`, so `"ΟΔΟΣ"` and
/// `"οδοσ"` are the same family here as they are in fontique.
fn family_key(family: &str) -> String {
    family.chars().flat_map(char::to_lowercase).collect()
}

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

/// One parley [`Blob`] for the bundled bold face (issue #56). The TTF is used
/// in place (no decode, no copy); one Blob id process-wide like the regular.
fn shared_bold_blob() -> Blob<u8> {
    static BLOB: OnceLock<Blob<u8>> = OnceLock::new();
    BLOB.get_or_init(|| Blob::new(Arc::new(FONT_BOLD) as Arc<_>))
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
    ///
    /// The byte check probe-registers the face with fontique, so it
    /// validates the tables fontique reads to register a face (the font
    /// header, names, weight/style metadata) but **not glyph outlines**: a
    /// file with a corrupt `glyf`/`CFF` table passes and paints missing
    /// glyphs. Only single-face files are accepted: any collection (a
    /// `ttcf` file tag, `.ttc` / `.otc`, even with one face) and any file
    /// fontique reads as more than one face is rejected, because one host
    /// family/weight/style override would apply to all of its faces.
    ///
    /// Families are compared case-insensitively the way fontique does
    /// (per-character lowercasing, see `family_key`). The bundled-face tie
    /// ignores width: a condensed `"DejaVu Sans"` 400 normal is refused too.
    pub(crate) fn build(faces: &[FontFace]) -> Result<Arc<Self>, EngineError> {
        let mut seen: HashSet<(String, u16, FontStyle)> = HashSet::new();
        // The bundled faces are implicit earlier entries (see BUNDLED_FAMILY).
        for w in BUNDLED_WEIGHTS {
            seen.insert((family_key(BUNDLED_FAMILY), w, BUNDLED_STYLE));
        }
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
            let key = (family_key(family), f.weight, f.style);
            let is_bundled = key.0 == family_key(BUNDLED_FAMILY)
                && BUNDLED_WEIGHTS.contains(&key.1)
                && key.2 == BUNDLED_STYLE;
            if !seen.insert(key) {
                return Err(fail(if is_bundled {
                    "ties with a bundled DejaVu Sans face (400 or 700 normal), which it would \
                     never replace; register DejaVu Sans only at another weight or style"
                } else {
                    "duplicate of an earlier face (same family, case-insensitive, weight and style)"
                }));
            }
            // TTF/OTF bytes are used as they are: the Blob shares the host's
            // `Arc<[u8]>` (no copy). Only WOFF/WOFF2 are decompressed, once
            // per engine.
            let decoded = decode_font_bytes(&f.bytes);
            // Any collection is refused by its file tag, even with one face
            // (checked on the decoded bytes, so a WOFF2-wrapped one counts).
            if decoded.starts_with(b"ttcf") {
                return Err(fail(COLLECTION_REASON));
            }
            let blob = match decoded {
                Cow::Borrowed(_) => Blob::new(Arc::new(Arc::clone(&f.bytes)) as Arc<_>),
                Cow::Owned(decoded) => Blob::new(Arc::new(decoded) as Arc<_>),
            };
            let face = HostFace {
                family: family.to_string(),
                weight: f.weight,
                style: f.style,
                blob,
            };
            // Probe-register into a scratch collection: fontique returns no
            // families for bytes it cannot read (garbage, a WOFF/WOFF2 that
            // failed to decompress, an empty file).
            let mut probe = Collection::new(CollectionOptions {
                shared: false,
                system_fonts: false,
            });
            let registered = probe.register_fonts(face.blob.clone(), Some(face.info_override()));
            let face_count: usize = registered.iter().map(|(_, infos)| infos.len()).sum();
            if face_count == 0 {
                return Err(fail("bytes are not a readable TTF/OTF/WOFF/WOFF2 font"));
            }
            // Defensive: with read-fonts 0.41 only a `ttcf` tag yields more
            // than one face, which the tag check above already refuses. Kept
            // in case a later read-fonts reads other multi-face containers.
            if face_count > 1 {
                return Err(fail(COLLECTION_REASON));
            }
            out.push(face);
        }
        Ok(Arc::new(Self { faces: out }))
    }
}

/// Build a FontContext with system fonts disabled, the bundled DejaVu faces
/// (400 and, issue #56, 700) registered as every generic family, then `host`'s faces in
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
    let mut family_ids: Vec<_> = Vec::new();
    for blob in [shared_font_blob(), shared_bold_blob()] {
        for (id, _) in ctx.collection.register_fonts(blob, None) {
            if !family_ids.contains(&id) {
                family_ids.push(id);
            }
        }
    }
    for generic in [
        GenericFamily::SansSerif,
        GenericFamily::Serif,
        GenericFamily::Monospace,
        GenericFamily::SystemUi,
    ] {
        ctx.collection
            .append_generic_families(generic, family_ids.iter().copied());
    }
    // Graph review of #61: bundled DejaVu is also the script fallback. A
    // `font-family` list naming only unknown families (no generic) otherwise
    // finds no font, and parley 0.12 then reuses one last-resort face, chosen
    // without regard to weight, for every run of the paragraph: `Plain
    // <b>Bold</b>` painted both words in one face. With a fallback, the
    // query yields the bundled family and weight matching works per run.
    for script in BUNDLED_FALLBACK_SCRIPTS {
        ctx.collection
            .append_fallbacks(Script::from_bytes(**script), family_ids.iter().copied());
    }
    for face in &host.faces {
        ctx.collection
            .register_fonts(face.blob.clone(), Some(face.info_override()));
    }
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Graph review of #61 (kills G4): both bundled faces are one Blob each,
    /// process-wide, so the shared SourceCache sees one id per face.
    #[test]
    fn bundled_blobs_are_shared_process_wide() {
        assert_eq!(shared_font_blob().id(), shared_font_blob().id());
        assert_eq!(shared_bold_blob().id(), shared_bold_blob().id());
        assert_ne!(shared_font_blob().id(), shared_bold_blob().id());
    }

    /// The bundled-face constants used by the duplicate check must match
    /// what fontique reads from the bundled files: one family, regular then
    /// bold, both normal style.
    #[test]
    fn bundled_face_constants_match_the_bundled_files() {
        let mut c = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        for (blob, weight) in [
            (shared_font_blob(), BUNDLED_WEIGHTS[0]),
            (shared_bold_blob(), BUNDLED_WEIGHTS[1]),
        ] {
            let reg = c.register_fonts(blob, None);
            assert_eq!(reg.len(), 1);
            let (id, infos) = &reg[0];
            assert_eq!(c.family_name(*id), Some(BUNDLED_FAMILY));
            assert_eq!(infos.len(), 1);
            assert_eq!(infos[0].weight().value(), weight as f32);
            assert_eq!(infos[0].style(), parley::fontique::FontStyle::Normal);
        }
        assert_eq!(BUNDLED_STYLE, FontStyle::Normal);
    }

    /// A plain TTF is not copied: the face's Blob reads the host's
    /// `Arc<[u8]>` allocation (Graph review of #30).
    #[test]
    fn plain_ttf_bytes_are_shared_not_copied() {
        let bytes: Arc<[u8]> = Arc::from(FONT_REGULAR);
        let fonts = HostFonts::build(&[FontFace::new(
            "Shared Host",
            400,
            FontStyle::Normal,
            Arc::clone(&bytes),
        )])
        .expect("valid face");
        assert_eq!(fonts.faces[0].blob.data().as_ptr(), bytes.as_ptr());
    }
}
