//! Public types from the locked API sketch.

use std::fmt;
use std::sync::Arc;

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
    /// Host-supplied font faces (issue #28). Empty by default, which keeps
    /// the bundled-DejaVu-only behaviour. Pass both the regular and the bold
    /// face of a family: Blitz paints no synthetic bold (issue #56). Faces
    /// are registered in this order, after bundled DejaVu Sans 400 and 700,
    /// under the family / weight / style
    /// given here (not the names inside the font file). DejaVu stays the
    /// generic (`sans-serif`, `serif`, `monospace`, `system-ui`) and
    /// unknown-family fallback. CSS `@font-face` rules stay ignored: the
    /// engine never fetches resources. Invalid entries fail
    /// [`Engine::try_new`](crate::Engine::try_new) with
    /// [`EngineError::InvalidFont`](crate::EngineError::InvalidFont).
    pub fonts: Vec<FontFace>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_paint_ns: 8_000_000, // ~8ms default headroom inside a 16.6ms frame
            pending_timeout_frames: 120,
            paint_mode: PaintMode::Exact,
            hidden_surface_budget_bytes: DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES,
            fonts: Vec::new(),
        }
    }
}

/// Font style of a host-supplied [`FontFace`] (issue #28). Crate-owned so no
/// Blitz / fontique type leaks into the public API; maps onto CSS
/// `font-style: normal | italic | oblique`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// One host-supplied font face (issue #28), registered under `family`,
/// `weight` and `style` regardless of the names inside the font file.
/// `bytes` is one single-face TTF, OTF, WOFF or WOFF2 file (collections such
/// as `.ttc` are rejected). `Arc<[u8]>` lets a host share one buffer
/// (`Arc::from(BYTES as &[u8])` once at startup, or bytes loaded at runtime)
/// across engines: for TTF/OTF every engine uses that same allocation, no
/// copy; a WOFF/WOFF2 is decompressed once per engine at build time.
///
/// `#[non_exhaustive]`: build one with [`FontFace::new`].
#[non_exhaustive]
#[derive(Clone)]
pub struct FontFace {
    /// CSS family name the face answers to (`font-family: <family>`).
    pub family: String,
    /// CSS weight, `1..=1000` (400 regular, 700 bold).
    pub weight: u16,
    pub style: FontStyle,
    pub bytes: Arc<[u8]>,
}

impl FontFace {
    pub fn new(
        family: impl Into<String>,
        weight: u16,
        style: FontStyle,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        Self {
            family: family.into(),
            weight,
            style,
            bytes: bytes.into(),
        }
    }
}

impl fmt::Debug for FontFace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontFace")
            .field("family", &self.family)
            .field("weight", &self.weight)
            .field("style", &self.style)
            .field("bytes_len", &self.bytes.len())
            .finish()
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
///
/// `#[non_exhaustive]` (Graph review 17:01, one-time breaking change
/// alongside adding `RequiresChange` — see docs/REQUIREMENTS.md): every
/// downstream exhaustive match on `ActionState` needs a `_` arm from here
/// on, the same as `InputEvent` / `InputOutcome` already require.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActionState {
    Enabled,
    /// One plain string; the crate never invents copy.
    Disabled {
        reason: String,
    },
    /// Issue #15 (locked by Mike, Floor 2026-10-19 22:02 America/Asuncion):
    /// greyed exactly like `Disabled` while the named `<form id>`'s live
    /// control values equal their baseline (clean). The first edit that
    /// changes a value ungreys it; editing back to the baseline greys it
    /// again. Re-evaluated locally on every value change, no Game round
    /// trip. `form` not in the document -> treated as `Enabled` (debug warn).
    RequiresChange {
        form: String,
        reason: String,
    },
}

/// Issue #15: an input-level dependency rule, keyed under the action that
/// posts the form owning the dependent input (not its own reserved slice).
///
/// `#[non_exhaustive]` (Graph review, PR #24 follow-up): unlike `ActionState`,
/// this enum is young enough that downstream exhaustive matches should not
/// be allowed to silently become exhaustive again when a new rule kind is
/// added later — every match needs a wildcard arm from the start.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputRule {
    /// Grey the dependent input (same three attributes as `Disabled`) while
    /// the named checkbox in the same form has `checked != checked`.
    EnabledWhen {
        input: String,
        checked: bool,
        reason: String,
    },
}

/// Per-input numeric bounds declared via [`SliceSource::input_limits`] (issue
/// #16: `actions.<action>.limits.<input>`). Samples/mocks omit `min`/`max`/
/// `step` on `<input type=range|number>`; the host's `actions` slice is the
/// source of truth and always wins over markup.
///
/// `step: None` means any value (htmx `step="any"`); the engine still writes
/// `step="any"` and falls back to a step of `1` for arrow/spin stepping,
/// matching the existing missing-`step` default.
///
/// `#[non_exhaustive]` (PR27 round 2): a prospective future field must not
/// break downstream struct literals. Build one with [`InputLimits::new`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct InputLimits {
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
}

impl InputLimits {
    pub fn new(min: f64, max: f64, step: Option<f64>) -> Self {
        Self { min, max, step }
    }
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
    /// Issue #15: dependent-input rules, keyed by the action whose form owns
    /// the input. `(action, input name, rule)`. Default empty (non-breaking).
    fn input_rules(&self) -> Vec<(String, String, InputRule)> {
        Vec::new()
    }

    /// `(action, input name) -> InputLimits`, from the reserved `actions`
    /// slice (issue #16: `actions.<action>.limits.<input>`). Default: none —
    /// a non-breaking addition, so existing `SliceSource` implementors
    /// compile unchanged.
    ///
    /// The engine calls this only on the initial `sync` and whenever the
    /// `actions` slice version changes (Game's request: limits are fixed per
    /// build, so hand them over once rather than on every tick). It caches
    /// the result and re-applies it from the cache whenever a later swap
    /// re-renders a matching input.
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        Vec::new()
    }
}

/// Input the host (UI) converts from egui/winit. Raw: no platform remapping.
///
/// `#[non_exhaustive]` so adding variants (Text/Paste/Copy/Cut, …) does not
/// break exhaustive matches in Mainnet / other downstream crates.
#[non_exhaustive]
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
    /// Committed text for a focused `<input type=text|number>`.
    /// Without a focused editable field, returned as [`InputOutcome::Unhandled`].
    Text(String),
    /// Host-supplied paste text for the focused editable field (no system clipboard
    /// inside the engine). Without focus, returned as [`InputOutcome::None`].
    Paste(String),
    /// Request a copy of the current selection. Yields
    /// [`InputOutcome::ClipboardWrite`] when there is selected text.
    Copy,
    /// Cut the current selection: [`InputOutcome::ClipboardWrite`] plus delete.
    Cut,
    /// Mouse wheel / trackpad scroll (issue #26), forwarded raw from egui's
    /// `Event::MouseWheel`. `pos` is panel-local CSS px (the same space as
    /// [`InputEvent::PointerMove`]); `delta` keeps egui's sign (positive `y` =
    /// content moves down = scroll up). Only `delta.1` is used (vertical
    /// scrolling only); a wheel with `delta.1 == 0` is ignored.
    ///
    /// Scrolls the innermost `overflow-y: auto | scroll` container under `pos`
    /// that can still move that way; never a parent of it in the same event
    /// beyond that choice, never a host action. Returns
    /// [`InputOutcome::Scrolled`] when something moved, else
    /// [`InputOutcome::None`] (the host may then let its own UI scroll).
    Wheel {
        pos: (f32, f32),
        delta: (f32, f32),
        unit: WheelUnit,
    },
}

/// Unit of [`InputEvent::Wheel`]'s `delta` (egui `MouseWheelUnit`).
///
/// Maps 1:1 from egui `MouseWheelUnit` (UI, 2026-10-21):
/// - `Pixel` (egui `Point`): `delta` is in CSS px (1 CSS px per point).
/// - `Line`: one line = [`SCROLL_LINE_ROWS`] rows of [`SCROLL_ROW_PX`]
///   (60 px; winit reports one line per wheel notch, the Windows feel).
/// - `Page`: one page = the container's visible (client) height minus one
///   [`SCROLL_ROW_PX`] row, the same step as PageDown and a track click.
///
/// `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WheelUnit {
    Pixel,
    Line,
    Page,
}

/// One scroll "row" in CSS px (issue #26): a scrollbar arrow click, and the
/// unit a wheel [`WheelUnit::Line`] is made of.
pub const SCROLL_ROW_PX: f32 = 20.0;
/// Rows per wheel line (browsers scroll about three lines per notch).
pub const SCROLL_LINE_ROWS: f32 = 3.0;
/// Width of the engine-drawn Win95 scrollbar in CSS px (HTML_CONTRACT §4).
pub const SCROLLBAR_WIDTH_PX: f32 = 16.0;

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
    /// Commit-only form control change (range release, toggle, select pick,
    /// arrow/spin step, typed Enter/blur).
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
///
/// `#[non_exhaustive]` so adding variants (e.g. `ClipboardWrite`) does not
/// break exhaustive matches in Mainnet / other downstream crates.
#[non_exhaustive]
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
    /// Host should place `text` on the system clipboard (Copy/Cut). The engine
    /// never touches the OS clipboard itself.
    ClipboardWrite {
        text: String,
    },
    /// A scroll container moved (issue #26): wheel, scrollbar arrow / track /
    /// thumb, or PageUp/PageDown/Home/End. `id` is the container's element id
    /// (`None` when it has none). Never fires a host action.
    Scrolled {
        id: Option<String>,
    },
}
