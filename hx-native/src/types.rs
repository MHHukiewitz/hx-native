//! Public types from the locked API sketch.

use std::fmt;

/// Engine configuration: offline, no network, no JS.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Hard per-frame paint budget (nanoseconds). Host aligns with FRAME_BUDGET.
    pub max_paint_ns: u64,
    /// Backstop for a missing `complete()`: after this many frames a pending
    /// request is dropped. Debug builds warn; test harnesses fail loudly.
    pub pending_timeout_frames: u32,
    /// Partial-raster strategy. Default is [`PaintMode::Exact`] (byte-identical).
    pub paint_mode: PaintMode,
    /// Upper bound on estimated bytes held by hidden, unpinned, live surfaces.
    /// Above it, the least recently used of them are evicted (see
    /// [`Engine::set_visible`](crate::Engine::set_visible)). Defaults to
    /// [`DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES`].
    pub hidden_surface_budget_bytes: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_paint_ns: 8_000_000, // ~8ms default headroom inside a 16.6ms frame
            pending_timeout_frames: 120,
            paint_mode: PaintMode::Exact,
            hidden_surface_budget_bytes: DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES,
        }
    }
}

const MIB: u64 = 1024 * 1024;

/// Default for [`EngineConfig::hidden_surface_budget_bytes`].
pub const DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES: u64 = 16 * MIB;

/// When the estimated bytes of all live pinned surfaces exceed this, the engine
/// logs a warning (pinned surfaces are never evicted).
pub const PINNED_SURFACE_WARN_BYTES: u64 = 32 * MIB;

/// Layout size of a surface created by [`Engine::load_html`](crate::Engine::load_html)
/// before the host has passed a viewport to `paint`.
pub const DEFAULT_SURFACE_WIDTH: u32 = 800;
/// See [`DEFAULT_SURFACE_WIDTH`].
pub const DEFAULT_SURFACE_HEIGHT: u32 = 120;

/// Host-chosen key for one HTML document + RGBA buffer inside an [`Engine`](crate::Engine).
///
/// [`SurfaceId::DEFAULT`] is the surface the single-document API
/// (`load_html`, `sync`, `input`, `paint(viewport)`, ...) operates on.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub struct SurfaceId(pub u64);

impl SurfaceId {
    pub const DEFAULT: SurfaceId = SurfaceId(0);
}

impl fmt::Display for SurfaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "surface#{}", self.0)
    }
}

/// Options for [`Engine::open_surface`](crate::Engine::open_surface).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SurfaceOpts {
    /// Hidden surfaces still run style, layout and a first raster on open
    /// (pre-drawn), but are never reported by `dirty_surfaces` unless pinned.
    pub visible: bool,
    /// Pinned surfaces are never evicted. Their bytes are reported by
    /// [`Engine::pinned_bytes`](crate::Engine::pinned_bytes).
    pub pinned: bool,
}

impl Default for SurfaceOpts {
    fn default() -> Self {
        Self {
            visible: true,
            pinned: false,
        }
    }
}

/// Snapshot of one surface's host-visible state.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceInfo {
    pub visible: bool,
    pub pinned: bool,
    /// True when the document and buffer were dropped by the hidden-surface
    /// budget. `set_visible(true)` rebuilds it from the stored HTML.
    pub evicted: bool,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    /// Estimated engine bytes ([`crate::estimate_surface_engine_bytes`]); 0 when evicted.
    pub estimated_bytes: u64,
}

/// Engine-wide surface memory accounting (estimates; see [`crate::estimate_surface_engine_bytes`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SurfaceMemoryStats {
    pub live_surfaces: usize,
    pub evicted_surfaces: usize,
    /// Estimated bytes of every live surface.
    pub live_bytes: u64,
    /// Estimated bytes of hidden, unpinned, live surfaces (what the budget limits).
    pub hidden_unpinned_bytes: u64,
    /// Estimated bytes of live pinned surfaces (never evicted).
    pub pinned_bytes: u64,
    /// [`EngineConfig::hidden_surface_budget_bytes`].
    pub hidden_budget_bytes: u64,
    /// Surfaces evicted since engine creation.
    pub eviction_count: u64,
    /// Wall time of the most recent eviction (dropping document + buffer), in ns.
    pub last_eviction_ns: u64,
    /// Evicted surfaces rebuilt by `set_visible(true)` since engine creation.
    pub rebuild_count: u64,
    /// Wall time of the most recent cold rebuild (parse + style + layout), in ns.
    pub last_rebuild_ns: u64,
}

/// How partial damage rects are rasterized.
///
/// [`PaintMode::Exact`] (default) is byte-identical to a full repaint. Mainnet
/// stays on Exact unless Graph signs off on Shifted.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PaintMode {
    /// Cull commands outside the rect, push a full-size clip layer, render at
    /// full surface size, then copy the rect. Byte-identical to a full paint.
    #[default]
    Exact,
    /// Opt-in faster path: shift and render into a rect-sized target. Can differ
    /// by up to 1/255 per channel inside the rect.
    Shifted,
}

/// Deterministic: starts at 1 per engine, +1 per emitted Action (incl. queued
/// releases), never reused, never persisted. One counter per engine, shared by
/// every surface.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub struct RequestId(pub u64);

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One plain host string (the same rule as Disabled/Blocked reasons).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reason(pub String);

impl From<&str> for Reason {
    fn from(s: &str) -> Self {
        Reason(s.to_string())
    }
}

impl From<String> for Reason {
    fn from(s: String) -> Self {
        Reason(s)
    }
}

/// Published by the host as the reserved `actions` slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionState {
    Enabled,
    /// One plain string; the crate never invents copy.
    Disabled {
        reason: String,
    },
}

/// Host hook: renders HTML for one section. Called only for dirty sections
/// (plus once for every section right after load).
pub trait FragmentSource {
    fn render(&mut self, section_id: &str, slices: &[&str]) -> String;
}

/// Host-owned slice versions from the published end-of-tick snapshot.
pub trait SliceSource {
    fn version(&self, name: &str) -> Option<u64>;
    /// Only for the reserved `actions` slice (greyed-before-click).
    fn action_states(&self) -> Vec<(String, ActionState)>;
}

/// Input the host (UI) converts from egui/winit. Raw: no platform remapping.
#[derive(Clone, Debug)]
pub enum InputEvent {
    PointerMove {
        x: f32,
        y: f32,
        mods: Modifiers,
    },
    PointerDown {
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Modifiers,
    },
    PointerUp {
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Modifiers,
    },
    PointerLeave,
    Key(KeyEvent),
    /// Reserved: no text fields in the first scope; returned as Unhandled.
    Text(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// DOM `key` value, e.g. "Enter", "Escape", "a", "A"
    pub key: String,
    /// DOM `code` value, e.g. "KeyA" (layout-independent)
    pub code: String,
    pub mods: Modifiers,
    /// OS auto-repeat
    pub repeat: bool,
    /// true = keydown, false = keyup
    pub pressed: bool,
}

/// What fired an Action/Blocked.
///
/// `#[non_exhaustive]` so adding variants (e.g. `Change` in #14) does not break
/// exhaustive matches in Mainnet / other downstream crates.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerInfo {
    Pointer {
        event: &'static str,
        button: PointerButton,
        mods: Modifiers,
    },
    Key {
        event: &'static str,
        key: String,
        code: String,
        mods: Modifiers,
        repeat: bool,
    },
    /// Commit-only form control change (range release, toggle, select pick, step).
    Change,
    Slice {
        name: String,
    },
}

/// Focus state reported to the host (Esc stays in Rust / Game).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusState {
    pub focused_id: Option<String>,
}

/// Built-in timing for the last sync/paint (ns).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub sync_ns: u64,
    pub paint_ns: u64,
    /// True when `paint` skipped raster because nothing was dirty (spec: skip clean frames).
    pub paint_skipped: bool,
}

/// Cap on damage rects after merge. Above this, rects collapse to their bounding box.
pub const MAX_DAMAGE_RECTS: usize = 8;

/// When total damage area exceeds this fraction of the surface, fall back to a full repaint.
pub const FULL_REPAINT_AREA_FRACTION: f64 = 0.5;

/// Per-frame paint diagnostics (damage merge / full-repaint fallbacks).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Number of damage rects after merge (0 when skipped; 1 for a full surface).
    pub damage_rect_count: usize,
    /// True when more than [`MAX_DAMAGE_RECTS`] rects collapsed to a bbox.
    pub merged_to_bbox: bool,
    /// True when damage area exceeded [`FULL_REPAINT_AREA_FRACTION`].
    pub full_repaint_area_fallback: bool,
    /// True when this frame was a forced full repaint (first frame, resize, scale,
    /// theme/stylesheet, or area fallback).
    pub full_repaint: bool,
    /// True when `damage` was widened to the full surface because the surface
    /// just became visible (or was rebuilt), so the host must re-upload it all.
    pub full_host_upload: bool,
}

/// RGBA8 image buffer (software / G1 path).
#[derive(Clone, Debug)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Opaque host texture handle (G2 path; unused in Layer 1 crate unit).
#[derive(Clone, Debug)]
pub struct HostTextureRef {
    pub width: u32,
    pub height: u32,
    pub handle: u64,
}

#[derive(Clone, Debug)]
pub struct PaintOutput {
    /// Fresh RGBA buffer when this paint rasterized (`skipped == false`).
    ///
    /// When `skipped` is `true`, this is always `None` — the engine does not
    /// copy the cached frame. The host should keep its last uploaded texture
    /// (Mainnet `src/ui/hx_time`) and ignore `rgba` on skipped frames. Use
    /// [`Engine::last_frame`](crate::Engine::last_frame) if a borrowed view of
    /// the cached buffer is needed without a copy.
    pub rgba: Option<RgbaImage>,
    pub texture: Option<HostTextureRef>,
    pub focus: FocusState,
    /// True when raster was skipped (clean frame). Then `rgba` is `None` and
    /// `damage` is empty.
    pub skipped: bool,
    /// Changed regions in integer device pixels (rounded outward, clamped to
    /// the surface). Empty when skipped. One full-surface rect on a full repaint.
    /// The host uploads only these rects with `write_texture`.
    pub damage: Vec<Rect>,
}

#[derive(Clone, Debug)]
pub struct Viewport {
    /// Physical (device-pixel) width of the paint surface.
    pub width: u32,
    /// Physical (device-pixel) height of the paint surface.
    pub height: u32,
    /// Device-pixel ratio / hidpi scale (1.0 = 1 CSS px per device px).
    pub scale: f64,
    pub holes: Vec<Hole>,
}

impl Viewport {
    /// Build a viewport at scale 1.0 with no holes.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            scale: 1.0,
            holes: Vec::new(),
        }
    }

    /// Build a viewport at the given scale with no holes.
    pub fn with_scale(width: u32, height: u32, scale: f64) -> Self {
        Self {
            width,
            height,
            scale,
            holes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hole {
    pub rect: Rect,
    pub texture: HostTextureRef,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Reported in pointer order: no-click -> Hover -> Press -> Clicked/Action.
#[derive(Clone, Debug, PartialEq)]
pub enum InputOutcome {
    None,
    Hover {
        id: Option<String>,
    },
    Press {
        id: Option<String>,
    },
    Clicked {
        id: String,
    },
    /// The ONLY action path. Host queues it and runs it between ticks.
    Action {
        request: RequestId,
        name: String,
        args: Vec<(String, String)>,
        source_id: String,
        trigger: TriggerInfo,
    },
    /// Race fallback: click landed on an action that was Disabled.
    Blocked {
        name: String,
        reason: String,
        source_id: String,
        trigger: TriggerInfo,
    },
    FocusChanged(FocusState),
    /// Key the engine did not consume. Host routes to game hotkeys / Esc stack.
    Unhandled(KeyEvent),
}
