//! Opt-in background paint worker (issue #8 step 3).
//!
//! # Host integration (Mainnet / wgpu / egui)
//!
//! Blitz documents are `!Send`, so the worker thread creates and owns the
//! [`Engine`] and every surface. The host keeps a [`PaintWorker`] handle that
//! is `Send` and never blocks waiting for paint after spawn.
//!
//! Typical Mainnet frame:
//!
//! 1. After the sim tick, build a [`SyncPacket`] from already-rendered slice
//!    HTML (the worker must never call into the sim). Enqueue with
//!    [`PaintWorker::sync`] / [`PaintWorker::sync_surface`] (fire-and-forget).
//! 2. Enqueue input and surface controls the same way. Discrete input and
//!    `complete` are never dropped; slice syncs, pointer moves, and per-surface
//!    controls coalesce. The structural bound is O(slices + surfaces) plus
//!    human-rate discrete input ([`PAINT_WORKER_DISCRETE_SOFT_CAP`]).
//! 3. Call [`PaintWorker::latest_frame`]`(id, since_generation)`. That returns
//!    an [`FrameView`] whose `output` is an `Arc` (cloning the view is cheap;
//!    pixels are not copied) and whose `damage` is the union since
//!    `since_generation` (or a full-surface rect when history is insufficient).
//!    Published frames also carry forward damage from any earlier published
//!    frame the host never took, so a host that ignores generations stays safe.
//! 4. Drain [`PaintWorker::poll_outcomes`] (each carries [`SurfaceId`]) and call
//!    [`PaintWorker::complete`] when an action finishes.
//! 5. On shutdown, drop the handle. The worker joins; a panic or stall is
//!    reported via [`PaintWorker::poll_error`].
//!
//! Keep using the synchronous [`Engine`] API when you do not need a worker.

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::damage::{merge_damage_rects, IntRect};
use crate::engine::{DirtySet, Engine};
use crate::error::EngineError;
use crate::types::*;

/// Soft cap on the order-preserving discrete-input queue. Exceeding it warns
/// and increments [`PaintWorker::discrete_over_cap_count`]; events are never
/// dropped. The real bound is O(slices + surfaces) for coalesced state plus
/// human-rate discrete input.
pub const PAINT_WORKER_DISCRETE_SOFT_CAP: usize = 256;

/// Hard cap on the discrete queue. Past this, host calls return
/// [`WorkerError::Stalled`] (events already queued are kept; new discrete
/// events are still enqueued so input is never dropped, but the host must
/// fall back). Also triggered by [`WORKER_STALL_TIMEOUT_NS`] with no progress.
pub const PAINT_WORKER_DISCRETE_HARD_CAP: usize = 1024;

/// Historical name kept for docs / benches. The mailbox no longer blocks at
/// this size; coalescing and the discrete soft-cap replace a hard wait.
pub const PAINT_WORKER_COMMAND_CAPACITY: usize = 64;

/// How long [`PaintWorker::Drop`] waits for the worker thread to join before
/// logging that shutdown is taking longer than expected.
pub const PAINT_WORKER_JOIN_WARN_NS: u64 = 2_000_000_000;

/// Number of RGBA / frame slots in the per-surface triple buffer.
pub const PAINT_WORKER_FRAME_SLOTS: usize = 3;

/// Generations of per-surface damage kept for [`PaintWorker::latest_frame`].
pub const DAMAGE_HISTORY_GENERATIONS: usize = 8;

/// Host-visible stall timeout. Any remaining wait (e.g. [`PaintWorker::flush`])
/// is bounded by this. A wedged worker yields [`WorkerError::Stalled`] so the
/// host can fall back to the sync [`Engine`].
pub const WORKER_STALL_TIMEOUT_NS: u64 = 100_000_000; // 100 ms

/// Error surfaced from the paint worker to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerError {
    /// The worker thread panicked. The last finished frames remain available.
    Panic(String),
    /// The worker has shut down (or never started).
    ShutDown,
    /// The worker made no progress within [`WORKER_STALL_TIMEOUT_NS`].
    /// Queued input is still held; the host may fall back to a sync [`Engine`].
    Stalled,
    /// An engine command failed on the worker.
    Engine {
        surface: Option<SurfaceId>,
        error: EngineError,
    },
    /// Internal mailbox / protocol failure.
    Internal(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::Panic(msg) => write!(f, "paint worker panicked: {msg}"),
            WorkerError::ShutDown => write!(f, "paint worker is shut down"),
            WorkerError::Stalled => write!(
                f,
                "paint worker stalled (no progress within {WORKER_STALL_TIMEOUT_NS} ns)"
            ),
            WorkerError::Engine { surface, error } => match surface {
                Some(id) => write!(f, "paint worker engine error on {id}: {error}"),
                None => write!(f, "paint worker engine error: {error}"),
            },
            WorkerError::Internal(msg) => write!(f, "paint worker internal error: {msg}"),
        }
    }
}

impl std::error::Error for WorkerError {}

fn engine_err(surface: Option<SurfaceId>, error: EngineError) -> WorkerError {
    WorkerError::Engine { surface, error }
}

/// Owned slice + fragment snapshot the host sends to the worker.
#[derive(Clone, Debug, Default)]
pub struct SyncPacket {
    pub versions: HashMap<String, u64>,
    pub actions: Vec<(String, ActionState)>,
    pub fragments: HashMap<String, String>,
}

impl SyncPacket {
    /// Merge `newer` per slice / section (latest key wins). Used when coalescing.
    pub fn merge_per_slice(&mut self, newer: SyncPacket) {
        let replace_actions = newer.versions.contains_key("actions") || !newer.actions.is_empty();
        for (k, v) in newer.versions {
            self.versions.insert(k, v);
        }
        for (k, v) in newer.fragments {
            self.fragments.insert(k, v);
        }
        if replace_actions {
            self.actions = newer.actions;
        }
    }
}

impl SliceSource for SyncPacket {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }

    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
}

impl FragmentSource for SyncPacket {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        self.fragments
            .get(section_id)
            .cloned()
            .unwrap_or_else(|| format!("<!-- {section_id} -->"))
    }
}

/// One finished surface frame held in the triple buffer.
///
/// Cloning this value is cheap: [`Self::output`] is an `Arc`, so only the `Arc`
/// and small metadata are copied — not the RGBA bytes.
#[derive(Clone, Debug)]
pub struct WorkerFrame {
    pub surface: SurfaceId,
    pub output: Arc<PaintOutput>,
    /// Damage for GPU upload: this paint ∪ any earlier published frame that was
    /// never taken by the host (may be a single full-surface rect).
    pub publish_damage: Vec<Rect>,
    pub stats: PaintStats,
    pub generation: u64,
    pub paint_ns: u64,
    pub width: u32,
    pub height: u32,
    pub full_upload: bool,
}

/// Host-facing view from [`PaintWorker::latest_frame`].
///
/// Cloning is cheap because [`Self::output`] is an `Arc` (no RGBA copy). Use
/// [`Self::damage`] for `write_texture`, not `output.damage` (that is only the
/// last paint's rects).
#[derive(Clone, Debug)]
pub struct FrameView {
    pub surface: SurfaceId,
    pub output: Arc<PaintOutput>,
    pub damage: Vec<Rect>,
    pub generation: u64,
    pub stats: PaintStats,
    pub paint_ns: u64,
    pub full_upload: bool,
}

/// Dispatched input outcome tagged with its surface.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerOutcome {
    pub surface: SurfaceId,
    pub outcome: InputOutcome,
}

#[derive(Clone, Debug, Default)]
pub struct WorkerStatus {
    pub dirty_surfaces: Vec<SurfaceId>,
    pub timings: Timings,
    pub paint_stats: PaintStats,
    pub last_dirty: DirtySet,
    pub has_pending: bool,
    pub frames_painted: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum SyncTarget {
    All,
    Surface(SurfaceId),
}

struct Reply<T>(SyncSender<T>);

impl<T> std::fmt::Debug for Reply<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reply(..)")
    }
}

impl<T> Reply<T> {
    fn send(self, value: T) {
        let _ = self.0.send(value);
    }
}

#[derive(Debug)]
enum Command {
    DeclareActions {
        names: Vec<String>,
    },
    MarkTheme,
    OpenSurface {
        id: SurfaceId,
        html: String,
        viewport: Viewport,
        opts: SurfaceOpts,
    },
    CloseSurface {
        id: SurfaceId,
    },
    SetVisible {
        id: SurfaceId,
        visible: bool,
    },
    ResizeSurface {
        id: SurfaceId,
        viewport: Viewport,
    },
    LoadHtml {
        html: String,
    },
    Sync {
        target: SyncTarget,
        packet: SyncPacket,
    },
    Input {
        surface: SurfaceId,
        event: InputEvent,
    },
    Complete {
        request: RequestId,
        result: Result<(), Reason>,
    },
    Flush {
        reply: Reply<Result<(), WorkerError>>,
    },
    TestPanic,
    /// Test-only: park until `release` is true (stall detection).
    TestHold {
        release: Arc<AtomicBool>,
    },
    Shutdown,
}

fn is_discrete_input(ev: &InputEvent) -> bool {
    !matches!(ev, InputEvent::PointerMove { .. })
}

fn is_discrete_cmd(cmd: &Command) -> bool {
    match cmd {
        Command::Input { event, .. } => is_discrete_input(event),
        // Close / open end a surface "run": control coalescing must not cross them.
        Command::Complete { .. }
        | Command::Flush { .. }
        | Command::DeclareActions { .. }
        | Command::OpenSurface { .. }
        | Command::CloseSurface { .. }
        | Command::LoadHtml { .. }
        | Command::TestPanic
        | Command::TestHold { .. }
        | Command::Shutdown => true,
        _ => false,
    }
}

fn union_damage(rects: Vec<Rect>, width: u32, height: u32) -> (Vec<Rect>, bool) {
    let int_rects: Vec<IntRect> = rects
        .into_iter()
        .map(|r| IntRect {
            x: r.x.floor() as i32,
            y: r.y.floor() as i32,
            w: r.width.ceil() as i32,
            h: r.height.ceil() as i32,
        })
        .map(|r| r.clamp_to_surface(width, height))
        .collect();
    let plan = merge_damage_rects(int_rects, width, height);
    let full_upload = plan.full_repaint
        || plan.full_repaint_area_fallback
        || (plan.merged_to_bbox && plan.rects.len() == 1);
    (
        plan.rects.iter().map(|r| r.to_rect()).collect(),
        full_upload,
    )
}

/// Non-blocking mailbox: coalesces syncs / moves / controls; never drops
/// discrete input or `complete`. Never waits on `not_full`.
struct Mailbox {
    inner: Mutex<MailboxInner>,
    not_empty: Condvar,
    discrete_over_cap: AtomicU64,
}

struct MailboxInner {
    queue: VecDeque<Command>,
    closed: bool,
    /// Primary/secondary/middle buttons currently down per surface. While > 0,
    /// PointerMove is not coalesced (drag keeps every move).
    buttons_down: HashMap<SurfaceId, u8>,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MailboxInner {
                queue: VecDeque::new(),
                closed: false,
                buttons_down: HashMap::new(),
            }),
            not_empty: Condvar::new(),
            discrete_over_cap: AtomicU64::new(0),
        }
    }

    fn close(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        self.not_empty.notify_all();
    }

    fn push_front_shutdown(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        g.queue.push_front(Command::Shutdown);
        self.not_empty.notify_all();
    }

    fn push_sync(&self, target: SyncTarget, packet: SyncPacket) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return Err(WorkerError::ShutDown);
        }
        let mut replace_at = None;
        for (idx, cmd) in g.queue.iter().enumerate().rev() {
            if is_discrete_cmd(cmd) {
                break;
            }
            if matches!(cmd, Command::Sync { target: t, .. } if *t == target) {
                replace_at = Some(idx);
                break;
            }
        }
        if let Some(idx) = replace_at {
            if let Some(Command::Sync { packet: slot, .. }) = g.queue.get_mut(idx) {
                slot.merge_per_slice(packet);
            }
        } else {
            g.queue.push_back(Command::Sync { target, packet });
            self.not_empty.notify_one();
        }
        Ok(())
    }

    fn push_pointer_move(&self, surface: SurfaceId, event: InputEvent) -> Result<(), WorkerError> {
        debug_assert!(matches!(event, InputEvent::PointerMove { .. }));
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return Err(WorkerError::ShutDown);
        }
        // While a button is held, keep every move (drag). Surviving coalesced
        // moves keep their own modifiers from the latest event.
        if g.buttons_down.get(&surface).copied().unwrap_or(0) > 0 {
            drop(g);
            return self.push_discrete(Command::Input { surface, event });
        }
        let mut replace_at = None;
        for (idx, cmd) in g.queue.iter().enumerate().rev() {
            if is_discrete_cmd(cmd) {
                break;
            }
            if matches!(
                cmd,
                Command::Input {
                    surface: s,
                    event: InputEvent::PointerMove { .. }
                } if *s == surface
            ) {
                replace_at = Some(idx);
                break;
            }
        }
        if let Some(idx) = replace_at {
            if let Some(Command::Input { event: slot, .. }) = g.queue.get_mut(idx) {
                *slot = event;
            }
        } else {
            g.queue.push_back(Command::Input { surface, event });
            self.not_empty.notify_one();
        }
        Ok(())
    }

    fn push_set_visible(&self, id: SurfaceId, visible: bool) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return Err(WorkerError::ShutDown);
        }
        let mut replace_at = None;
        for (idx, cmd) in g.queue.iter().enumerate().rev() {
            if is_discrete_cmd(cmd) {
                break;
            }
            if matches!(cmd, Command::SetVisible { id: s, .. } if *s == id) {
                replace_at = Some(idx);
                break;
            }
        }
        if let Some(idx) = replace_at {
            if let Some(Command::SetVisible { visible: slot, .. }) = g.queue.get_mut(idx) {
                *slot = visible;
            }
        } else {
            g.queue.push_back(Command::SetVisible { id, visible });
            self.not_empty.notify_one();
        }
        Ok(())
    }

    fn push_resize(&self, id: SurfaceId, viewport: Viewport) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return Err(WorkerError::ShutDown);
        }
        let mut replace_at = None;
        for (idx, cmd) in g.queue.iter().enumerate().rev() {
            if is_discrete_cmd(cmd) {
                break;
            }
            if matches!(cmd, Command::ResizeSurface { id: s, .. } if *s == id) {
                replace_at = Some(idx);
                break;
            }
        }
        if let Some(idx) = replace_at {
            if let Some(Command::ResizeSurface { viewport: slot, .. }) = g.queue.get_mut(idx) {
                *slot = viewport;
            }
        } else {
            g.queue.push_back(Command::ResizeSurface { id, viewport });
            self.not_empty.notify_one();
        }
        Ok(())
    }

    fn push_close(&self, id: SurfaceId) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return Err(WorkerError::ShutDown);
        }
        // Drop coalescable controls / syncs for this surface since the last barrier.
        let mut keep = VecDeque::new();
        let mut stripping = true;
        while let Some(cmd) = g.queue.pop_back() {
            if stripping && is_discrete_cmd(&cmd) {
                stripping = false;
                keep.push_front(cmd);
                continue;
            }
            if stripping {
                let drop_it = match &cmd {
                    Command::SetVisible { id: s, .. }
                    | Command::ResizeSurface { id: s, .. }
                    | Command::CloseSurface { id: s } => *s == id,
                    Command::Sync {
                        target: SyncTarget::Surface(s),
                        ..
                    } => *s == id,
                    Command::Input {
                        surface: s,
                        event: InputEvent::PointerMove { .. },
                    } => *s == id,
                    _ => false,
                };
                if drop_it {
                    continue;
                }
            }
            keep.push_front(cmd);
        }
        g.queue = keep;
        g.buttons_down.remove(&id);
        g.queue.push_back(Command::CloseSurface { id });
        self.not_empty.notify_one();
        Ok(())
    }

    fn push_mark_theme(&self) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return Err(WorkerError::ShutDown);
        }
        let mut replace_at = None;
        for (idx, cmd) in g.queue.iter().enumerate().rev() {
            if is_discrete_cmd(cmd) {
                break;
            }
            if matches!(cmd, Command::MarkTheme) {
                replace_at = Some(idx);
                break;
            }
        }
        if replace_at.is_none() {
            g.queue.push_back(Command::MarkTheme);
            self.not_empty.notify_one();
        }
        Ok(())
    }

    fn push_discrete(&self, cmd: Command) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            return Err(WorkerError::ShutDown);
        }
        // Track button press state for drag-aware move coalescing.
        if let Command::Input { surface, event } = &cmd {
            match event {
                InputEvent::PointerDown { .. } => {
                    let n = g.buttons_down.entry(*surface).or_insert(0);
                    *n = n.saturating_add(1);
                }
                InputEvent::PointerUp { .. } => {
                    if let Some(n) = g.buttons_down.get_mut(surface) {
                        *n = n.saturating_sub(1);
                        if *n == 0 {
                            g.buttons_down.remove(surface);
                        }
                    }
                }
                InputEvent::PointerLeave => {
                    g.buttons_down.remove(surface);
                }
                _ => {}
            }
        }
        if let Command::OpenSurface { id, .. } = &cmd {
            g.buttons_down.remove(id);
        }

        let discrete_len = g.queue.iter().filter(|c| is_discrete_cmd(c)).count();
        if discrete_len >= PAINT_WORKER_DISCRETE_SOFT_CAP {
            let n = self.discrete_over_cap.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n.is_multiple_of(64) {
                eprintln!(
                    "hx_native: discrete command queue above soft cap \
                     {PAINT_WORKER_DISCRETE_SOFT_CAP} (count={n}); not dropping"
                );
            }
        }
        let hard = discrete_len >= PAINT_WORKER_DISCRETE_HARD_CAP;
        g.queue.push_back(cmd);
        self.not_empty.notify_one();
        if hard {
            Err(WorkerError::Stalled)
        } else {
            Ok(())
        }
    }

    fn recv_batch(&self) -> Option<Vec<Command>> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if !g.queue.is_empty() {
                let mut out = Vec::with_capacity(g.queue.len());
                while let Some(cmd) = g.queue.pop_front() {
                    out.push(cmd);
                }
                return Some(out);
            }
            if g.closed {
                return None;
            }
            g = self.not_empty.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[derive(Clone, Debug)]
struct UntakenDamage {
    rects: Vec<Rect>,
    width: u32,
    height: u32,
    full: bool,
}

/// Triple buffer of finished frames for one surface (safe Rust, no `unsafe`).
///
/// Soundness comes from **each slot having its own `Mutex`**: a producer write
/// and a consumer read never share a slot mutex, so index swaps under
/// `indices` only decide *which* slot is published — they do not protect pixel
/// data by themselves.
struct TripleFrameBuffer {
    slots: [Mutex<Option<WorkerFrame>>; PAINT_WORKER_FRAME_SLOTS],
    indices: Mutex<(usize, Option<usize>)>,
    /// Damage of the last published frame if the host has not taken it yet.
    untaken_damage: Mutex<Option<UntakenDamage>>,
    history: Mutex<VecDeque<HistEntry>>,
}

#[derive(Clone, Debug)]
struct HistEntry {
    generation: u64,
    damage: Vec<Rect>,
    width: u32,
    height: u32,
}

impl TripleFrameBuffer {
    fn new() -> Self {
        Self {
            slots: [Mutex::new(None), Mutex::new(None), Mutex::new(None)],
            indices: Mutex::new((0, None)),
            untaken_damage: Mutex::new(None),
            history: Mutex::new(VecDeque::new()),
        }
    }

    fn publish(&self, mut frame: WorkerFrame) {
        let paint_damage = frame.output.damage.clone();
        let w = frame.width;
        let h = frame.height;

        // Carry forward damage from a published frame the host never took.
        {
            let mut untaken = self
                .untaken_damage
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(prev) = untaken.take() {
                if prev.full || prev.width != w || prev.height != h {
                    frame.publish_damage = vec![IntRect::full_surface(w, h).to_rect()];
                    frame.full_upload = true;
                } else {
                    let mut all = prev.rects;
                    all.extend(paint_damage.iter().copied());
                    let (merged, full) = union_damage(all, w, h);
                    frame.publish_damage = merged;
                    frame.full_upload = full;
                }
            } else {
                frame.publish_damage = paint_damage.clone();
                frame.full_upload = false;
            }
            *untaken = Some(UntakenDamage {
                rects: frame.publish_damage.clone(),
                width: w,
                height: h,
                full: frame.full_upload,
            });
        }

        {
            let mut hist = self.history.lock().unwrap_or_else(|e| e.into_inner());
            hist.push_back(HistEntry {
                generation: frame.generation,
                damage: paint_damage,
                width: w,
                height: h,
            });
            while hist.len() > DAMAGE_HISTORY_GENERATIONS {
                hist.pop_front();
            }
        }

        let mut idx = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        let write = idx.0;
        {
            let mut slot = self.slots[write].lock().unwrap_or_else(|e| e.into_inner());
            *slot = Some(frame);
        }
        let published = write;
        let prev_published = idx.1;
        let next = (0..PAINT_WORKER_FRAME_SLOTS)
            .find(|&i| i != published && Some(i) != prev_published)
            .unwrap_or_else(|| (published + 1) % PAINT_WORKER_FRAME_SLOTS);
        idx.0 = next;
        idx.1 = Some(published);
    }

    /// Latest published frame. Cloning is cheap (`Arc` on pixels).
    fn latest(&self) -> Option<WorkerFrame> {
        let published = {
            let idx = self.indices.lock().unwrap_or_else(|e| e.into_inner());
            idx.1?
        };
        let slot = self.slots[published]
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        slot.clone()
    }

    fn mark_taken(&self) {
        let mut untaken = self
            .untaken_damage
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *untaken = None;
    }

    fn damage_since(&self, since_generation: u64, latest: &WorkerFrame) -> (Vec<Rect>, bool) {
        if since_generation >= latest.generation {
            return (Vec::new(), false);
        }
        let hist = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let needed_from = since_generation.saturating_add(1);
        let oldest = hist.front().map(|e| e.generation);
        if oldest.is_none_or(|o| o > needed_from) {
            return (
                vec![IntRect::full_surface(latest.width, latest.height).to_rect()],
                true,
            );
        }
        if latest.full_upload {
            return (
                vec![IntRect::full_surface(latest.width, latest.height).to_rect()],
                true,
            );
        }
        let mut rects = Vec::new();
        let mut seen = 0u64;
        for e in hist.iter() {
            if e.generation < needed_from || e.generation > latest.generation {
                continue;
            }
            if e.width != latest.width || e.height != latest.height {
                return (
                    vec![IntRect::full_surface(latest.width, latest.height).to_rect()],
                    true,
                );
            }
            seen += 1;
            rects.extend(e.damage.iter().copied());
        }
        let expected = latest.generation.saturating_sub(since_generation);
        if seen < expected {
            return (
                vec![IntRect::full_surface(latest.width, latest.height).to_rect()],
                true,
            );
        }
        union_damage(rects, latest.width, latest.height)
    }
}

struct SharedState {
    frames: Mutex<HashMap<SurfaceId, Arc<TripleFrameBuffer>>>,
    outcomes: Mutex<Vec<WorkerOutcome>>,
    status: Mutex<WorkerStatus>,
    error: Mutex<Option<WorkerError>>,
    input_seq: AtomicU64,
    frame_input_seq: AtomicU64,
    /// Nanoseconds (Instant::now relative) of last worker progress.
    last_progress_ns: AtomicU64,
    progress_epoch: Instant,
    busy: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        let epoch = Instant::now();
        Self {
            frames: Mutex::new(HashMap::new()),
            outcomes: Mutex::new(Vec::new()),
            status: Mutex::new(WorkerStatus::default()),
            error: Mutex::new(None),
            input_seq: AtomicU64::new(0),
            frame_input_seq: AtomicU64::new(0),
            last_progress_ns: AtomicU64::new(0),
            progress_epoch: epoch,
            busy: AtomicBool::new(false),
        }
    }

    fn bump_progress(&self) {
        let ns = self.progress_epoch.elapsed().as_nanos() as u64;
        self.last_progress_ns.store(ns, Ordering::SeqCst);
    }

    fn is_stalled(&self) -> bool {
        let last = self.last_progress_ns.load(Ordering::SeqCst);
        let now = self.progress_epoch.elapsed().as_nanos() as u64;
        now.saturating_sub(last) > WORKER_STALL_TIMEOUT_NS
    }

    fn buffer_for(&self, id: SurfaceId) -> Arc<TripleFrameBuffer> {
        let mut map = self.frames.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(id)
            .or_insert_with(|| Arc::new(TripleFrameBuffer::new()))
            .clone()
    }

    fn set_error(&self, err: WorkerError) {
        let mut g = self.error.lock().unwrap_or_else(|e| e.into_inner());
        // Prefer Panic / Stalled over quieter engine errors once set.
        match (&*g, &err) {
            (Some(WorkerError::Panic(_)), _) => {}
            (Some(WorkerError::Stalled), WorkerError::Engine { .. }) => {}
            _ => *g = Some(err),
        }
    }
}

/// Host-side handle for a background paint thread (`Send` + `Sync`).
pub struct PaintWorker {
    mailbox: Arc<Mailbox>,
    shared: Arc<SharedState>,
    join: Option<JoinHandle<()>>,
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    fn check() {
        assert_send_sync::<PaintWorker>();
        assert_send_sync::<SyncPacket>();
        assert_send_sync::<WorkerFrame>();
        assert_send_sync::<FrameView>();
        assert_send_sync::<WorkerOutcome>();
        assert_send_sync::<WorkerError>();
    }
    let _ = check;
};

impl PaintWorker {
    /// Spawn a worker thread that creates and owns the [`Engine`].
    pub fn spawn(config: EngineConfig) -> Self {
        let mailbox = Arc::new(Mailbox::new());
        let shared = Arc::new(SharedState::new());
        shared.bump_progress();
        let mb = Arc::clone(&mailbox);
        let sh = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("hx-native-paint".into())
            .spawn(move || worker_main(config, mb, sh))
            .expect("spawn hx-native paint worker");
        Self {
            mailbox,
            shared,
            join: Some(join),
        }
    }

    /// After a successful enqueue, surface a stall so the host can fall back.
    /// Input remains queued.
    fn check_stall_after_enqueue(
        &self,
        enqueued: Result<(), WorkerError>,
    ) -> Result<(), WorkerError> {
        if let Err(WorkerError::Stalled) = enqueued {
            self.shared.set_error(WorkerError::Stalled);
            return enqueued;
        }
        enqueued?;
        if let Some(err @ WorkerError::Panic(_)) = self.poll_error() {
            return Err(err);
        }
        if self.shared.is_stalled() {
            let err = WorkerError::Stalled;
            self.shared.set_error(err.clone());
            return Err(err);
        }
        Ok(())
    }

    /// Fire-and-forget. Engine failures appear in [`Self::poll_error`] with a
    /// [`SurfaceId`] when known.
    pub fn declare_actions(&self, names: &[&str]) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_discrete(Command::DeclareActions {
            names: names.iter().map(|s| (*s).to_string()).collect(),
        }))
    }

    pub fn mark_theme_or_stylesheet_changed(&self) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_mark_theme())
    }

    pub fn open_surface(
        &self,
        id: SurfaceId,
        html: &str,
        viewport: Viewport,
        opts: SurfaceOpts,
    ) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_discrete(Command::OpenSurface {
            id,
            html: html.to_string(),
            viewport,
            opts,
        }))
    }

    pub fn close_surface(&self, id: SurfaceId) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_close(id))
    }

    pub fn set_visible(&self, id: SurfaceId, visible: bool) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_set_visible(id, visible))
    }

    pub fn resize_surface(&self, id: SurfaceId, viewport: Viewport) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_resize(id, viewport))
    }

    pub fn load_html(&self, html: &str) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_discrete(Command::LoadHtml {
            html: html.to_string(),
        }))
    }

    pub fn sync(&self, packet: SyncPacket) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_sync(SyncTarget::All, packet))
    }

    pub fn sync_surface(&self, id: SurfaceId, packet: SyncPacket) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(self.mailbox.push_sync(SyncTarget::Surface(id), packet))
    }

    pub fn input(&self, event: InputEvent) -> Result<(), WorkerError> {
        self.input_surface(SurfaceId::DEFAULT, event)
    }

    pub fn input_surface(&self, id: SurfaceId, event: InputEvent) -> Result<(), WorkerError> {
        self.shared.input_seq.fetch_add(1, Ordering::SeqCst);
        let r = if matches!(event, InputEvent::PointerMove { .. }) {
            self.mailbox.push_pointer_move(id, event)
        } else {
            self.mailbox
                .push_discrete(Command::Input { surface: id, event })
        };
        self.check_stall_after_enqueue(r)
    }

    pub fn complete(
        &self,
        request: RequestId,
        result: Result<(), Reason>,
    ) -> Result<(), WorkerError> {
        self.check_stall_after_enqueue(
            self.mailbox
                .push_discrete(Command::Complete { request, result }),
        )
    }

    /// Wait until commands queued before this call are applied and dirty
    /// surfaces painted. Bound by [`WORKER_STALL_TIMEOUT_NS`].
    pub fn flush(&self) -> Result<(), WorkerError> {
        if let Some(err) = self.poll_error() {
            return Err(err);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox
            .push_discrete(Command::Flush { reply: Reply(tx) })?;
        match rx.recv_timeout(Duration::from_nanos(WORKER_STALL_TIMEOUT_NS)) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                let err = WorkerError::Stalled;
                self.shared.set_error(err.clone());
                Err(err)
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(err) = self.poll_error() {
                    Err(err)
                } else {
                    Err(WorkerError::ShutDown)
                }
            }
        }
    }

    /// Latest finished frame for `id`, with damage union since `since_generation`.
    ///
    /// Returns `None` if nothing has been published yet. Cloning the returned
    /// [`FrameView`] is cheap because [`FrameView::output`] is an `Arc` — the
    /// RGBA buffer is not copied. Marks the frame taken so the next publish
    /// does not carry this damage forward again.
    pub fn latest_frame(&self, id: SurfaceId, since_generation: u64) -> Option<FrameView> {
        let buf = {
            let map = self.shared.frames.lock().unwrap_or_else(|e| e.into_inner());
            map.get(&id)?.clone()
        };
        let latest = buf.latest()?;
        let (damage, full) = buf.damage_since(since_generation, &latest);
        buf.mark_taken();
        Some(FrameView {
            surface: latest.surface,
            output: latest.output,
            damage,
            generation: latest.generation,
            stats: latest.stats,
            paint_ns: latest.paint_ns,
            full_upload: full || latest.full_upload,
        })
    }

    pub fn poll_outcomes(&self) -> Vec<WorkerOutcome> {
        let mut g = self
            .shared
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *g)
    }

    pub fn status(&self) -> WorkerStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn poll_error(&self) -> Option<WorkerError> {
        self.shared
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn last_frame_input_seq(&self) -> u64 {
        self.shared.frame_input_seq.load(Ordering::SeqCst)
    }

    pub fn current_input_seq(&self) -> u64 {
        self.shared.input_seq.load(Ordering::SeqCst)
    }

    pub fn discrete_over_cap_count(&self) -> u64 {
        self.mailbox.discrete_over_cap.load(Ordering::Relaxed)
    }

    pub fn inject_panic_for_test(&self) -> Result<(), WorkerError> {
        self.mailbox.push_discrete(Command::TestPanic)
    }

    /// Test-only: park the worker until `release` is set true.
    pub fn inject_hold_for_test(&self, release: Arc<AtomicBool>) -> Result<(), WorkerError> {
        self.mailbox.push_discrete(Command::TestHold { release })
    }
}

impl Drop for PaintWorker {
    fn drop(&mut self) {
        self.mailbox.push_front_shutdown();
        if let Some(handle) = self.join.take() {
            let t0 = Instant::now();
            let _ = handle.join();
            let waited = t0.elapsed().as_nanos() as u64;
            if waited > PAINT_WORKER_JOIN_WARN_NS {
                eprintln!(
                    "hx_native: PaintWorker join took {waited} ns (warn above {PAINT_WORKER_JOIN_WARN_NS})"
                );
            }
        }
        self.mailbox.close();
    }
}

fn worker_main(config: EngineConfig, mailbox: Arc<Mailbox>, shared: Arc<SharedState>) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut eng = Engine::new(config);
        let mut generations: HashMap<SurfaceId, u64> = HashMap::new();
        shared.bump_progress();
        while let Some(batch) = mailbox.recv_batch() {
            shared.busy.store(true, Ordering::SeqCst);
            shared.bump_progress();
            let mut stop = false;
            let mut flush_replies: Vec<Reply<Result<(), WorkerError>>> = Vec::new();
            for cmd in batch {
                match process_command(&mut eng, cmd, &shared, &mut flush_replies) {
                    Ok(true) => {
                        stop = true;
                        break;
                    }
                    Ok(false) => shared.bump_progress(),
                    Err(err) => {
                        shared.set_error(err.clone());
                        for reply in flush_replies.drain(..) {
                            reply.send(Err(err.clone()));
                        }
                    }
                }
            }
            if stop {
                for reply in flush_replies.drain(..) {
                    reply.send(Ok(()));
                }
                shared.busy.store(false, Ordering::SeqCst);
                break;
            }
            let paint_result = paint_dirty(&mut eng, &shared, &mut generations);
            refresh_status(&eng, &shared);
            shared.bump_progress();
            match paint_result {
                Ok(()) => {
                    for reply in flush_replies.drain(..) {
                        reply.send(Ok(()));
                    }
                }
                Err(err) => {
                    shared.set_error(err.clone());
                    for reply in flush_replies.drain(..) {
                        reply.send(Err(err.clone()));
                    }
                }
            }
            shared.busy.store(false, Ordering::SeqCst);
        }
    }));
    if let Err(payload) = result {
        shared.set_error(WorkerError::Panic(panic_message(&payload)));
    }
    mailbox.close();
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".into()
    }
}

fn process_command(
    eng: &mut Engine,
    cmd: Command,
    shared: &SharedState,
    flush_replies: &mut Vec<Reply<Result<(), WorkerError>>>,
) -> Result<bool, WorkerError> {
    match cmd {
        Command::DeclareActions { names } => {
            let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            eng.declare_actions(&refs);
            Ok(false)
        }
        Command::MarkTheme => {
            eng.mark_theme_or_stylesheet_changed();
            Ok(false)
        }
        Command::OpenSurface {
            id,
            html,
            viewport,
            opts,
        } => {
            if let Err(e) = eng.open_surface(id, &html, viewport, opts) {
                shared.set_error(engine_err(Some(id), e));
            }
            Ok(false)
        }
        Command::CloseSurface { id } => {
            if let Err(e) = eng.close_surface(id) {
                shared.set_error(engine_err(Some(id), e));
            }
            Ok(false)
        }
        Command::SetVisible { id, visible } => {
            if let Err(e) = eng.set_visible(id, visible) {
                shared.set_error(engine_err(Some(id), e));
            }
            Ok(false)
        }
        Command::ResizeSurface { id, viewport } => {
            if let Err(e) = eng.resize_surface(id, viewport) {
                shared.set_error(engine_err(Some(id), e));
            }
            Ok(false)
        }
        Command::LoadHtml { html } => {
            if let Err(e) = eng.load_html(&html) {
                shared.set_error(engine_err(Some(SurfaceId::DEFAULT), e));
            }
            Ok(false)
        }
        Command::Sync { target, packet } => {
            let slices = packet.clone();
            let mut frags = packet;
            let dirty = match target {
                SyncTarget::All => eng
                    .sync(&slices, &mut frags)
                    .map_err(|e| engine_err(None, e))?,
                SyncTarget::Surface(id) => eng
                    .sync_surface(id, &slices, &mut frags)
                    .map_err(|e| engine_err(Some(id), e))?,
            };
            let mut st = shared.status.lock().unwrap_or_else(|e| e.into_inner());
            st.last_dirty = dirty;
            Ok(false)
        }
        Command::Input { surface, event } => {
            let outcome = eng.input_surface(surface, event);
            push_outcome(shared, surface, outcome);
            for o in eng.take_surface_outcomes(surface) {
                push_outcome(shared, surface, o);
            }
            Ok(false)
        }
        Command::Complete { request, result } => {
            eng.complete(request, result);
            for id in eng.surface_ids() {
                for o in eng.take_surface_outcomes(id) {
                    push_outcome(shared, id, o);
                }
            }
            Ok(false)
        }
        Command::Flush { reply } => {
            flush_replies.push(reply);
            Ok(false)
        }
        Command::TestPanic => {
            panic!("hx_native paint worker test panic");
        }
        Command::TestHold { release } => {
            while !release.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
            Ok(false)
        }
        Command::Shutdown => Ok(true),
    }
}

fn push_outcome(shared: &SharedState, surface: SurfaceId, outcome: InputOutcome) {
    if matches!(outcome, InputOutcome::None) {
        return;
    }
    shared
        .outcomes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(WorkerOutcome { surface, outcome });
}

fn paint_dirty(
    eng: &mut Engine,
    shared: &SharedState,
    generations: &mut HashMap<SurfaceId, u64>,
) -> Result<(), WorkerError> {
    let input_seq = shared.input_seq.load(Ordering::SeqCst);
    let dirty = eng.dirty_surfaces();
    for id in dirty {
        let t0 = Instant::now();
        let output = eng.paint_surface(id).map_err(|e| engine_err(Some(id), e))?;
        let paint_ns = t0.elapsed().as_nanos() as u64;
        let stats = eng.last_paint_stats();
        if output.skipped && output.rgba.is_none() && output.damage.is_empty() {
            continue;
        }
        let (width, height) = match &output.rgba {
            Some(img) => (img.width, img.height),
            None => continue,
        };
        let gen = {
            let entry = generations.entry(id).or_insert(0);
            *entry = entry.saturating_add(1);
            *entry
        };
        let frame = WorkerFrame {
            surface: id,
            output: Arc::new(output),
            publish_damage: Vec::new(),
            stats: stats.clone(),
            generation: gen,
            paint_ns,
            width,
            height,
            full_upload: false,
        };
        shared.buffer_for(id).publish(frame);
        shared.frame_input_seq.store(input_seq, Ordering::SeqCst);
        let mut st = shared.status.lock().unwrap_or_else(|e| e.into_inner());
        st.frames_painted = st.frames_painted.saturating_add(1);
        st.paint_stats = stats;
        st.timings = eng.last_timings();
        shared.bump_progress();
    }
    for id in eng.surface_ids() {
        for o in eng.take_surface_outcomes(id) {
            push_outcome(shared, id, o);
        }
    }
    Ok(())
}

fn refresh_status(eng: &Engine, shared: &SharedState) {
    let mut st = shared.status.lock().unwrap_or_else(|e| e.into_inner());
    st.dirty_surfaces = eng.dirty_surfaces();
    st.timings = eng.last_timings();
    st.paint_stats = eng.last_paint_stats();
    st.has_pending = eng.has_pending();
}

#[cfg(test)]
mod mailbox_and_buffer_tests {
    use super::*;

    fn pkt(tag: &str) -> SyncPacket {
        let mut fragments = HashMap::new();
        fragments.insert("sec".into(), tag.into());
        SyncPacket {
            versions: HashMap::from([("ticker".into(), 1)]),
            actions: Vec::new(),
            fragments,
        }
    }

    fn make_frame(gen: u64, damage: Vec<Rect>) -> WorkerFrame {
        WorkerFrame {
            surface: SurfaceId::DEFAULT,
            output: Arc::new(PaintOutput {
                rgba: Some(RgbaImage {
                    width: 4,
                    height: 4,
                    pixels: vec![gen as u8; 4 * 4 * 4],
                }),
                texture: None,
                focus: FocusState::default(),
                skipped: false,
                damage: damage.clone(),
            }),
            publish_damage: damage,
            stats: PaintStats::default(),
            generation: gen,
            paint_ns: 0,
            width: 4,
            height: 4,
            full_upload: false,
        }
    }

    #[test]
    fn triple_buffer_untaken_damage_merges_on_publish() {
        let buf = TripleFrameBuffer::new();
        buf.publish(make_frame(
            1,
            vec![Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }],
        ));
        // No take — publish gen 2 with different damage.
        buf.publish(make_frame(
            2,
            vec![Rect {
                x: 3.0,
                y: 3.0,
                width: 1.0,
                height: 1.0,
            }],
        ));
        let frame = buf.latest().unwrap();
        assert_eq!(frame.generation, 2);
        assert!(
            frame.publish_damage.len() >= 2 || frame.full_upload,
            "untaken gen1 damage must merge into gen2: {:?}",
            frame.publish_damage
        );
    }

    #[test]
    fn mailbox_coalesces_syncs_and_moves_across_clicks() {
        let mb = Mailbox::new();
        mb.push_sync(SyncTarget::All, pkt("a")).unwrap();
        mb.push_sync(SyncTarget::All, pkt("b")).unwrap();
        mb.push_pointer_move(
            SurfaceId::DEFAULT,
            InputEvent::PointerMove {
                x: 1.0,
                y: 1.0,
                mods: Modifiers::default(),
            },
        )
        .unwrap();
        mb.push_pointer_move(
            SurfaceId::DEFAULT,
            InputEvent::PointerMove {
                x: 9.0,
                y: 9.0,
                mods: Modifiers::default(),
            },
        )
        .unwrap();
        mb.push_discrete(Command::Input {
            surface: SurfaceId::DEFAULT,
            event: InputEvent::PointerDown {
                x: 9.0,
                y: 9.0,
                button: PointerButton::Primary,
                mods: Modifiers::default(),
            },
        })
        .unwrap();
        mb.push_pointer_move(
            SurfaceId::DEFAULT,
            InputEvent::PointerMove {
                x: 2.0,
                y: 2.0,
                mods: Modifiers::default(),
            },
        )
        .unwrap();
        let batch = mb.recv_batch().unwrap();
        assert_eq!(batch.len(), 4);
        match &batch[0] {
            Command::Sync { packet, .. } => {
                assert_eq!(packet.fragments.get("sec").map(String::as_str), Some("b"));
            }
            other => panic!("{other:?}"),
        }
        match &batch[1] {
            Command::Input {
                event: InputEvent::PointerMove { x, y, .. },
                ..
            } => {
                assert_eq!((*x, *y), (9.0, 9.0));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            batch[2],
            Command::Input {
                event: InputEvent::PointerDown { .. },
                ..
            }
        ));
        match &batch[3] {
            Command::Input {
                event: InputEvent::PointerMove { x, y, .. },
                ..
            } => {
                assert_eq!((*x, *y), (2.0, 2.0));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn mailbox_saturated_burst_never_blocks() {
        let mb = Mailbox::new();
        let t0 = Instant::now();
        for i in 0..10_000 {
            mb.push_sync(SyncTarget::All, pkt(&format!("v{i}")))
                .unwrap();
            mb.push_pointer_move(
                SurfaceId::DEFAULT,
                InputEvent::PointerMove {
                    x: i as f32,
                    y: i as f32,
                    mods: Modifiers::default(),
                },
            )
            .unwrap();
            mb.push_set_visible(SurfaceId(1), i % 2 == 0).unwrap();
        }
        assert!(
            t0.elapsed() < Duration::from_millis(200),
            "saturated coalescing burst blocked for {:?}",
            t0.elapsed()
        );
        let batch = mb.recv_batch().unwrap();
        assert!(
            batch.len() <= 4,
            "expected heavy coalesce, got {}",
            batch.len()
        );
    }

    #[test]
    fn mailbox_drag_keeps_every_move_and_surviving_mods() {
        let mb = Mailbox::new();
        let id = SurfaceId::DEFAULT;
        mb.push_discrete(Command::Input {
            surface: id,
            event: InputEvent::PointerDown {
                x: 0.0,
                y: 0.0,
                button: PointerButton::Primary,
                mods: Modifiers::default(),
            },
        })
        .unwrap();
        for i in 0..8 {
            mb.push_pointer_move(
                id,
                InputEvent::PointerMove {
                    x: i as f32,
                    y: 1.0,
                    mods: Modifiers {
                        shift: i == 7,
                        ..Modifiers::default()
                    },
                },
            )
            .unwrap();
        }
        mb.push_discrete(Command::Input {
            surface: id,
            event: InputEvent::PointerUp {
                x: 7.0,
                y: 1.0,
                button: PointerButton::Primary,
                mods: Modifiers::default(),
            },
        })
        .unwrap();
        // After release, moves coalesce again; surviving move keeps its mods.
        mb.push_pointer_move(
            id,
            InputEvent::PointerMove {
                x: 1.0,
                y: 2.0,
                mods: Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            },
        )
        .unwrap();
        mb.push_pointer_move(
            id,
            InputEvent::PointerMove {
                x: 9.0,
                y: 9.0,
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            },
        )
        .unwrap();
        let batch = mb.recv_batch().unwrap();
        let moves: Vec<_> = batch
            .iter()
            .filter_map(|c| match c {
                Command::Input {
                    event: InputEvent::PointerMove { x, y, mods },
                    ..
                } => Some((*x, *y, mods.shift, mods.alt, mods.ctrl)),
                _ => None,
            })
            .collect();
        assert_eq!(
            moves.len(),
            9,
            "8 drag moves + 1 coalesced post-up: {moves:?}"
        );
        for (i, m) in moves.iter().take(8).enumerate() {
            assert_eq!(m.0, i as f32);
            assert_eq!(m.2, i == 7, "drag move {i} keeps its own mods");
        }
        assert_eq!(moves[8], (9.0, 9.0, false, false, true));
    }

    #[test]
    fn mailbox_close_open_barrier_blocks_set_visible_merge() {
        let mb = Mailbox::new();
        let id = SurfaceId(7);
        mb.push_set_visible(id, false).unwrap();
        mb.push_close(id).unwrap();
        mb.push_discrete(Command::OpenSurface {
            id,
            html: "<html></html>".into(),
            viewport: Viewport::new(10, 10),
            opts: SurfaceOpts {
                visible: false,
                pinned: false,
            },
        })
        .unwrap();
        mb.push_set_visible(id, true).unwrap();
        let batch = mb.recv_batch().unwrap();
        let visibles: Vec<_> = batch
            .iter()
            .filter_map(|c| match c {
                Command::SetVisible { id: s, visible } if *s == id => Some(*visible),
                _ => None,
            })
            .collect();
        assert!(
            batch
                .iter()
                .any(|c| matches!(c, Command::CloseSurface { id: s } if *s == id)),
            "close must remain: {batch:?}"
        );
        assert!(
            batch
                .iter()
                .any(|c| matches!(c, Command::OpenSurface { id: s, .. } if *s == id)),
            "open must remain: {batch:?}"
        );
        assert_eq!(
            visibles,
            vec![true],
            "pre-close set_visible must not survive; post-open true must: {batch:?}"
        );
    }

    #[test]
    fn mailbox_hard_cap_returns_stalled() {
        let mb = Mailbox::new();
        let leave = InputEvent::PointerLeave;
        for _ in 0..PAINT_WORKER_DISCRETE_HARD_CAP {
            mb.push_discrete(Command::Input {
                surface: SurfaceId::DEFAULT,
                event: leave.clone(),
            })
            .unwrap();
        }
        let err = mb
            .push_discrete(Command::Input {
                surface: SurfaceId::DEFAULT,
                event: leave,
            })
            .expect_err("hard cap must stall");
        assert!(matches!(err, WorkerError::Stalled), "{err:?}");
        assert!(mb.discrete_over_cap.load(Ordering::Relaxed) > 0);
    }
}
