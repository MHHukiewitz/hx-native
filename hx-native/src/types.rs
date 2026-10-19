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
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_paint_ns: 8_000_000, // ~8ms default headroom inside a 16.6ms frame
            pending_timeout_frames: 120,
        }
    }
}

/// Deterministic: starts at 1 per engine, +1 per emitted Action (incl. queued
/// releases), never reused, never persisted.
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
    /// True when raster was skipped (clean frame). Then `rgba` is `None`.
    pub skipped: bool,
}

#[derive(Clone, Debug)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub holes: Vec<Hole>,
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
