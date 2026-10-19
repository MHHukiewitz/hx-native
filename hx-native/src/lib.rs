//! `hx_native` — in-process HTMX-style fragment engine (offline, no JS, no network).
//!
//! See `docs/REQUIREMENTS.md` for the locked contract.

#![deny(unsafe_code)]

mod dialect;
mod engine;
mod error;
mod focus;
mod fonts;
mod paint;
mod pending;
mod trigger;
mod types;

pub use dialect::{SwapMode, SyncMode};
pub use engine::{DirtySet, Engine};
pub use error::EngineError;
pub use pending::PendingTimeout;
pub use trigger::{FilterExpr, FromTarget, ParsedTrigger, TriggerEvent};
pub use types::*;

/// Crate version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
