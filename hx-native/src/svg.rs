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
//! most [`DATA_IMAGE_MAX_PIXELS`] (every frame of a GIF), and then one trial
//! decode with Blitz's own decoder ([`probe_decode`]). Any failure rejects
//! the whole load or swap with [`SvgRejection`]; nothing is ever fetched.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};

use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};

/// Largest decoded `data:image/svg+xml` payload hx-native accepts, per URL
/// (64 KiB). Mainnet's icons are about 500 bytes. Raise only with a measured
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
pub const FORBIDDEN_SVG_ELEMENTS: &[&str] = &["text", "tspan", "textPath", "feImage"];

/// The only `href` / `xlink:href` an SVG `<image>` may carry: a `data:` URL
/// of one of these MIME types (matched case-insensitively, as usvg does).
/// Any other href (`http(s):`, `file:`, a path, `#fragment`, `data:` of
/// another type, including `image/svg+xml`, `image/jpg` and `text/plain`)
/// rejects the load or swap. A nested SVG is refused because usvg would
/// parse it with the host's options, where a bare path is read from disk.
pub const SVG_IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Largest decoded image an SVG `<image>` may declare: width x height at
/// most 4096 x 4096 (16 Mi pixels, 64 MiB RGBA), checked from the file
/// header before any decode. For GIF the logical screen and every frame are
/// checked. Raise only with a measured case.
pub const DATA_IMAGE_MAX_PIXELS: u64 = 4096 * 4096;

/// Why an SVG was rejected. Carried by
/// [`EngineError::InvalidSvg`](crate::EngineError::InvalidSvg).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SvgRejection {
    /// The SVG contains one of [`FORBIDDEN_SVG_ELEMENTS`].
    ForbiddenElement { element: String },
    /// The decoded `data:` payload is larger than [`DATA_SVG_MAX_BYTES`].
    TooLarge { bytes: usize, limit: usize },
    /// Not SVG markup, bad base64, or usvg could not parse it.
    Malformed { detail: String },
    /// An `<image>` href that is not a `data:` URL of one of
    /// [`SVG_IMAGE_MIME_TYPES`] (shortened to 80 characters).
    ImageHref { href: String },
    /// The image bytes are a different format from the declared MIME type.
    ImageTypeMismatch { declared: String, actual: String },
    /// The image header declares more than [`DATA_IMAGE_MAX_PIXELS`].
    ImageTooLarge { width: u64, height: u64, limit: u64 },
    /// The image header is unreadable or the decoder refused the data.
    CorruptImage { mime: String, detail: String },
}

impl std::fmt::Display for SvgRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SvgRejection::ForbiddenElement { element } => {
                write!(f, "SVG contains a forbidden <{element}> element")
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
            SvgRejection::CorruptImage { mime, detail } => {
                write!(f, "corrupt {mime} in SVG <image>: {detail}")
            }
        }
    }
}

/// First forbidden element name in raw SVG/XML text, if any. Matches
/// `<name` and `<prefix:name` followed by whitespace, `/` or `>`. Text in
/// comments or CDATA that looks like such a tag also matches: the gate fails
/// closed.
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
            && (src[end].is_ascii_alphanumeric() || matches!(src[end], b':' | b'_' | b'-' | b'.'))
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
                .find(|f| f.as_bytes() == local)
            {
                return Some(hit);
            }
        }
        i = end.max(start);
    }
    None
}

/// An SVG `<image>` payload that passed the static checks, waiting for the
/// trial decode in [`probe_decode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RasterImage {
    pub(crate) mime: &'static str,
    pub(crate) data: Arc<Vec<u8>>,
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
    for (w, h) in dims {
        if w == 0 || h == 0 {
            return Err(corrupt("zero-sized image"));
        }
        if w * h > DATA_IMAGE_MAX_PIXELS {
            return Err(SvgRejection::ImageTooLarge {
                width: w,
                height: h,
                limit: DATA_IMAGE_MAX_PIXELS,
            });
        }
    }
    Ok(RasterImage {
        mime: declared,
        data: Arc::new(data),
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

/// PNG: the IHDR chunk must come first.
fn png_dimensions(d: &[u8]) -> Option<(u64, u64)> {
    if be32(d, 8)? != 13 || d.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be32(d, 16)?, be32(d, 20)?))
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
/// frame only; memory is bounded by [`DATA_IMAGE_MAX_PIXELS`].
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
    if let Some(element) = find_forbidden_element(body) {
        return Err(SvgRejection::ForbiddenElement {
            element: element.to_string(),
        });
    }
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
    usvg::Tree::from_data(body, &opts).map_err(|e| SvgRejection::Malformed {
        detail: e.to_string(),
    })?;
    drop(opts);
    let found = found.into_inner().unwrap_or_default();
    found.into_iter().collect()
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
