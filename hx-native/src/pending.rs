//! Async pending state: RequestId, hx-request / aria-busy, hx-sync drop/queue.

use std::collections::HashMap;

use crate::types::{RequestId, TriggerInfo};

#[derive(Clone, Debug)]
pub struct PendingEntry {
    pub request: RequestId,
    pub source_id: String,
    #[allow(dead_code)]
    pub action_name: String,
    #[allow(dead_code)]
    pub args: Vec<(String, String)>,
    #[allow(dead_code)]
    pub trigger: TriggerInfo,
    pub indicator_id: Option<String>,
    pub frames_alive: u32,
}

#[derive(Clone, Debug)]
pub struct QueuedRelease {
    pub source_id: String,
    pub action_name: String,
    pub args: Vec<(String, String)>,
    pub trigger: TriggerInfo,
    pub indicator_id: Option<String>,
}

/// Raised by the test harness when a pending request times out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingTimeout {
    pub request: RequestId,
    pub source_id: String,
    pub frames: u32,
}

impl std::fmt::Display for PendingTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pending request {} on `{}` timed out after {} frames",
            self.request, self.source_id, self.frames
        )
    }
}

impl std::error::Error for PendingTimeout {}

#[derive(Default, Debug)]
pub struct PendingState {
    /// source_id -> pending entry
    pub by_source: HashMap<String, PendingEntry>,
    /// request -> source_id
    pub by_request: HashMap<RequestId, String>,
    /// source_id -> at most one queued release (hx-sync=queue)
    pub queued: HashMap<String, QueuedRelease>,
    /// Timed-out request ids (late complete is a no-op only for these).
    pub timed_out: HashMap<RequestId, ()>,
    /// Loud failures collected for the test harness.
    pub timeout_failures: Vec<PendingTimeout>,
    /// Unknown complete() calls (host bug).
    pub unknown_completes: Vec<RequestId>,
}

impl PendingState {
    pub fn clear_all(&mut self) {
        self.by_source.clear();
        self.by_request.clear();
        self.queued.clear();
        self.timed_out.clear();
        // Keep timeout_failures / unknown_completes for harness inspection across reload? Spec:
        // load_html drops all pending. Failures are harness-side.
        self.timeout_failures.clear();
        self.unknown_completes.clear();
    }

    pub fn is_pending(&self, source_id: &str) -> bool {
        self.by_source.contains_key(source_id)
    }

    pub fn insert(&mut self, entry: PendingEntry) {
        let source = entry.source_id.clone();
        let req = entry.request;
        self.by_request.insert(req, source.clone());
        self.by_source.insert(source, entry);
    }

    pub fn tick_frames(&mut self, timeout_frames: u32) {
        let mut timed = Vec::new();
        for entry in self.by_source.values_mut() {
            entry.frames_alive = entry.frames_alive.saturating_add(1);
            if entry.frames_alive > timeout_frames {
                timed.push((entry.request, entry.source_id.clone(), entry.frames_alive));
            }
        }
        for (request, source_id, frames) in timed {
            self.by_source.remove(&source_id);
            self.by_request.remove(&request);
            self.queued.remove(&source_id);
            self.timed_out.insert(request, ());
            #[cfg(debug_assertions)]
            eprintln!(
                "hx_native: pending request {request} on `{source_id}` timed out after {frames} frames"
            );
            self.timeout_failures.push(PendingTimeout {
                request,
                source_id,
                frames,
            });
        }
    }
}
