//! `hx_native` — in-process HTMX-style fragment engine (offline, no JS, no network).
//!
//! See `docs/REQUIREMENTS.md` for the locked contract.
//!
//! For a background paint thread (issue #8 step 3), see [`PaintWorker`] and the
//! rustdoc on the [`worker`] module (Mainnet / wgpu / egui host notes).

#![deny(unsafe_code)]

mod damage;
mod dialect;
mod ellipsis;
mod engine;
mod error;
mod fields;
mod focus;
mod fonts;
mod form_args;
mod form_controls;
mod inset_shadow;
mod memory;
mod paint;
mod pending;
mod radio;
mod scroll;
mod select_ui;
mod slider;
mod stylo_text_style;
mod svg;
mod text_edit;
mod text_shadow;
mod tile_coalesce;
mod trigger;
mod types;
pub mod worker;

pub use damage::{element_border_box_css, element_border_box_device};
pub use dialect::{
    SwapMode, SyncMode, ATTR_HX_APPLIED, ATTR_HX_GROUP, ATTR_HX_STEP, ATTR_HX_STEP_FOR,
    SUPPORTED_HX_ATTRIBUTES,
};
pub use engine::{DirtySet, Engine};
pub use error::EngineError;
pub use form_args::{args_get, args_get_all};
#[doc(hidden)]
pub use inset_shadow::{
    set_inset_shadow_fast_path_for_test, take_inset_shadow_counters_for_test, InsetShadowCounters,
};
pub use memory::{
    estimate_document_bytes, estimate_rgba_bytes, estimate_surface_engine_bytes,
    DOCUMENT_ESTIMATE_BASE_BYTES, DOCUMENT_ESTIMATE_PER_HTML_BYTE, RGBA_BYTES_PER_PIXEL,
};
pub use pending::PendingTimeout;
pub use svg::{
    svg_warm_up, svg_warm_up_done, SvgRejection, DATA_IMAGE_MAX_PIXELS, DATA_IMAGE_MAX_SIDE,
    DATA_IMAGE_MAX_TOTAL_PIXELS, DATA_SVG_MAX_BYTES, FORBIDDEN_SVG_ELEMENTS,
    FORBIDDEN_SVG_PROPERTIES, SVG_IMAGE_MIME_TYPES, SVG_MAX_DEPTH,
};
#[doc(hidden)]
pub use text_shadow::{take_text_shadow_counters_for_test, TextShadowCounters};
#[doc(hidden)]
pub use tile_coalesce::{
    clear_tile_cache_for_test, set_tile_coalesce_for_test, take_tile_coalesce_counters_for_test,
    TileCoalesceCounters,
};
pub use trigger::{FilterExpr, FromTarget, ParsedTrigger, TriggerEvent};
pub use types::*;
pub use worker::{
    FrameView, PaintWorker, SyncPacket, WorkerError, WorkerFrame, WorkerOutcome, WorkerStatus,
    DAMAGE_HISTORY_GENERATIONS, PAINT_WORKER_COMMAND_CAPACITY, PAINT_WORKER_DISCRETE_HARD_CAP,
    PAINT_WORKER_DISCRETE_SOFT_CAP, PAINT_WORKER_FRAME_SLOTS, PAINT_WORKER_JOIN_WARN_NS,
    WORKER_STALL_TIMEOUT_NS,
};

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
        assert_send::<SyncPacket>();
        assert_send::<WorkerFrame>();
        assert_send::<FrameView>();
        assert_send::<WorkerOutcome>();
    }
    let _ = check;
};
