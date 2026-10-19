//! Engine errors.

use std::fmt;

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
    /// Paint / layout failure.
    Paint(String),
    /// Engine used before `load_html`.
    NotLoaded,
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
            EngineError::Paint(msg) => write!(f, "paint error: {msg}"),
            EngineError::NotLoaded => write!(f, "engine has no loaded HTML"),
            EngineError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for EngineError {}
