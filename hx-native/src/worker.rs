//! Opt-in background paint worker (issue #8 step 3).
//!
//! # Host integration (Mainnet / wgpu / egui)
//!
//! Blitz documents are `!Send`, so the worker thread creates and owns the
//! [`Engine`] and every surface. The host keeps a [`PaintWorker`] handle that
//! is `Send` and never blocks waiting for paint.
//!
//! Typical Mainnet frame:
//!
//! 1. After the sim tick, build a [`SyncPacket`] from already-rendered slice
//!    HTML (the worker must never call into the sim). Enqueue it with
//!    [`PaintWorker::sync`] / [`PaintWorker::sync_surface`].
//! 2. Enqueue input with [`PaintWorker::input`] / [`PaintWorker::input_surface`].
//!    Input is never dropped; slice syncs for the same target may coalesce.
//! 3. Call [`PaintWorker::take_frame`] for each surface you composite. That
//!    returns the last finished [`PaintOutput`] (shared via `Arc`) plus damage.
//!    Upload only the damage rects with `queue.write_texture`. If there is no
//!    newer frame, keep the previous GPU texture.
//! 4. Drain [`PaintWorker::poll_outcomes`] and route `Action` / `Blocked` to the
//!    game between ticks. Call [`PaintWorker::complete`] when an action finishes.
//! 5. On shutdown, drop the handle. The worker joins; a panic is reported via
//!    [`PaintWorker::poll_error`] and the last finished frames stay available.
//!
//! Keep using the synchronous [`Engine`] API when you do not need a worker. The
//! worker is a wrapper; it does not remove or change that API.

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::engine::{DirtySet, Engine};
use crate::error::EngineError;
use crate::types::*;

/// Bound on the worker command mailbox (queued commands).
pub const PAINT_WORKER_COMMAND_CAPACITY: usize = 64;

/// How long [`PaintWorker::Drop`] waits for the worker thread to join before
/// logging that shutdown is taking longer than expected.
pub const PAINT_WORKER_JOIN_WARN_NS: u64 = 2_000_000_000;

/// Number of RGBA / frame slots in the per-surface triple buffer.
pub const PAINT_WORKER_FRAME_SLOTS: usize = 3;

/// Error surfaced from the paint worker to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerError {
    /// The worker thread panicked. The last finished frames remain available.
    Panic(String),
    /// The worker has shut down (or never started).
    ShutDown,
    /// An engine command failed on the worker.
    Engine(EngineError),
    /// Internal mailbox / protocol failure.
    Internal(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::Panic(msg) => write!(f, "paint worker panicked: {msg}"),
            WorkerError::ShutDown => write!(f, "paint worker is shut down"),
            WorkerError::Engine(e) => write!(f, "paint worker engine error: {e}"),
            WorkerError::Internal(msg) => write!(f, "paint worker internal error: {msg}"),
        }
    }
}

impl std::error::Error for WorkerError {}

impl From<EngineError> for WorkerError {
    fn from(value: EngineError) -> Self {
        WorkerError::Engine(value)
    }
}

/// Owned slice + fragment snapshot the host sends to the worker.
///
/// The worker applies this through [`SliceSource`] / [`FragmentSource`] and
/// never holds a live reference into the sim. Build it on the host after the
/// sim tick has already rendered HTML for dirty sections.
#[derive(Clone, Debug, Default)]
pub struct SyncPacket {
    /// Slice name → version from the published end-of-tick snapshot.
    pub versions: HashMap<String, u64>,
    /// Reserved `actions` slice greying.
    pub actions: Vec<(String, ActionState)>,
    /// section id → HTML body for [`FragmentSource::render`].
    pub fragments: HashMap<String, String>,
}

impl SyncPacket {
    /// Replace `self` with `newer` (latest sync wins when coalesced).
    pub fn merge_latest(&mut self, newer: SyncPacket) {
        *self = newer;
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

/// One finished surface frame the host can composite.
#[derive(Clone, Debug)]
pub struct WorkerFrame {
    pub surface: SurfaceId,
    /// Shared so the host can hold the last frame without copying pixels.
    pub output: Arc<PaintOutput>,
    pub stats: PaintStats,
    /// Monotonic paint generation for this surface (starts at 1).
    pub generation: u64,
    /// Worker paint time for this frame (nanoseconds).
    pub paint_ns: u64,
}

/// Snapshot of host-visible worker state after a command batch.
#[derive(Clone, Debug, Default)]
pub struct WorkerStatus {
    pub dirty_surfaces: Vec<SurfaceId>,
    pub timings: Timings,
    pub paint_stats: PaintStats,
    pub last_dirty: DirtySet,
    /// True when at least one surface still has a pending action.
    pub has_pending: bool,
    /// Frames painted since the worker started.
    pub frames_painted: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum SyncTarget {
    All,
    Surface(SurfaceId),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum InputTarget {
    Default,
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
        reply: Reply<Result<(), WorkerError>>,
    },
    MarkTheme {
        reply: Reply<Result<(), WorkerError>>,
    },
    OpenSurface {
        id: SurfaceId,
        html: String,
        viewport: Viewport,
        opts: SurfaceOpts,
        reply: Reply<Result<(), WorkerError>>,
    },
    CloseSurface {
        id: SurfaceId,
        reply: Reply<Result<(), WorkerError>>,
    },
    SetVisible {
        id: SurfaceId,
        visible: bool,
        reply: Reply<Result<(), WorkerError>>,
    },
    ResizeSurface {
        id: SurfaceId,
        viewport: Viewport,
        reply: Reply<Result<(), WorkerError>>,
    },
    LoadHtml {
        html: String,
        reply: Reply<Result<(), WorkerError>>,
    },
    Sync {
        target: SyncTarget,
        packet: SyncPacket,
    },
    Input {
        target: InputTarget,
        event: InputEvent,
    },
    Complete {
        request: RequestId,
        result: Result<(), Reason>,
    },
    Flush {
        reply: Reply<Result<(), WorkerError>>,
    },
    /// Test-only: panic on the worker after the mailbox accepts the command.
    TestPanic,
    Shutdown,
}

fn sync_target_of(cmd: &Command) -> Option<SyncTarget> {
    match cmd {
        Command::Sync { target, .. } => Some(*target),
        _ => None,
    }
}

fn is_input(cmd: &Command) -> bool {
    matches!(cmd, Command::Input { .. })
}

/// Bounded mailbox: input is never dropped; same-target syncs coalesce when no
/// input sits between them.
struct Mailbox {
    inner: Mutex<MailboxInner>,
    not_empty: Condvar,
    not_full: Condvar,
}

struct MailboxInner {
    queue: VecDeque<Command>,
    capacity: usize,
    closed: bool,
}

impl Mailbox {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(MailboxInner {
                queue: VecDeque::with_capacity(capacity),
                capacity,
                closed: false,
            }),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
        }
    }

    fn close(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        self.not_empty.notify_all();
        self.not_full.notify_all();
    }

    /// Enqueue a sync: replace an existing same-target sync if one sits after
    /// the last input; otherwise push (waiting when the mailbox is full).
    fn push_sync(&self, target: SyncTarget, packet: SyncPacket) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if g.closed {
                return Err(WorkerError::ShutDown);
            }
            let mut replace_at: Option<usize> = None;
            for (idx, cmd) in g.queue.iter().enumerate().rev() {
                if is_input(cmd) {
                    break;
                }
                if sync_target_of(cmd) == Some(target) {
                    replace_at = Some(idx);
                    break;
                }
            }
            if let Some(idx) = replace_at {
                if let Some(Command::Sync {
                    packet: slot,
                    target: t,
                }) = g.queue.get_mut(idx)
                {
                    debug_assert_eq!(*t, target);
                    slot.merge_latest(packet);
                }
                return Ok(());
            }
            if g.queue.len() < g.capacity {
                g.queue.push_back(Command::Sync { target, packet });
                self.not_empty.notify_one();
                return Ok(());
            }
            g = self.not_full.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Enqueue any non-sync command. Waits when full so input is never dropped.
    fn push(&self, cmd: Command) -> Result<(), WorkerError> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if g.closed {
                return Err(WorkerError::ShutDown);
            }
            if g.queue.len() < g.capacity {
                g.queue.push_back(cmd);
                self.not_empty.notify_one();
                return Ok(());
            }
            g = self.not_full.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn try_push_front_shutdown(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        g.queue.push_front(Command::Shutdown);
        self.not_empty.notify_all();
        self.not_full.notify_all();
    }

    fn recv_batch(&self) -> Option<Vec<Command>> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if !g.queue.is_empty() {
                let mut out = Vec::with_capacity(g.queue.len());
                while let Some(cmd) = g.queue.pop_front() {
                    out.push(cmd);
                }
                self.not_full.notify_all();
                return Some(out);
            }
            if g.closed {
                return None;
            }
            g = self.not_empty.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Triple buffer of finished frames for one surface (safe Rust, no `unsafe`).
///
/// The mutex guards only index swaps. Pixel buffers live in the slots and are
/// moved / `Arc`-cloned, never copied byte-by-byte on publish.
struct TripleFrameBuffer {
    slots: [Mutex<Option<WorkerFrame>>; PAINT_WORKER_FRAME_SLOTS],
    /// `(write_idx, published_idx)`.
    indices: Mutex<(usize, Option<usize>)>,
}

impl TripleFrameBuffer {
    fn new() -> Self {
        Self {
            slots: [Mutex::new(None), Mutex::new(None), Mutex::new(None)],
            indices: Mutex::new((0, None)),
        }
    }

    fn publish(&self, frame: WorkerFrame) {
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

    fn take_latest(&self) -> Option<WorkerFrame> {
        let published = {
            let idx = self.indices.lock().unwrap_or_else(|e| e.into_inner());
            idx.1?
        };
        let slot = self.slots[published]
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        slot.clone()
    }
}

struct SharedState {
    frames: Mutex<HashMap<SurfaceId, Arc<TripleFrameBuffer>>>,
    outcomes: Mutex<Vec<InputOutcome>>,
    status: Mutex<WorkerStatus>,
    error: Mutex<Option<WorkerError>>,
    /// Generation counter for input→frame latency (host increments on input).
    input_seq: AtomicU64,
    /// Last published frame's observed `input_seq`.
    frame_input_seq: AtomicU64,
    busy: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            frames: Mutex::new(HashMap::new()),
            outcomes: Mutex::new(Vec::new()),
            status: Mutex::new(WorkerStatus::default()),
            error: Mutex::new(None),
            input_seq: AtomicU64::new(0),
            frame_input_seq: AtomicU64::new(0),
            busy: AtomicBool::new(false),
        }
    }

    fn buffer_for(&self, id: SurfaceId) -> Arc<TripleFrameBuffer> {
        let mut map = self.frames.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(id)
            .or_insert_with(|| Arc::new(TripleFrameBuffer::new()))
            .clone()
    }

    fn take_frame(&self, id: SurfaceId) -> Option<WorkerFrame> {
        let map = self.frames.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&id)?.take_latest()
    }

    fn set_error(&self, err: WorkerError) {
        let mut g = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(err);
        }
    }
}

/// Host-side handle for a background paint thread.
///
/// `Send` and `Sync`. Drop joins the worker cleanly.
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
        assert_send_sync::<WorkerStatus>();
        assert_send_sync::<WorkerError>();
    }
    let _ = check;
};

impl PaintWorker {
    /// Spawn a worker thread that creates and owns the [`Engine`].
    pub fn spawn(config: EngineConfig) -> Self {
        let mailbox = Arc::new(Mailbox::new(PAINT_WORKER_COMMAND_CAPACITY));
        let shared = Arc::new(SharedState::new());
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

    pub fn declare_actions(&self, names: &[&str]) -> Result<(), WorkerError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::DeclareActions {
            names: names.iter().map(|s| (*s).to_string()).collect(),
            reply: Reply(tx),
        })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    pub fn mark_theme_or_stylesheet_changed(&self) -> Result<(), WorkerError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::MarkTheme { reply: Reply(tx) })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    pub fn open_surface(
        &self,
        id: SurfaceId,
        html: &str,
        viewport: Viewport,
        opts: SurfaceOpts,
    ) -> Result<(), WorkerError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::OpenSurface {
            id,
            html: html.to_string(),
            viewport,
            opts,
            reply: Reply(tx),
        })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    pub fn close_surface(&self, id: SurfaceId) -> Result<(), WorkerError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::CloseSurface {
            id,
            reply: Reply(tx),
        })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    pub fn set_visible(&self, id: SurfaceId, visible: bool) -> Result<(), WorkerError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::SetVisible {
            id,
            visible,
            reply: Reply(tx),
        })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    pub fn resize_surface(&self, id: SurfaceId, viewport: Viewport) -> Result<(), WorkerError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::ResizeSurface {
            id,
            viewport,
            reply: Reply(tx),
        })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    pub fn load_html(&self, html: &str) -> Result<(), WorkerError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::LoadHtml {
            html: html.to_string(),
            reply: Reply(tx),
        })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    /// Enqueue a sync for every live surface. May coalesce with a prior pending
    /// all-surface sync when no input sits between them.
    pub fn sync(&self, packet: SyncPacket) -> Result<(), WorkerError> {
        self.mailbox.push_sync(SyncTarget::All, packet)
    }

    /// Enqueue a sync for one surface. May coalesce with a prior pending sync
    /// for the same surface when no input sits between them.
    pub fn sync_surface(&self, id: SurfaceId, packet: SyncPacket) -> Result<(), WorkerError> {
        self.mailbox.push_sync(SyncTarget::Surface(id), packet)
    }

    /// Enqueue input for the default surface. Never dropped.
    pub fn input(&self, event: InputEvent) -> Result<(), WorkerError> {
        self.shared.input_seq.fetch_add(1, Ordering::SeqCst);
        self.mailbox.push(Command::Input {
            target: InputTarget::Default,
            event,
        })
    }

    /// Enqueue input for one surface. Never dropped.
    pub fn input_surface(&self, id: SurfaceId, event: InputEvent) -> Result<(), WorkerError> {
        self.shared.input_seq.fetch_add(1, Ordering::SeqCst);
        self.mailbox.push(Command::Input {
            target: InputTarget::Surface(id),
            event,
        })
    }

    pub fn complete(
        &self,
        request: RequestId,
        result: Result<(), Reason>,
    ) -> Result<(), WorkerError> {
        self.mailbox.push(Command::Complete { request, result })
    }

    /// Wait until the worker has drained every command queued before this call
    /// and painted dirty surfaces. Used by tests and setup code.
    pub fn flush(&self) -> Result<(), WorkerError> {
        if let Some(err) = self.poll_error() {
            return Err(err);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.mailbox.push(Command::Flush { reply: Reply(tx) })?;
        rx.recv().map_err(|_| WorkerError::ShutDown)?
    }

    /// Last finished frame for `id`, if the worker has published one. Never waits
    /// on paint.
    pub fn take_frame(&self, id: SurfaceId) -> Option<WorkerFrame> {
        self.shared.take_frame(id)
    }

    /// Drain dispatched input outcomes (actions, blocked, …) since the last poll.
    pub fn poll_outcomes(&self) -> Vec<InputOutcome> {
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

    /// Worker panic or fatal error, if any. The last finished frames stay available.
    pub fn poll_error(&self) -> Option<WorkerError> {
        self.shared
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Input sequence number observed by the most recently published paint.
    pub fn last_frame_input_seq(&self) -> u64 {
        self.shared.frame_input_seq.load(Ordering::SeqCst)
    }

    pub fn current_input_seq(&self) -> u64 {
        self.shared.input_seq.load(Ordering::SeqCst)
    }

    /// Test-only: ask the worker to panic. Last finished frames stay available.
    pub fn inject_panic_for_test(&self) -> Result<(), WorkerError> {
        self.mailbox.push(Command::TestPanic)
    }
}

impl Drop for PaintWorker {
    fn drop(&mut self) {
        self.mailbox.try_push_front_shutdown();
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
        while let Some(batch) = mailbox.recv_batch() {
            shared.busy.store(true, Ordering::SeqCst);
            let mut stop = false;
            let mut flush_replies: Vec<Reply<Result<(), WorkerError>>> = Vec::new();
            for cmd in batch {
                match process_command(&mut eng, cmd, &shared, &mut flush_replies) {
                    Ok(true) => {
                        stop = true;
                        break;
                    }
                    Ok(false) => {}
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
        let msg = panic_message(&payload);
        shared.set_error(WorkerError::Panic(msg));
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

/// Returns Ok(true) when the worker should shut down.
fn process_command(
    eng: &mut Engine,
    cmd: Command,
    shared: &SharedState,
    flush_replies: &mut Vec<Reply<Result<(), WorkerError>>>,
) -> Result<bool, WorkerError> {
    match cmd {
        Command::DeclareActions { names, reply } => {
            let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            eng.declare_actions(&refs);
            reply.send(Ok(()));
            Ok(false)
        }
        Command::MarkTheme { reply } => {
            eng.mark_theme_or_stylesheet_changed();
            reply.send(Ok(()));
            Ok(false)
        }
        Command::OpenSurface {
            id,
            html,
            viewport,
            opts,
            reply,
        } => {
            reply.send(
                eng.open_surface(id, &html, viewport, opts)
                    .map_err(Into::into),
            );
            Ok(false)
        }
        Command::CloseSurface { id, reply } => {
            reply.send(eng.close_surface(id).map_err(Into::into));
            Ok(false)
        }
        Command::SetVisible { id, visible, reply } => {
            reply.send(eng.set_visible(id, visible).map_err(Into::into));
            Ok(false)
        }
        Command::ResizeSurface {
            id,
            viewport,
            reply,
        } => {
            reply.send(eng.resize_surface(id, viewport).map_err(Into::into));
            Ok(false)
        }
        Command::LoadHtml { html, reply } => {
            reply.send(eng.load_html(&html).map_err(Into::into));
            Ok(false)
        }
        Command::Sync { target, packet } => {
            let slices = packet.clone();
            let mut frags = packet;
            let dirty = match target {
                SyncTarget::All => eng.sync(&slices, &mut frags)?,
                SyncTarget::Surface(id) => eng.sync_surface(id, &slices, &mut frags)?,
            };
            let mut st = shared.status.lock().unwrap_or_else(|e| e.into_inner());
            st.last_dirty = dirty;
            Ok(false)
        }
        Command::Input { target, event } => {
            let outcome = match target {
                InputTarget::Default => eng.input(event),
                InputTarget::Surface(id) => eng.input_surface(id, event),
            };
            push_outcome(shared, outcome);
            for o in eng.take_outcomes() {
                push_outcome(shared, o);
            }
            Ok(false)
        }
        Command::Complete { request, result } => {
            eng.complete(request, result);
            for o in eng.take_outcomes() {
                push_outcome(shared, o);
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
        Command::Shutdown => Ok(true),
    }
}

fn push_outcome(shared: &SharedState, outcome: InputOutcome) {
    if matches!(outcome, InputOutcome::None) {
        return;
    }
    shared
        .outcomes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(outcome);
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
        let output = eng.paint_surface(id)?;
        let paint_ns = t0.elapsed().as_nanos() as u64;
        let stats = eng.last_paint_stats();
        if output.skipped && output.rgba.is_none() && output.damage.is_empty() {
            continue;
        }
        let gen = {
            let entry = generations.entry(id).or_insert(0);
            *entry = entry.saturating_add(1);
            *entry
        };
        let frame = WorkerFrame {
            surface: id,
            output: Arc::new(output),
            stats: stats.clone(),
            generation: gen,
            paint_ns,
        };
        shared.buffer_for(id).publish(frame);
        shared.frame_input_seq.store(input_seq, Ordering::SeqCst);
        let mut st = shared.status.lock().unwrap_or_else(|e| e.into_inner());
        st.frames_painted = st.frames_painted.saturating_add(1);
        st.paint_stats = stats;
        st.timings = eng.last_timings();
    }
    for o in eng.take_outcomes() {
        push_outcome(shared, o);
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
    use std::time::Duration;

    fn pkt(tag: &str) -> SyncPacket {
        let mut fragments = HashMap::new();
        fragments.insert("sec".into(), tag.into());
        SyncPacket {
            versions: HashMap::from([("ticker".into(), 1)]),
            actions: Vec::new(),
            fragments,
        }
    }

    #[test]
    fn triple_buffer_fast_producer_slow_consumer() {
        let buf = TripleFrameBuffer::new();
        let make = |gen: u64| WorkerFrame {
            surface: SurfaceId::DEFAULT,
            output: Arc::new(PaintOutput {
                rgba: Some(RgbaImage {
                    width: 1,
                    height: 1,
                    pixels: vec![gen as u8, 0, 0, 255],
                }),
                texture: None,
                focus: FocusState::default(),
                skipped: false,
                damage: vec![Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                }],
            }),
            stats: PaintStats::default(),
            generation: gen,
            paint_ns: 0,
        };

        let mut last = 0u64;
        for gen in 1..=100u64 {
            buf.publish(make(gen));
            if gen % 7 == 0 {
                let frame = buf.take_latest().expect("frame");
                assert!(frame.generation >= last);
                last = frame.generation;
                let px = frame.output.rgba.as_ref().unwrap().pixels[0];
                assert_eq!(px, frame.generation as u8);
            }
        }
        let final_frame = buf.take_latest().unwrap();
        assert_eq!(final_frame.generation, 100);
    }

    #[test]
    fn mailbox_coalesces_same_target_syncs() {
        let mb = Mailbox::new(8);
        mb.push_sync(SyncTarget::All, pkt("a")).unwrap();
        mb.push_sync(SyncTarget::All, pkt("b")).unwrap();
        mb.push_sync(SyncTarget::All, pkt("c")).unwrap();
        let batch = mb.recv_batch().unwrap();
        assert_eq!(batch.len(), 1, "three All syncs must coalesce to one");
        match &batch[0] {
            Command::Sync { packet, .. } => {
                assert_eq!(packet.fragments.get("sec").map(String::as_str), Some("c"));
            }
            other => panic!("expected Sync, got {other:?}"),
        }
        // Input breaks the coalesce window.
        mb.push_sync(SyncTarget::All, pkt("d")).unwrap();
        mb.push(Command::Input {
            target: InputTarget::Default,
            event: InputEvent::PointerLeave,
        })
        .unwrap();
        mb.push_sync(SyncTarget::All, pkt("e")).unwrap();
        let batch = mb.recv_batch().unwrap();
        assert_eq!(batch.len(), 3);
        match &batch[0] {
            Command::Sync { packet, .. } => {
                assert_eq!(packet.fragments.get("sec").map(String::as_str), Some("d"));
            }
            _ => panic!("expected sync before input"),
        }
        assert!(is_input(&batch[1]));
        match &batch[2] {
            Command::Sync { packet, .. } => {
                assert_eq!(packet.fragments.get("sec").map(String::as_str), Some("e"));
            }
            _ => panic!("expected sync after input"),
        }
    }

    #[test]
    fn mailbox_input_waits_instead_of_dropping() {
        let mb = Arc::new(Mailbox::new(2));
        mb.push(Command::Input {
            target: InputTarget::Default,
            event: InputEvent::PointerLeave,
        })
        .unwrap();
        mb.push(Command::Input {
            target: InputTarget::Default,
            event: InputEvent::PointerLeave,
        })
        .unwrap();

        let mb2 = Arc::clone(&mb);
        let handle = thread::spawn(move || {
            mb2.push(Command::Input {
                target: InputTarget::Default,
                event: InputEvent::PointerLeave,
            })
        });

        thread::sleep(Duration::from_millis(20));
        assert!(
            !handle.is_finished(),
            "third input must wait on full mailbox"
        );
        let _ = mb.recv_batch();
        handle.join().unwrap().unwrap();
        let batch = mb.recv_batch().unwrap();
        assert_eq!(batch.len(), 1);
        assert!(is_input(&batch[0]));
    }
}
