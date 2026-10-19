//! `hx_native` — in-process HTMX-style fragment engine (offline, no JS, no network).
//!
//! See `docs/REQUIREMENTS.md` for the locked contract.

#![deny(unsafe_code)]

mod damage;
mod dialect;
mod engine;
mod error;
mod focus;
mod fonts;
mod form_args;
mod memory;
mod paint;
mod pending;
mod trigger;
mod types;

pub use dialect::{SwapMode, SyncMode};
pub use engine::{DirtySet, Engine};
pub use error::EngineError;
pub use memory::{
    estimate_document_bytes, estimate_rgba_bytes, estimate_surface_engine_bytes,
    DOCUMENT_ESTIMATE_BASE_BYTES, DOCUMENT_ESTIMATE_PER_HTML_BYTE, RGBA_BYTES_PER_PIXEL,
};
pub use pending::PendingTimeout;
pub use trigger::{FilterExpr, FromTarget, ParsedTrigger, TriggerEvent};
pub use types::*;

/// Crate version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Paint-state types that step 3 must move across threads. A Blitz bump that
/// makes any of these `!Send` fails the build of this crate (not only a test).
const _: () = {
    fn assert_send<T: Send>() {}
    fn check() {
        assert_send::<RgbaImage>();
        assert_send::<PaintOutput>();
        assert_send::<PaintStats>();
        assert_send::<Timings>();
    }
    let _ = check;
};
