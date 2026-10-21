//! SVG icons (issue #23): the offline `data:image/svg+xml` image provider,
//! the content gate shared by fragment validation and the provider, and the
//! one-time SVG warm-up.
//!
//! hx-native stays offline. The only resource Blitz is ever handed is the
//! body of a `data:image/svg+xml` URL that passes [`check_svg_bytes`]. Every
//! other URL (`file:`, `http(s):`, relative, `data:` with any other MIME type,
//! `data:` fonts for `@font-face`) is dropped: Blitz's handler is never
//! called. Fonts still come only from `EngineConfig::fonts` (issue #28).
//!
//! Blitz's `NetProvider::fetch` does not say what kind of resource it wants
//! (image, stylesheet, `@import`, `@font-face`), so the gate decides from the
//! URL and the bytes alone: MIME essence `image/svg+xml`, first non-space
//! byte `<` (no TTF/OTF/WOFF/WOFF2/TTC starts with `<`), at most
//! [`DATA_SVG_MAX_BYTES`], none of [`FORBIDDEN_SVG_ELEMENTS`], and a clean
//! `usvg` parse.
//!
//! SVG `<image>` (Mike, 2026-10-21) is allowed only with a `data:` href of
//! one of [`SVG_IMAGE_MIME_TYPES`]. Each image is checked here before usvg or
//! Blitz ever decodes it: decoded size at most [`DATA_SVG_MAX_BYTES`], magic
//! bytes matching the declared type, header dimensions read by hand and at
//! most [`DATA_IMAGE_MAX_PIXELS`] with no side over [`DATA_IMAGE_MAX_SIDE`]
//! (every frame of a GIF), at most [`DATA_IMAGE_MAX_TOTAL_PIXELS`] summed over
//! every `<image>` reference in one load or swap, and then one trial
//! decode with Blitz's own decoder ([`probe_decode`]). Any failure rejects
//! the whole load or swap with [`SvgRejection`]; nothing is ever fetched.
//!
//! Round 3 (Graph HOLD on #52): a `data:` SVG is scanned for nesting depth
//! without recursion ([`xml_depth_over`]), parsed once with usvg's own
//! roxmltree options, and every element is checked by namespace-independent
//! local name, every attribute for hrefs and [`FORBIDDEN_SVG_PROPERTIES`]
//! ([`check_xml_tree`]) before usvg converts that same tree. Inline SVG gets
//! the same element, attribute, property and depth rules in the DOM.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};

use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};

/// Largest decoded `data:image/svg+xml` payload hx-native accepts, per URL
/// (64 KiB). Measured on the decoded bytes, after base64 or percent
/// decoding, never on the encoded URL; the same cap applies to each
/// `<image>` raster after its own base64 decoding. Mainnet's icons are about 500 bytes. Raise only with a measured
/// case (decision on #23, 2026-10-21).
pub const DATA_SVG_MAX_BYTES: usize = 64 * 1024;

/// SVG elements that make an SVG (inline or `data:`) invalid.
///
/// - `text`, `tspan`, `textPath`: Blitz's usvg shapes SVG text with the
///   machine's system fonts (a scan downstream crates cannot turn off), so
///   it would not be deterministic and would break the #28 font rule.
/// - `feImage`: anyrender_svg does not paint SVG filters, so an `feImage`
///   would never show; refused rather than half-supported. (`<image>` is
///   allowed under the rules in [`SVG_IMAGE_MIME_TYPES`].)
/// - `use`: one kilobyte of nested `<use>` fans out to 1e5 nodes (1.6 s to
///   load in release, Graph on #52). Mainnet's icons do not use it.
/// - `marker`: usvg copies a marker's content to every vertex of the path
///   that uses it, and a marker may hold a path with other markers, so
///   nested markers fan out like `<use>` (and an `<image>` in a marker is
///   decoded at every vertex on every paint while the budget counts it
///   once; Graph on #52 round 4). Mainnet's icons do not use it.
/// - `mask`, `pattern`: anyrender_svg paints neither, but usvg converts
///   their content again for every element that uses them with the default
///   `objectBoundingBox` units, and content may use further masks or
///   patterns: 3 KB nested five deep took 5.5-6.9 s and 170-250 MB to load
///   in release (Graph on #52 round 4). Refused rather than half-supported.
///   `clipPath` stays (anyrender_svg paints a single-path clip), but a
///   `clip-path` on a `<clipPath>` or inside one, or in a `<style>` sheet,
///   is refused for the same reason.
///
/// Names are matched on the local part (after any `prefix:`), ASCII
/// case-insensitively, in every namespace: `<é:text>`, `<svg:text>`,
/// `<TEXT>` and a `<text>` under `<foreignObject>` are all refused.
pub const FORBIDDEN_SVG_ELEMENTS: &[&str] = &[
    "text", "tspan", "textPath", "feImage", "use", "marker", "mask", "pattern",
];

/// SVG properties that make an SVG invalid, as an attribute, in a `style`
/// attribute or in a `<style>` sheet. A 147-byte `stroke-dasharray="0.01"`
/// over a 1e7-long path makes the paint try to allocate 7.5 GB and aborts
/// the process (Graph on #52); Mainnet uses no dashes.
pub const FORBIDDEN_SVG_PROPERTIES: &[&str] = &["stroke-dasharray", "stroke-dashoffset"];

/// Properties refused in an SVG `<style>` sheet (which can reach any
/// element, including the content of a `<clipPath>`) and inside a
/// `<clipPath>`: [`FORBIDDEN_SVG_PROPERTIES`] plus `clip-path`, so clip
/// paths cannot nest and fan out (Graph on #52 round 4).
pub(crate) const SVG_SHEET_FORBIDDEN_PROPERTIES: &[&str] =
    &["stroke-dasharray", "stroke-dashoffset", "clip-path"];

/// Deepest element nesting an SVG may have, counted from its root `<svg>`
/// (which is depth 1). Checked by a non-recursive scan before the XML parse,
/// again on the parsed tree (entities can add elements), and on inline SVG
/// in the DOM, so usvg's recursive conversion never sees a deep tree.
pub const SVG_MAX_DEPTH: usize = 64;

/// The only `href` / `xlink:href` an SVG `<image>` may carry: a `data:` URL
/// of one of these MIME types (matched case-insensitively, as usvg does).
/// Any other href (`http(s):`, `file:`, a path, `#fragment`, `data:` of
/// another type, including `image/svg+xml`, `image/jpg` and `text/plain`)
/// rejects the load or swap. A nested SVG is refused because usvg would
/// parse it with the host's options, where a bare path is read from disk.
pub const SVG_IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Largest decoded image an SVG `<image>` may declare: width x height at
/// most 256 x 256 (64 Ki pixels, 256 KiB RGBA), checked from the file header
/// before any decode. For GIF the logical screen and every frame are
/// checked. anyrender_svg decodes the raster again on every paint, so this
/// bounds each repaint (Graph's #52 HOLD; at 4096 x 4096 one repaint cost
/// 104-247 ms and 130-163 MB in release, at 256 x 256 it costs 0.4 ms and
/// about 1 MB). Raise only with a measured case.
///
/// This header check is the limit: `image::Limits` cannot be set from
/// hx-native (the decoders run inside blitz-dom and anyrender_svg under
/// `image`'s defaults), and hx-native takes no direct `image` dependency.
pub const DATA_IMAGE_MAX_PIXELS: u64 = 256 * 256;

/// Longest side an SVG `<image>` may declare, whatever its area:
/// anyrender_vello_cpu panics on every paint for a side above 65535, and a
/// 1 x 65536 PNG would pass the area cap alone (Graph's #52 HOLD).
pub const DATA_IMAGE_MAX_SIDE: u64 = 4096;

/// Decoded `<image>` pixels allowed in one load or swap, summed over every
/// `<image>` reference in the markup (an inline `<image>`, or one inside a
/// `data:` SVG, each time it appears), so many copies cannot add up:
/// 512 x 512 (four images at the per-image cap).
pub const DATA_IMAGE_MAX_TOTAL_PIXELS: u64 = 512 * 512;

/// Why an SVG was rejected. Carried by
/// [`EngineError::InvalidSvg`](crate::EngineError::InvalidSvg).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SvgRejection {
    /// The SVG contains one of [`FORBIDDEN_SVG_ELEMENTS`].
    ForbiddenElement { element: String },
    /// The SVG sets one of [`FORBIDDEN_SVG_PROPERTIES`].
    ForbiddenProperty { property: String },
    /// Elements nest deeper than [`SVG_MAX_DEPTH`].
    TooDeep { depth: usize, limit: usize },
    /// An `href` / `xlink:href` on an element other than `<image>` that is
    /// not a `data:` URL (shortened to 80 characters). Every non-data href
    /// is refused, `#fragment` references included.
    ExternalHref { element: String, href: String },
    /// The decoded `data:` payload is larger than [`DATA_SVG_MAX_BYTES`].
    TooLarge { bytes: usize, limit: usize },
    /// Not SVG markup, bad base64, or usvg could not parse it.
    Malformed { detail: String },
    /// An `<image>` href that is not a `data:` URL of one of
    /// [`SVG_IMAGE_MIME_TYPES`] (shortened to 80 characters).
    ImageHref { href: String },
    /// The image bytes are a different format from the declared MIME type.
    ImageTypeMismatch { declared: String, actual: String },
    /// The image header declares more than [`DATA_IMAGE_MAX_PIXELS`], or a
    /// side longer than [`DATA_IMAGE_MAX_SIDE`] (`limit` is the area cap).
    ImageTooLarge { width: u64, height: u64, limit: u64 },
    /// The `<image>`s of one load or swap add up to more than
    /// [`DATA_IMAGE_MAX_TOTAL_PIXELS`] decoded pixels.
    ImagePixelBudget { pixels: u64, limit: u64 },
    /// The image header is unreadable or the decoder refused the data.
    CorruptImage { mime: String, detail: String },
}

impl std::fmt::Display for SvgRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SvgRejection::ForbiddenElement { element } => {
                write!(f, "SVG contains a forbidden <{element}> element")
            }
            SvgRejection::ForbiddenProperty { property } => {
                write!(f, "SVG sets the forbidden property {property}")
            }
            SvgRejection::TooDeep { depth, limit } => {
                write!(
                    f,
                    "SVG nests {depth} elements deep, above the limit of {limit}"
                )
            }
            SvgRejection::ExternalHref { element, href } => {
                write!(f, "SVG <{element}> href `{href}` is not a data: URL")
            }
            SvgRejection::TooLarge { bytes, limit } => {
                write!(
                    f,
                    "data: SVG is {bytes} bytes, above the {limit}-byte limit"
                )
            }
            SvgRejection::Malformed { detail } => write!(f, "malformed SVG: {detail}"),
            SvgRejection::ImageHref { href } => write!(
                f,
                "SVG <image> href `{href}` is not a data: URL of {}",
                SVG_IMAGE_MIME_TYPES.join(", ")
            ),
            SvgRejection::ImageTypeMismatch { declared, actual } => {
                write!(f, "SVG <image> declared {declared} but holds {actual}")
            }
            SvgRejection::ImageTooLarge {
                width,
                height,
                limit,
            } => write!(
                f,
                "SVG <image> is {width}x{height}, above the {limit}-pixel limit"
            ),
            SvgRejection::ImagePixelBudget { pixels, limit } => write!(
                f,
                "SVG <image>s in one load or swap total {pixels} pixels, above the {limit}-pixel limit"
            ),
            SvgRejection::CorruptImage { mime, detail } => {
                write!(f, "corrupt {mime} in SVG <image>: {detail}")
            }
        }
    }
}

/// First forbidden element name in raw SVG/XML text, if any. Matches
/// `<name` and `<prefix:name` followed by whitespace, `/` or `>`, with any
/// byte >= 0x80 counted as a name character (`<é:text>`) and the name
/// compared case-insensitively. Text in comments or CDATA that looks like
/// such a tag also matches: the gate fails closed. A first layer only; the
/// namespace-aware check is [`check_xml_tree`].
pub(crate) fn find_forbidden_element(src: &[u8]) -> Option<&'static str> {
    let mut i = 0;
    while i < src.len() {
        if src[i] != b'<' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < src.len()
            && (src[end].is_ascii_alphanumeric()
                || src[end] >= 0x80
                || matches!(src[end], b':' | b'_' | b'-' | b'.'))
        {
            end += 1;
        }
        let terminated = end == src.len()
            || src[end].is_ascii_whitespace()
            || src[end] == b'/'
            || src[end] == b'>';
        if end > start && terminated {
            let name = &src[start..end];
            let local = match name.iter().rposition(|&b| b == b':') {
                Some(p) => &name[p + 1..],
                None => name,
            };
            if let Some(hit) = FORBIDDEN_SVG_ELEMENTS
                .iter()
                .find(|f| f.as_bytes().eq_ignore_ascii_case(local))
            {
                return Some(hit);
            }
        }
        i = end.max(start);
    }
    None
}

/// The forbidden element an element name stands for: the local part after
/// the last `:`, compared ASCII case-insensitively.
pub(crate) fn forbidden_element(name: &str) -> Option<&'static str> {
    let local = name.rsplit(':').next().unwrap_or(name);
    FORBIDDEN_SVG_ELEMENTS
        .iter()
        .find(|f| f.eq_ignore_ascii_case(local))
        .copied()
}

/// The first of [`FORBIDDEN_SVG_PROPERTIES`] a CSS text mentions (tests).
#[cfg(test)]
pub(crate) fn css_forbidden_property(css: &str) -> Option<&'static str> {
    css_mentions(css, FORBIDDEN_SVG_PROPERTIES)
}

/// The first of `names` a CSS text sets or mentions, if any. Escapes are
/// decoded and case folded first, so `STROKE-DASH\61rray` is caught. The
/// text is checked twice, once with comments dropped (`stroke-/**/dasharray`)
/// and once with them kept: a `/*` inside a CSS string (`"/*"`) is not a
/// comment to usvg's CSS parser, so dropping it here could hide a live
/// declaration (Graph, #52 round 4). Any mention in either view fails closed.
pub(crate) fn css_mentions(css: &str, names: &[&'static str]) -> Option<&'static str> {
    let bytes = css.as_bytes();
    let mut stripped = Vec::with_capacity(bytes.len());
    let mut kept = Vec::with_capacity(bytes.len());
    let mut j = 0;
    while j < bytes.len() {
        if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
            kept.extend_from_slice(b"/*");
            j += 2;
            while j < bytes.len() && !(bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/')) {
                if bytes[j] == b'\\' {
                    j += 1;
                    read_css_escape(bytes, &mut j, &mut kept);
                } else {
                    kept.push(bytes[j]);
                    j += 1;
                }
            }
            kept.extend_from_slice(b"*/");
            j += 2;
        } else if bytes[j] == b'\\' {
            j += 1;
            let at = kept.len();
            read_css_escape(bytes, &mut j, &mut kept);
            stripped.extend_from_slice(&kept[at..]);
        } else {
            stripped.push(bytes[j]);
            kept.push(bytes[j]);
            j += 1;
        }
    }
    let views = [
        String::from_utf8_lossy(&stripped).to_ascii_lowercase(),
        String::from_utf8_lossy(&kept).to_ascii_lowercase(),
    ];
    names
        .iter()
        .find(|p| views.iter().any(|v| v.contains(**p)))
        .copied()
}

/// One attribute of an SVG element (in a `data:` SVG or inline in the DOM),
/// by its name as written (`href`, `xlink:href`, `é:href`, `HREF`). Refuses
/// [`FORBIDDEN_SVG_PROPERTIES`] as attributes or inside `style`, and every
/// href that is not a `data:` URL. An `<image>` href must also pass
/// [`check_image_href`]; the checked raster is returned for the trial decode.
pub(crate) fn check_svg_attribute(
    element: &str,
    attr: &str,
    value: &str,
    in_clip_path: bool,
) -> Result<Option<RasterImage>, SvgRejection> {
    let local = attr.rsplit(':').next().unwrap_or(attr);
    let lower = local.to_ascii_lowercase();
    let refused = if in_clip_path {
        SVG_SHEET_FORBIDDEN_PROPERTIES
    } else {
        FORBIDDEN_SVG_PROPERTIES
    };
    if let Some(p) = refused.iter().find(|p| lower.contains(**p)) {
        return Err(SvgRejection::ForbiddenProperty {
            property: p.to_string(),
        });
    }
    if local.eq_ignore_ascii_case("style") {
        if let Some(p) = css_mentions(value, refused) {
            return Err(SvgRejection::ForbiddenProperty {
                property: p.to_string(),
            });
        }
    }
    if !local.eq_ignore_ascii_case("href") {
        return Ok(None);
    }
    let el_local = element.rsplit(':').next().unwrap_or(element);
    if el_local.eq_ignore_ascii_case("image") {
        return check_image_href(value).map(Some);
    }
    if data_url::DataUrl::process(value).is_ok() {
        return Ok(None);
    }
    Err(SvgRejection::ExternalHref {
        element: element.to_string(),
        href: short(value),
    })
}

/// Deepest element nesting in raw XML, scanned without recursion and
/// without parsing: comments, CDATA, processing instructions and the
/// DOCTYPE (with its internal subset) are skipped, quoted attribute values
/// are honoured, `/>` does not nest. Stops as soon as `limit` is passed and
/// returns that depth; `None` when the text stays within it.
pub(crate) fn xml_depth_over(src: &[u8], limit: usize) -> Option<usize> {
    let find = |from: usize, pat: &[u8]| {
        src.get(from..)
            .and_then(|rest| rest.windows(pat.len()).position(|w| w == pat))
            .map(|p| from + p + pat.len())
    };
    let mut depth = 0usize;
    let mut i = 0;
    while i < src.len() {
        if src[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &src[i..];
        if rest.starts_with(b"<!--") {
            i = find(i + 4, b"-->").unwrap_or(src.len());
        } else if rest.starts_with(b"<![CDATA[") {
            i = find(i + 9, b"]]>").unwrap_or(src.len());
        } else if rest.starts_with(b"<?") {
            i = find(i + 2, b"?>").unwrap_or(src.len());
        } else if rest.starts_with(b"<!") {
            // DOCTYPE: skip to the `>` outside quotes and `[...]`.
            let (mut j, mut bracket, mut quote) = (i + 2, 0usize, 0u8);
            while j < src.len() {
                let b = src[j];
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'[' {
                    bracket += 1;
                } else if b == b']' {
                    bracket = bracket.saturating_sub(1);
                } else if b == b'>' && bracket == 0 {
                    break;
                }
                j += 1;
            }
            i = j + 1;
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(i + 2, b">").unwrap_or(src.len());
        } else {
            let (mut j, mut quote) = (i + 1, 0u8);
            while j < src.len() {
                let b = src[j];
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' {
                    break;
                }
                j += 1;
            }
            if j >= src.len() || src[j - 1] != b'/' {
                depth += 1;
                if depth > limit {
                    return Some(depth);
                }
            }
            i = j + 1;
        }
    }
    None
}

/// The namespace-aware gate on a parsed `data:` SVG (the same roxmltree
/// parse usvg then converts): every element at any depth, in any namespace
/// and under any prefix, entity expansions included. Returns the `<image>`
/// rasters it checked.
pub(crate) fn check_xml_tree(
    doc: &usvg::roxmltree::Document,
) -> Result<Vec<RasterImage>, SvgRejection> {
    let mut images = Vec::new();
    for node in doc.descendants() {
        if node.is_text() {
            if let Some(p) = node
                .text()
                .and_then(|t| css_mentions(t, SVG_SHEET_FORBIDDEN_PROPERTIES))
            {
                return Err(SvgRejection::ForbiddenProperty {
                    property: p.to_string(),
                });
            }
            continue;
        }
        if !node.is_element() {
            continue;
        }
        let depth = node.ancestors().filter(|n| n.is_element()).count();
        if depth > SVG_MAX_DEPTH {
            return Err(SvgRejection::TooDeep {
                depth,
                limit: SVG_MAX_DEPTH,
            });
        }
        let name = node.tag_name().name();
        if let Some(element) = forbidden_element(name) {
            return Err(SvgRejection::ForbiddenElement {
                element: element.to_string(),
            });
        }
        let in_clip_path = node
            .ancestors()
            .any(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("clipPath"));
        for a in node.attributes() {
            if let Some(img) = check_svg_attribute(name, a.name(), a.value(), in_clip_path)? {
                images.push(img);
            }
        }
    }
    Ok(images)
}

/// An SVG `<image>` payload that passed the static checks, waiting for the
/// trial decode in [`probe_decode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RasterImage {
    pub(crate) mime: &'static str,
    pub(crate) data: Arc<Vec<u8>>,
    /// Decoded pixels one paint of this image costs (the largest frame or
    /// screen the header declares), counted toward
    /// [`DATA_IMAGE_MAX_TOTAL_PIXELS`].
    pub(crate) pixels: u64,
}

fn short(href: &str) -> String {
    let mut s: String = href.chars().take(80).collect();
    if s.len() < href.len() {
        s.push('…');
    }
    s
}

/// Check one `<image>` href the way usvg reads it: `data_url::DataUrl::process`
/// on the raw attribute value; anything that is not a data URL is a path or
/// URL usvg would hand to its file loader, so it is refused.
pub(crate) fn check_image_href(href: &str) -> Result<RasterImage, SvgRejection> {
    let Ok(url) = data_url::DataUrl::process(href) else {
        return Err(SvgRejection::ImageHref { href: short(href) });
    };
    let mime = format!("{}/{}", url.mime_type().type_, url.mime_type().subtype);
    let Ok((data, _)) = url.decode_to_vec() else {
        return Err(SvgRejection::CorruptImage {
            mime,
            detail: "invalid base64".into(),
        });
    };
    check_raster(&mime, data).map_err(|e| match e {
        SvgRejection::ImageHref { .. } => SvgRejection::ImageHref { href: short(href) },
        other => other,
    })
}

/// Static checks on decoded `<image>` bytes with their declared MIME type.
pub(crate) fn check_raster(mime: &str, data: Vec<u8>) -> Result<RasterImage, SvgRejection> {
    let Some(&declared) = SVG_IMAGE_MIME_TYPES.iter().find(|m| **m == mime) else {
        return Err(SvgRejection::ImageHref {
            href: format!("data:{mime},…"),
        });
    };
    if data.len() > DATA_SVG_MAX_BYTES {
        return Err(SvgRejection::TooLarge {
            bytes: data.len(),
            limit: DATA_SVG_MAX_BYTES,
        });
    }
    let corrupt = |detail: &str| SvgRejection::CorruptImage {
        mime: declared.to_string(),
        detail: detail.to_string(),
    };
    let Some(actual) = sniff_image(&data) else {
        return Err(corrupt("not a PNG, JPEG, GIF or WebP file"));
    };
    if actual != declared {
        return Err(SvgRejection::ImageTypeMismatch {
            declared: declared.to_string(),
            actual: actual.to_string(),
        });
    }
    let dims = match actual {
        "image/png" => png_dimensions(&data).map(|d| vec![d]),
        "image/gif" => gif_dimensions(&data),
        "image/webp" => webp_dimensions(&data).map(|d| vec![d]),
        _ => jpeg_dimensions(&data).map(|d| vec![d]),
    };
    let Some(dims) = dims else {
        return Err(corrupt("unreadable image header"));
    };
    if actual == "image/png" {
        if let Some(c) = png_refused_chunk(&data) {
            return Err(corrupt(&format!(
                "PNG {c} chunk refused (inflated on every paint); strip the colour profile"
            )));
        }
    }
    let mut pixels = 0;
    for (w, h) in dims {
        if w == 0 || h == 0 {
            return Err(corrupt("zero-sized image"));
        }
        if w > DATA_IMAGE_MAX_SIDE || h > DATA_IMAGE_MAX_SIDE || w * h > DATA_IMAGE_MAX_PIXELS {
            return Err(SvgRejection::ImageTooLarge {
                width: w,
                height: h,
                limit: DATA_IMAGE_MAX_PIXELS,
            });
        }
        pixels = pixels.max(w * h);
    }
    Ok(RasterImage {
        mime: declared,
        data: Arc::new(data),
        pixels,
    })
}

/// The format the bytes really are, from their magic number.
fn sniff_image(d: &[u8]) -> Option<&'static str> {
    if d.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if d.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if d.len() >= 12 && &d[..4] == b"RIFF" && &d[8..12] == b"WEBP" {
        Some("image/webp")
    } else if d
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(d)
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        == Some(&b'<')
    {
        // SVG or other XML behind a raster label: refused as a mismatch,
        // never parsed.
        Some("image/svg+xml")
    } else {
        None
    }
}

fn be32(d: &[u8], i: usize) -> Option<u64> {
    let b = d.get(i..i + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64)
}
fn be16(d: &[u8], i: usize) -> Option<u64> {
    let b = d.get(i..i + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]) as u64)
}
fn le16(d: &[u8], i: usize) -> Option<u64> {
    let b = d.get(i..i + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]) as u64)
}
fn le24(d: &[u8], i: usize) -> Option<u64> {
    let b = d.get(i..i + 3)?;
    Some(b[0] as u64 | (b[1] as u64) << 8 | (b[2] as u64) << 16)
}

/// PNG chunks whose payload the `png` crate inflates on every decode,
/// whatever the image size: a 61 KB 16x16 PNG whose `iCCP` inflates to
/// 60 MB costs about 38 ms and +60 MB on every paint in release (Graph, #52
/// round 4). The header cap cannot see it, so the chunk is refused.
/// (`zTXt` and compressed `iTXt` of the same size measured no cost: the
/// decoder does not inflate them.)
const PNG_REFUSED_CHUNKS: &[&[u8; 4]] = &[b"iCCP"];

/// PNG: the IHDR chunk must come first.
fn png_dimensions(d: &[u8]) -> Option<(u64, u64)> {
    if be32(d, 8)? != 13 || d.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be32(d, 16)?, be32(d, 20)?))
}

/// The first of [`PNG_REFUSED_CHUNKS`] anywhere in a PNG, walking every
/// chunk (the file is at most [`DATA_SVG_MAX_BYTES`]).
fn png_refused_chunk(d: &[u8]) -> Option<&'static str> {
    let mut i = 8usize;
    while let (Some(len), Some(kind)) = (be32(d, i), d.get(i + 4..i + 8)) {
        if let Some(c) = PNG_REFUSED_CHUNKS.iter().find(|c| &c[..] == kind) {
            return std::str::from_utf8(&c[..]).ok();
        }
        if kind == b"IEND" {
            return None;
        }
        i = i.checked_add(12)?.checked_add(usize::try_from(len).ok()?)?;
    }
    None
}

/// GIF: the logical screen and every image descriptor (frame), walking the
/// block structure. Truncated or unknown blocks are unreadable (`None`).
fn gif_dimensions(d: &[u8]) -> Option<Vec<(u64, u64)>> {
    let mut out = vec![(le16(d, 6)?, le16(d, 8)?)];
    let flags = *d.get(10)?;
    let mut i = 13;
    if flags & 0x80 != 0 {
        i += 3 << ((flags & 7) + 1);
    }
    let skip_sub_blocks = |mut i: usize| -> Option<usize> {
        loop {
            let n = *d.get(i)? as usize;
            i += 1;
            if n == 0 {
                return Some(i);
            }
            i += n;
        }
    };
    loop {
        match *d.get(i)? {
            0x3B => return Some(out),
            0x21 => i = skip_sub_blocks(i + 2)?,
            0x2C => {
                out.push((le16(d, i + 5)?, le16(d, i + 7)?));
                let f = *d.get(i + 9)?;
                i += 10;
                if f & 0x80 != 0 {
                    i += 3 << ((f & 7) + 1);
                }
                i = skip_sub_blocks(i + 1)?;
            }
            _ => return None,
        }
    }
}

/// WebP: the canvas of an extended (VP8X) file, else the lossy (VP8) or
/// lossless (VP8L) frame header.
fn webp_dimensions(d: &[u8]) -> Option<(u64, u64)> {
    match d.get(12..16)? {
        b"VP8X" => Some((le24(d, 24)? + 1, le24(d, 27)? + 1)),
        b"VP8 " => {
            if d.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((le16(d, 26)? & 0x3FFF, le16(d, 28)? & 0x3FFF))
        }
        b"VP8L" => {
            if *d.get(20)? != 0x2F {
                return None;
            }
            let b = d.get(21..25)?;
            let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64;
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

/// JPEG: the first start-of-frame marker, walking the marker segments.
fn jpeg_dimensions(d: &[u8]) -> Option<(u64, u64)> {
    let mut i = 2;
    loop {
        if *d.get(i)? != 0xFF {
            return None;
        }
        let marker = *d.get(i + 1)?;
        match marker {
            0xFF => i += 1,
            0xD8 | 0x01 | 0xD0..=0xD7 => i += 2,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Some((be16(d, i + 7)?, be16(d, i + 5)?));
            }
            0xD9 | 0xDA => return None,
            _ => i += 2 + be16(d, i + 2)? as usize,
        }
    }
}

/// Trial-decode every image with Blitz's own decoder (the same `image`
/// crate build anyrender_svg paints with) in a throwaway document whose
/// only resource source is these bytes. A corrupt image is rejected here so
/// it never reaches paint, where anyrender_svg would draw a half-transparent
/// red box over it. Animated GIF and WebP: both decoders take the first
/// frame only; memory is bounded by [`DATA_IMAGE_MAX_PIXELS`] per image and
/// [`DATA_IMAGE_MAX_TOTAL_PIXELS`] per load or swap.
pub(crate) fn probe_decode(images: &[RasterImage]) -> Result<(), SvgRejection> {
    if images.is_empty() {
        return Ok(());
    }
    let mut unique: Vec<&RasterImage> = Vec::new();
    for img in images {
        if !unique.iter().any(|u| u.data == img.data) {
            unique.push(img);
        }
    }
    let mut html = String::from("<!DOCTYPE html><html><body>");
    for i in 0..unique.len() {
        html.push_str(&format!("<img id=\"p{i}\" src=\"hx-probe:{i}\">"));
    }
    html.push_str("</body></html>");
    let provider = Arc::new(ProbeProvider(
        unique.iter().map(|u| Arc::clone(&u.data)).collect(),
    ));
    let mut doc = blitz_html::HtmlDocument::from_html(
        &html,
        blitz_dom::DocumentConfig {
            viewport: Some(blitz_traits::shell::Viewport::new(
                4,
                4,
                1.0,
                blitz_traits::shell::ColorScheme::Light,
            )),
            html_parser_provider: Some(Arc::new(blitz_html::HtmlProvider)),
            net_provider: Some(provider as Arc<dyn NetProvider>),
            style_threading: blitz_dom::StyleThreading::Sequential,
            ..Default::default()
        },
    );
    doc.resolve(0.0);
    doc.resolve(0.0);
    for (i, img) in unique.iter().enumerate() {
        let decoded = doc
            .get_element_by_id(&format!("p{i}"))
            .and_then(|nid| doc.get_node(nid))
            .and_then(|n| n.element_data())
            .is_some_and(|el| el.raster_image_data().is_some());
        if !decoded {
            return Err(SvgRejection::CorruptImage {
                mime: img.mime.to_string(),
                detail: "the decoder refused the image data".into(),
            });
        }
    }
    Ok(())
}

/// Serves `hx-probe:<n>` from the probe's own byte list and nothing else.
struct ProbeProvider(Vec<Arc<Vec<u8>>>);

impl NetProvider for ProbeProvider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str();
        let Some(data) = url
            .strip_prefix("hx-probe:")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| self.0.get(n))
        else {
            return;
        };
        handler.bytes(url.to_string(), Bytes::from(data.as_ref().clone()));
    }
    fn is_noop(&self) -> bool {
        true
    }
}

/// The full content gate for a decoded SVG payload. Returns the `<image>`
/// payloads usvg found, already statically checked, for [`probe_decode`].
pub(crate) fn check_svg_bytes(bytes: &[u8]) -> Result<Vec<RasterImage>, SvgRejection> {
    if bytes.len() > DATA_SVG_MAX_BYTES {
        return Err(SvgRejection::TooLarge {
            bytes: bytes.len(),
            limit: DATA_SVG_MAX_BYTES,
        });
    }
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if body.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'<') {
        return Err(SvgRejection::Malformed {
            detail: "payload does not start with `<`".into(),
        });
    }
    if let Some(depth) = xml_depth_over(body, SVG_MAX_DEPTH) {
        return Err(SvgRejection::TooDeep {
            depth,
            limit: SVG_MAX_DEPTH,
        });
    }
    if let Some(element) = find_forbidden_element(body) {
        return Err(SvgRejection::ForbiddenElement {
            element: element.to_string(),
        });
    }
    let malformed = |detail: String| SvgRejection::Malformed { detail };
    let text = std::str::from_utf8(body).map_err(|_| malformed("not UTF-8".into()))?;
    // The same roxmltree parse usvg does (DTD allowed), checked by
    // namespace and local name before usvg converts it.
    let xml = usvg::roxmltree::Document::parse_with_options(
        text,
        usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| malformed(format!("SVG data parsing failed cause {e}")))?;
    let mut checked = check_xml_tree(&xml)?;
    // An empty font database: never the system font scan, and with text
    // already refused above nothing here needs a font. The image resolvers
    // replace usvg's defaults (which read paths from disk and parse nested
    // SVG): every `<image>` usvg converts, also through `<use>`, is checked
    // and recorded, never loaded.
    let found: std::sync::Mutex<Vec<Result<RasterImage, SvgRejection>>> =
        std::sync::Mutex::new(Vec::new());
    let record = |r| {
        if let Ok(mut v) = found.lock() {
            v.push(r);
        }
    };
    let opts = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|mime: &str, data: Arc<Vec<u8>>, _: &usvg::Options| {
                record(check_raster(mime, data.as_ref().clone()));
                None
            }),
            resolve_string: Box::new(|href: &str, _: &usvg::Options| {
                record(Err(SvgRejection::ImageHref { href: short(href) }));
                None
            }),
        },
        ..usvg::Options::default()
    };
    usvg::Tree::from_xmltree(&xml, &opts).map_err(|e| malformed(e.to_string()))?;
    drop(opts);
    let found = found.into_inner().unwrap_or_default();
    for r in found {
        let img = r?;
        if !checked.contains(&img) {
            checked.push(img);
        }
    }
    Ok(checked)
}

/// What a URL string is to the SVG gate.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DataSvg {
    /// Not a `data:image/svg+xml` URL (another scheme, another MIME type, or
    /// not a URL). The provider drops it; validation ignores it.
    NotSvg,
    /// A `data:image/svg+xml` URL that passes the gate, with its body and
    /// the `<image>` payloads still to be trial-decoded.
    Ok(Vec<u8>, Vec<RasterImage>),
    /// A `data:image/svg+xml` URL that fails the gate.
    Rejected(SvgRejection),
}

/// Classify one URL. Percent-encoded and `;base64` bodies both work.
pub(crate) fn classify_url(url: &str) -> DataSvg {
    let trimmed = url.trim();
    if !trimmed
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("data:"))
    {
        return DataSvg::NotSvg;
    }
    let Ok(parsed) = data_url::DataUrl::process(trimmed) else {
        return DataSvg::NotSvg;
    };
    let mime = parsed.mime_type();
    if mime.type_ != "image" || mime.subtype != "svg+xml" {
        return DataSvg::NotSvg;
    }
    let body = match parsed.decode_to_vec() {
        Ok((body, _)) => body,
        Err(_) => {
            return DataSvg::Rejected(SvgRejection::Malformed {
                detail: "invalid base64".into(),
            })
        }
    };
    match check_svg_bytes(&body) {
        Ok(images) => DataSvg::Ok(body, images),
        Err(r) => DataSvg::Rejected(r),
    }
}

/// Read one CSS escape after a backslash at `bytes[*j]` (already past the
/// `\\`): 1-6 hex digits plus one optional whitespace, or any other single
/// character. Pushes the decoded character to `out`.
fn read_css_escape(bytes: &[u8], j: &mut usize, out: &mut Vec<u8>) {
    let start = *j;
    while *j < bytes.len() && *j - start < 6 && bytes[*j].is_ascii_hexdigit() {
        *j += 1;
    }
    if *j > start {
        let hex = std::str::from_utf8(&bytes[start..*j]).unwrap_or("0");
        let cp = u32::from_str_radix(hex, 16).unwrap_or(0xFFFD);
        let ch = char::from_u32(cp)
            .filter(|&c| c != '\0')
            .unwrap_or('\u{FFFD}');
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        if *j < bytes.len() && bytes[*j].is_ascii_whitespace() {
            *j += 1;
        }
    } else if *j < bytes.len() {
        out.push(bytes[*j]);
        *j += 1;
    }
}

/// Read a quoted CSS string starting at the quote `bytes[*j]`, unescaped.
fn read_css_string(bytes: &[u8], j: &mut usize) -> String {
    let q = bytes[*j];
    *j += 1;
    let mut val = Vec::new();
    while *j < bytes.len() && bytes[*j] != q {
        if bytes[*j] == b'\\' {
            *j += 1;
            read_css_escape(bytes, j, &mut val);
        } else {
            val.push(bytes[*j]);
            *j += 1;
        }
    }
    *j += 1; // closing quote
    String::from_utf8_lossy(&val).into_owned()
}

/// URLs a stylesheet or `style` attribute can hand to Blitz's resource
/// provider: every `url(...)` value (quoted or not) and every quoted string
/// that starts with `data:` (as in `image-set("data:…" 1x)`). CSS escapes are
/// decoded and comments skipped, the same way Stylo reads them, so an escaped
/// `da\74 a:` cannot slip past validation. Used to validate `<style>` text and
/// `style` attributes before a load or swap is applied.
pub(crate) fn css_urls(css: &str) -> Vec<String> {
    let bytes = css.as_bytes();
    let mut out = Vec::new();
    let mut j = 0;
    while j < bytes.len() {
        let b = bytes[j];
        if b == b'/' && bytes.get(j + 1) == Some(&b'*') {
            j += 2;
            while j < bytes.len() && !(bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/')) {
                j += 1;
            }
            j += 2;
        } else if b == b'"' || b == b'\'' {
            let val = read_css_string(bytes, &mut j);
            if val
                .trim_start()
                .get(..5)
                .is_some_and(|p| p.eq_ignore_ascii_case("data:"))
            {
                out.push(val.trim().to_string());
            }
        } else if j + 4 <= bytes.len() && bytes[j..j + 4].eq_ignore_ascii_case(b"url(") {
            j += 4;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                let val = read_css_string(bytes, &mut j);
                out.push(val.trim().to_string());
            } else {
                let mut val = Vec::new();
                while j < bytes.len() && bytes[j] != b')' {
                    if bytes[j] == b'\\' {
                        j += 1;
                        read_css_escape(bytes, &mut j, &mut val);
                    } else {
                        val.push(bytes[j]);
                        j += 1;
                    }
                }
                out.push(String::from_utf8_lossy(&val).trim().to_string());
            }
        } else if b == b'\\' {
            // An escape outside a string (e.g. in `u\72l(`) is decoded by
            // Stylo too; treat the rest of the declaration conservatively by
            // skipping just the escape here.
            j += 1;
            let mut sink = Vec::new();
            read_css_escape(bytes, &mut j, &mut sink);
        } else {
            j += 1;
        }
    }
    out
}

/// The document's only resource source: `data:image/svg+xml` URLs that pass
/// the gate, answered on the calling thread. Crate-private, with nothing to
/// configure: a host cannot make it fetch anything else.
#[derive(Debug, Default)]
pub(crate) struct DataSvgProvider {
    served: AtomicU64,
    refused: AtomicU64,
}

impl DataSvgProvider {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// URLs answered so far (each first use of a URL in a document).
    pub(crate) fn served(&self) -> u64 {
        self.served.load(Ordering::Relaxed)
    }
    /// URLs dropped so far (any scheme or MIME type, or a failed gate).
    pub(crate) fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }
}

impl NetProvider for DataSvgProvider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        match classify_url(request.url.as_str()) {
            DataSvg::Ok(body, _) => {
                self.served.fetch_add(1, Ordering::Relaxed);
                handler.bytes(request.url.to_string(), Bytes::from(body));
            }
            DataSvg::NotSvg | DataSvg::Rejected(_) => {
                // Dropped: the handler is never called.
                self.refused.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// `true` so Blitz never marks a `<link rel=stylesheet>` as
    /// render-blocking: this provider drops it, and a blocking request that
    /// never answers would stop `resolve` for good.
    fn is_noop(&self) -> bool {
        true
    }
}

static WARM: OnceLock<Duration> = OnceLock::new();
static WARM_SPAWN: Once = Once::new();

/// One-time SVG warm-up. The first SVG Blitz parses in a process runs its
/// system font scan (`fontdb::Database::load_system_fonts`), which costs
/// about 140-215 ms in a debug build. This parses a one-path inline SVG to
/// pay that cost now. It blocks until the warm-up is done (concurrent
/// callers wait for the same one) and returns how long the warm-up took.
/// [`Engine::new`](crate::Engine::new) already starts it on a background
/// thread, so hosts normally never call this.
pub fn svg_warm_up() -> Duration {
    *WARM.get_or_init(|| {
        let t0 = Instant::now();
        let mut doc = blitz_html::HtmlDocument::from_html(
            r#"<!DOCTYPE html><html><body style="margin:0"><svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><path d="M0 0h1v1H0z"/></svg></body></html>"#,
            blitz_dom::DocumentConfig {
                viewport: Some(blitz_traits::shell::Viewport::new(
                    4,
                    4,
                    1.0,
                    blitz_traits::shell::ColorScheme::Light,
                )),
                html_parser_provider: Some(Arc::new(blitz_html::HtmlProvider)),
                font_ctx: Some(crate::fonts::bundled_font_context(
                    &crate::fonts::HostFonts::default(),
                )),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        t0.elapsed()
    })
}

/// True once [`svg_warm_up`] has finished in this process.
pub fn svg_warm_up_done() -> bool {
    WARM.get().is_some()
}

/// Start the warm-up on a background thread, once per process.
pub(crate) fn spawn_svg_warm_up() {
    WARM_SPAWN.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("hx-native-svg-warm-up".into())
            .spawn(|| {
                let _ = svg_warm_up();
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARN: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16'%3E%3Cpath fill='%23000' d='M7 0h2v2H7z'/%3E%3C/svg%3E";

    struct Recorder(Arc<std::sync::Mutex<Vec<String>>>);
    impl NetHandler for Recorder {
        fn bytes(self: Box<Self>, _resolved_url: String, bytes: Bytes) {
            self.0
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&bytes).into_owned());
        }
    }

    fn fetch(p: &DataSvgProvider, url: &str) -> Vec<String> {
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        let req = Request::get(blitz_traits::net::Url::parse(url).unwrap());
        p.fetch(0, req, Box::new(Recorder(Arc::clone(&got))));
        let v = got.lock().unwrap().clone();
        v
    }

    /// The provider gate on its own (not through fragment validation): only
    /// a passing `data:image/svg+xml` URL ever reaches Blitz's handler.
    #[test]
    fn provider_gate() {
        let p = DataSvgProvider::default();
        assert!(p.is_noop());
        let served = fetch(&p, WARN);
        assert_eq!(served.len(), 1);
        assert!(served[0].starts_with("<svg"));
        for url in [
            "file:///etc/hostname",
            "http://127.0.0.1/a.svg",
            "data:text/css,body%7Bbackground:red%7D",
            "data:font/ttf;base64,AAEAAA==",
            "data:image/svg+xml;base64,AAEAAA==",
            "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Ctext%3Ex%3C/text%3E%3C/svg%3E",
            "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Cimage href='/etc/hostname'/%3E%3C/svg%3E",
            "data:image/svg+xml,%3Csvg",
        ] {
            assert!(fetch(&p, url).is_empty(), "provider must drop {url}");
        }
        assert_eq!((p.served(), p.refused()), (1, 8));
    }

    #[test]
    fn forbidden_scan() {
        assert_eq!(
            find_forbidden_element(b"<svg><text>a</text></svg>"),
            Some("text")
        );
        assert_eq!(
            find_forbidden_element(b"<svg><svg:tspan/></svg>"),
            Some("tspan")
        );
        assert_eq!(
            find_forbidden_element(b"<svg><textPath href='#a'/></svg>"),
            Some("textPath")
        );
        assert_eq!(
            find_forbidden_element(b"<svg><image href='/etc/x'/></svg>"),
            None
        );
        assert_eq!(
            find_forbidden_element(b"<svg><feImage/></svg>"),
            Some("feImage")
        );
        assert_eq!(
            find_forbidden_element(b"<svg><textarea/><path d='M0 0'/></svg>"),
            None
        );
        assert_eq!(
            find_forbidden_element(b"<svg><g class='text'/></svg>"),
            None
        );
    }

    #[test]
    fn forbidden_scan_prefixes_and_case() {
        assert_eq!(
            find_forbidden_element("<svg><é:text>".as_bytes()),
            Some("text")
        );
        assert_eq!(
            find_forbidden_element("<svg><a·b:text/>".as_bytes()),
            Some("text")
        );
        assert_eq!(find_forbidden_element(b"<svg><TEXT>"), Some("text"));
        assert_eq!(
            find_forbidden_element(b"<svg><use href='#a'/>"),
            Some("use")
        );
        assert_eq!(find_forbidden_element("<svg><é:image/>".as_bytes()), None);
        assert_eq!(forbidden_element("é:TextPath"), Some("textPath"));
        assert_eq!(forbidden_element("svg:use"), Some("use"));
        assert_eq!(forbidden_element("image"), None);
    }

    #[test]
    fn tree_check_is_namespace_aware() {
        let parse = |s: &'static str| {
            usvg::roxmltree::Document::parse_with_options(
                s,
                usvg::roxmltree::ParsingOptions {
                    allow_dtd: true,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let doc = parse("<svg xmlns='http://www.w3.org/2000/svg' xmlns:é='http://www.w3.org/2000/svg'><é:text>x</é:text></svg>");
        assert_eq!(
            check_xml_tree(&doc),
            Err(SvgRejection::ForbiddenElement {
                element: "text".into()
            })
        );
        let doc = parse("<!DOCTYPE svg [<!ENTITY t \"<tspan/>\">]><svg xmlns='http://www.w3.org/2000/svg'>&t;</svg>");
        assert_eq!(
            check_xml_tree(&doc),
            Err(SvgRejection::ForbiddenElement {
                element: "tspan".into()
            })
        );
        let doc = parse(
            "<svg xmlns='http://www.w3.org/2000/svg'><style>a{stroke-dashoffset:1}</style></svg>",
        );
        assert_eq!(
            check_xml_tree(&doc),
            Err(SvgRejection::ForbiddenProperty {
                property: "stroke-dashoffset".into()
            })
        );
        let doc = parse("<svg xmlns='http://www.w3.org/2000/svg' xmlns:é='http://www.w3.org/2000/svg'><é:image href='/tmp/x.png'/></svg>");
        assert_eq!(
            check_xml_tree(&doc),
            Err(SvgRejection::ImageHref {
                href: "/tmp/x.png".into()
            })
        );
        let doc = parse("<svg xmlns='http://www.w3.org/2000/svg'><g><g><rect/></g></g></svg>");
        assert_eq!(check_xml_tree(&doc), Ok(vec![]));
        let deep = format!(
            "<svg xmlns='http://www.w3.org/2000/svg'>{}{}</svg>",
            "<g>".repeat(SVG_MAX_DEPTH),
            "</g>".repeat(SVG_MAX_DEPTH)
        );
        let doc = usvg::roxmltree::Document::parse(&deep).unwrap();
        assert_eq!(
            check_xml_tree(&doc),
            Err(SvgRejection::TooDeep {
                depth: SVG_MAX_DEPTH + 1,
                limit: SVG_MAX_DEPTH
            })
        );
    }

    #[test]
    fn depth_scan() {
        let deep = |n: usize| format!("<svg>{}{}</svg>", "<g>".repeat(n), "</g>".repeat(n));
        assert_eq!(
            xml_depth_over(deep(SVG_MAX_DEPTH - 1).as_bytes(), SVG_MAX_DEPTH),
            None
        );
        assert_eq!(
            xml_depth_over(deep(SVG_MAX_DEPTH).as_bytes(), SVG_MAX_DEPTH),
            Some(SVG_MAX_DEPTH + 1)
        );
        assert_eq!(xml_depth_over(b"<a><b/><b/><b/><c></c></a>", 2), None);
        assert_eq!(xml_depth_over(b"<a><b><c/></b></a>", 2), None);
        assert_eq!(xml_depth_over(b"<a><b><c></c></b></a>", 2), Some(3));
        assert_eq!(
            xml_depth_over(b"<!DOCTYPE a [<!ENTITY e '<b><c>'>]><?x <q>?><a><!-- <b><c> --><![CDATA[<b><c>]]><b k='>'/></a>", 1),
            None
        );
        assert_eq!(xml_depth_over(b"<a k='/'><b k=\"/\">", 1), Some(2));
    }

    #[test]
    fn css_and_attribute_checks() {
        for css in [
            "stroke-dasharray:1",
            "STROKE-DASHOFFSET:1",
            r"stroke-dash\61rray:1",
            r"stroke-da\shoffset:1",
            "stroke-/**/dasharray:1",
        ] {
            assert!(css_forbidden_property(css).is_some(), "{css}");
        }
        assert_eq!(css_forbidden_property("stroke:red;stroke-width:2"), None);
        let attr = |e, a, v| check_svg_attribute(e, a, v, false).map(|o| o.is_some());
        assert!(matches!(
            attr("path", "stroke-dasharray", "1"),
            Err(SvgRejection::ForbiddenProperty { .. })
        ));
        assert!(matches!(
            attr("path", "x:Stroke-DashOffset", "1"),
            Err(SvgRejection::ForbiddenProperty { .. })
        ));
        assert!(matches!(
            attr("path", "style", "fill:red;stroke-dasharray:1"),
            Err(SvgRejection::ForbiddenProperty { .. })
        ));
        assert_eq!(attr("path", "class", "stroke-dasharray"), Ok(false));
        assert!(matches!(
            attr("linearGradient", "href", "#g"),
            Err(SvgRejection::ExternalHref { .. })
        ));
        assert!(matches!(
            attr("a", "é:HREF", "https://x"),
            Err(SvgRejection::ExternalHref { .. })
        ));
        assert_eq!(attr("a", "href", "data:,x"), Ok(false));
        assert!(matches!(
            attr("svg:image", "href", "/tmp/x.png"),
            Err(SvgRejection::ImageHref { .. })
        ));
    }

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    #[test]
    fn pixel_cap_boundary_and_svg_sniff() {
        let too_large = |width, height| {
            Err(SvgRejection::ImageTooLarge {
                width,
                height,
                limit: DATA_IMAGE_MAX_PIXELS,
            })
        };
        // Area: 256x256 is the cap, one more column or row is over it.
        let ok = check_raster("image/png", png_header(256, 256)).unwrap();
        assert_eq!(ok.pixels, 256 * 256);
        assert_eq!(
            check_raster("image/png", png_header(257, 256)),
            too_large(257, 256)
        );
        assert_eq!(
            check_raster("image/png", png_header(256, 257)),
            too_large(256, 257)
        );
        // GIF (Graph #52 r4): the largest frame counts, not the screen.
        let mut gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00".to_vec();
        gif.extend_from_slice(b"\x2C\x00\x00\x00\x00\x02\x00\x02\x00\x00\x02\x00\x3B");
        assert_eq!(check_raster("image/gif", gif).unwrap().pixels, 4);
        // Side (Graph #52 r2): within the area cap, a side over 4096 is
        // refused; vello_cpu panics on every paint above 65535.
        assert!(check_raster("image/png", png_header(4096, 16)).is_ok());
        assert!(check_raster("image/png", png_header(16, 4096)).is_ok());
        assert_eq!(
            check_raster("image/png", png_header(4097, 1)),
            too_large(4097, 1)
        );
        assert_eq!(
            check_raster("image/png", png_header(1, 4097)),
            too_large(1, 4097)
        );
        assert_eq!(
            check_raster("image/png", png_header(1, 65_536)),
            too_large(1, 65_536)
        );
        assert_eq!(
            check_raster("image/png", png_header(1, 16_777_216)),
            too_large(1, 16_777_216)
        );
        assert_eq!(
            check_raster("image/png", png_header(70_000, 1)),
            too_large(70_000, 1)
        );
        assert_eq!(
            check_raster("image/gif", b" <svg/>".to_vec()),
            Err(SvgRejection::ImageTypeMismatch {
                declared: "image/gif".into(),
                actual: "image/svg+xml".into()
            })
        );
    }

    #[test]
    fn classify() {
        assert!(matches!(classify_url(WARN), DataSvg::Ok(..)));
        assert_eq!(classify_url("file:///etc/hostname"), DataSvg::NotSvg);
        assert_eq!(classify_url("http://example.com/a.svg"), DataSvg::NotSvg);
        assert_eq!(classify_url("data:text/css,a{}"), DataSvg::NotSvg);
        assert_eq!(
            classify_url("data:font/ttf;base64,AAEAAA=="),
            DataSvg::NotSvg
        );
        // A font hidden behind the SVG MIME type: TTF magic 00 01 00 00.
        assert_eq!(
            classify_url("data:image/svg+xml;base64,AAEAAA=="),
            DataSvg::Rejected(SvgRejection::Malformed {
                detail: "payload does not start with `<`".into()
            })
        );
        assert_eq!(
            classify_url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Ctext%3Ex%3C/text%3E%3C/svg%3E"),
            DataSvg::Rejected(SvgRejection::ForbiddenElement {
                element: "text".into()
            })
        );
        assert!(matches!(
            classify_url("data:image/svg+xml,%3Csvg%3E%3Cg%3E"),
            DataSvg::Rejected(SvgRejection::Malformed { .. })
        ));
    }

    #[test]
    fn size_cap_is_exact() {
        let pad = DATA_SVG_MAX_BYTES - "<svg xmlns='http://www.w3.org/2000/svg'></svg>".len();
        let ok = format!(
            "<svg xmlns='http://www.w3.org/2000/svg'>{}</svg>",
            " ".repeat(pad)
        );
        assert_eq!(ok.len(), DATA_SVG_MAX_BYTES);
        assert_eq!(check_svg_bytes(ok.as_bytes()), Ok(vec![]));
        let big = format!("{ok} ");
        assert!(matches!(
            check_svg_bytes(big.as_bytes()),
            Err(SvgRejection::TooLarge { .. })
        ));
    }

    #[test]
    fn css_url_escapes_strings_and_comments() {
        // Escaped `data:` is decoded the way Stylo decodes it.
        assert_eq!(
            css_urls(r"a{b:url(da\74 a:image/svg+xml,x)}"),
            vec!["data:image/svg+xml,x".to_string()]
        );
        // image-set strings that start with data: are collected.
        assert_eq!(
            css_urls(r#"a{b:image-set("data:image/svg+xml,y" 1x)}"#),
            vec!["data:image/svg+xml,y".to_string()]
        );
        // An apostrophe in a comment does not start a string.
        assert_eq!(
            css_urls("/* don't */ a{b:url(data:z,1)}"),
            vec!["data:z,1".to_string()]
        );
    }

    #[test]
    fn css_url_scan() {
        let css =
            r#"a{--i:url("data:image/svg+xml,%3Csvg x='1'%3E");b:URL( 'x.png' );c:url(data:a,b)}"#;
        assert_eq!(
            css_urls(css),
            vec![
                "data:image/svg+xml,%3Csvg x='1'%3E".to_string(),
                "x.png".to_string(),
                "data:a,b".to_string()
            ]
        );
    }
}
