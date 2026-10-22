//! Engine errors.

use std::fmt;

use crate::types::SurfaceId;

/// `#[non_exhaustive]` (PR27 round 2, one-time breaking change): new
/// variants (e.g. `UnknownLimitInput` / `InvalidInputLimits`, issue #16) must
/// not break an exhaustive `match` in Mainnet or other downstream crates
/// going forward. A `match` without a wildcard arm needs one added once, for
/// this change.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// `hx-post` names an action not passed to `declare_actions`.
    UndeclaredAction { name: String, source_id: String },
    /// Unknown `hx-trigger` event, filter, operator, or `from:` target.
    InvalidTrigger { source_id: String, detail: String },
    /// Unsupported `hx-swap` value (outside innerHTML / outerHTML).
    UnsupportedSwap { value: String, source_id: String },
    /// Unsupported `hx-sync` value (outside drop / queue).
    UnsupportedSync { value: String, source_id: String },
    /// Attribute outside the locked dialect, or an invalid dialect value/target.
    /// Used for unknown `hx-*` / `data-hx-*` names, engine-only `data-hx-reason`
    /// in loaded HTML, and `hx-target` / `hx-indicator` that are not a resolving `#id`.
    InvalidAttribute {
        id: String,
        attr: String,
        reason: String,
    },
    /// `SliceSource::input_limits` (issue #16) names a range/number input
    /// that does not exist inside any `<form>` that does exist for `action`.
    /// **Not** raised when no form for `action` is in the document at all
    /// (it may be on another surface, or simply not rendered yet).
    UnknownLimitInput { action: String, input: String },
    /// A `SliceSource::input_limits` (issue #16) entry failed validation:
    /// non-finite `min`/`max`, `min > max`, or a non-finite / non-positive
    /// `step` when `Some`.
    InvalidInputLimits {
        action: String,
        input: String,
        reason: String,
    },
    /// An [`EngineConfig::fonts`](crate::EngineConfig::fonts) entry (issue
    /// #28) was rejected when the engine was built: bytes that are not a
    /// readable single-face TTF/OTF/WOFF/WOFF2 (any collection, i.e. a
    /// `ttcf` file tag even with one face, is rejected), an empty family, a
    /// weight outside `1..=1000`, a duplicate of an earlier entry (same
    /// family compared case-insensitively per character as fontique does,
    /// same weight and style), or a face that ties with a bundled
    /// "DejaVu Sans" face, 400 or 700 normal (#56; width is not compared, so a condensed face
    /// under that name is refused too). The byte check covers the tables
    /// fontique reads to register a face, not glyph outlines: a corrupt
    /// outline table passes and paints missing glyphs. `index` is the
    /// entry's position in `EngineConfig::fonts`.
    InvalidFont {
        index: usize,
        family: String,
        reason: String,
    },
    /// Paint / layout failure.
    Paint(String),
    /// Engine used before `load_html`.
    NotLoaded,
    /// No surface with this id is open.
    UnknownSurface(SurfaceId),
    /// The surface was evicted by the hidden-surface budget; `set_visible(id, true)`
    /// rebuilds it.
    SurfaceEvicted(SurfaceId),
    /// Internal invariant broken (tests fail loudly).
    Internal(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::UndeclaredAction { name, source_id } => {
                write!(f, "undeclared action `{name}` on element `{source_id}`")
            }
            EngineError::InvalidTrigger { source_id, detail } => {
                write!(f, "invalid hx-trigger on `{source_id}`: {detail}")
            }
            EngineError::UnsupportedSwap { value, source_id } => {
                write!(f, "unsupported hx-swap `{value}` on `{source_id}`")
            }
            EngineError::UnsupportedSync { value, source_id } => {
                write!(f, "unsupported hx-sync `{value}` on `{source_id}`")
            }
            EngineError::InvalidAttribute { id, attr, reason } => {
                write!(f, "invalid attribute `{attr}` on `{id}`: {reason}")
            }
            EngineError::UnknownLimitInput { action, input } => write!(
                f,
                "input_limits names unknown input `{input}` for action `{action}`"
            ),
            EngineError::InvalidFont {
                index,
                family,
                reason,
            } => write!(f, "invalid host font #{index} (`{family}`): {reason}"),
            EngineError::InvalidInputLimits {
                action,
                input,
                reason,
            } => write!(f, "invalid input_limits for `{action}.{input}`: {reason}"),
            EngineError::Paint(msg) => write!(f, "paint error: {msg}"),
            EngineError::NotLoaded => write!(f, "engine has no loaded HTML"),
            EngineError::UnknownSurface(id) => write!(f, "{id} is not open"),
            EngineError::SurfaceEvicted(id) => {
                write!(f, "{id} was evicted; set_visible(true) rebuilds it")
            }
            EngineError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for EngineError {}
