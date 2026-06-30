//! `GuiPermissionHandler` — bridges the synchronous `PermissionHandler` trait
//! onto an asynchronous GUI modal.
//!
//! The library's `PermissionHandler::request_permission` is a *synchronous*
//! method (it returns a `PermissionDecision` directly), but a GUI permission
//! dialog is inherently asynchronous: the background agent task must wait until
//! the user clicks a button in the foreground window.
//!
//! To reconcile the two worlds, this handler:
//!
//! 1. Holds an `async_channel::Sender` over a GUI-side `PermissionRequest`
//!    (a distinct type that carries a `tokio::sync::oneshot` reply channel).
//!    The foreground UI owns the matching `Receiver` and renders a modal for
//!    every request it pulls off it.
//! 2. In the synchronous `request_permission` method, it clones the relevant
//!    fields out of the library's borrowed `PermissionRequest`, attaches a
//!    fresh oneshot reply channel, pushes the request through the channel
//!    with `send_blocking`, and then blocks the calling task until the GUI
//!    posts its decision back.
//!
//! Blocking inside an async context is done via
//! `tokio::task::block_in_place(|| rx.blocking_recv())`, mirroring the
//! existing pattern used by the tool layer (see `src/tools/mod.rs`). This
//! requires a multi-threaded runtime, which is the same assumption the rest
//! of the GUI already makes.
//!
//! `AllowPermanently` decisions are cached per `tool_name` so subsequent
//! invocations of the same tool are fast-pathed without bouncing through the
//! UI again.

use std::collections::HashSet;

use async_channel::Sender;
use tokio::sync::oneshot;

use crate::core::permissions::{
    PermissionDecision, PermissionHandler, PermissionRequest as CorePermissionRequest,
};

/// A permission request sent from the background agent to the GUI foreground.
///
/// This is intentionally a *separate* type from the library's
/// `core::permissions::PermissionRequest`: it owns its data and carries the
/// `oneshot` reply channel the GUI uses to post the user's decision back to
/// the blocked agent task.
pub struct PermissionRequest {
    /// Unique id for this request (used by the UI for tracking / dedup).
    pub id: String,
    /// Tool that wants permission (e.g. `"Bash"`, `"Edit"`).
    pub tool_name: String,
    /// Human-readable one-line summary of the operation.
    pub description: String,
    /// Optional extra detail shown in an expandable section of the modal.
    pub details: Option<String>,
    /// Whether the operation is side-effect free (read-only).
    pub is_read_only: bool,
    /// Canonical / resolved target path, when the decision is path-sensitive.
    pub path: Option<String>,
    /// Context-aware explanation of *why* the tool needs permission.
    pub context_description: Option<String>,
    /// Channel the GUI uses to deliver the user's decision back to the agent.
    pub reply_tx: oneshot::Sender<PermissionDecision>,
}

/// GUI-side `PermissionHandler` that delegates interactive (`Ask`) decisions
/// to a foreground modal via an `async_channel`.
///
/// `AllowPermanently` responses are cached per `tool_name` so that, once a
/// user has chosen "always allow" for a tool, subsequent calls short-circuit
/// straight to `Allow` without round-tripping through the UI.
pub struct GuiPermissionHandler {
    /// Sends each request to the foreground UI that renders the modal.
    request_tx: Sender<PermissionRequest>,
    /// `tool_name`s the user has allowed permanently this session.
    always_allow: parking_lot::Mutex<HashSet<String>>,
}

impl GuiPermissionHandler {
    /// Create a new handler that forwards interactive permission requests to
    /// the GUI over `request_tx`.
    pub fn new(request_tx: Sender<PermissionRequest>) -> Self {
        Self {
            request_tx,
            always_allow: parking_lot::Mutex::new(HashSet::new()),
        }
    }

    /// Build the GUI-side request envelope for a library request, attaching a
    /// fresh reply channel. Returns the envelope together with the receiver
    /// the agent should block on.
    fn build_gui_request(
        &self,
        request: &CorePermissionRequest,
    ) -> (PermissionRequest, oneshot::Receiver<PermissionDecision>) {
        let (reply_tx, reply_rx) = oneshot::channel();
        let gui_request = PermissionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: request.tool_name.clone(),
            description: request.description.clone(),
            details: request.details.clone(),
            is_read_only: request.is_read_only,
            path: request.path.clone(),
            context_description: request.context_description.clone(),
            reply_tx,
        };
        (gui_request, reply_rx)
    }

    /// Human-readable reason used when escalating a non-cached request to the
    /// user via `Ask`.
    fn ask_reason(request: &CorePermissionRequest) -> String {
        request
            .context_description
            .clone()
            .or_else(|| request.details.clone())
            .unwrap_or_else(|| format!("Tool '{}' requests permission", request.tool_name))
    }
}

impl PermissionHandler for GuiPermissionHandler {
    fn check_permission(&self, request: &CorePermissionRequest) -> PermissionDecision {
        // Fast path: the user already allowed this tool permanently.
        if self.always_allow.lock().contains(&request.tool_name) {
            return PermissionDecision::Allow;
        }
        // Otherwise signal that we need to ask the user.
        PermissionDecision::Ask {
            reason: Self::ask_reason(request),
        }
    }

    fn request_permission(&self, request: &CorePermissionRequest) -> PermissionDecision {
        // 1. Fast path: previously allowed permanently.
        if self.always_allow.lock().contains(&request.tool_name) {
            return PermissionDecision::Allow;
        }

        // 2. Build the GUI envelope with a reply channel.
        let (gui_request, reply_rx) = self.build_gui_request(request);

        // 3. Hand the request to the foreground UI. `send_blocking` parks
        //    the current thread until the channel has capacity (or is closed).
        if self.request_tx.send_blocking(gui_request).is_err() {
            // The GUI receiver was dropped (e.g. window closed): deny safely.
            tracing::warn!(
                tool = %request.tool_name,
                "permission channel closed; denying tool"
            );
            return PermissionDecision::Deny;
        }

        // 4. Block the agent task until the GUI posts its decision back.
        //    `block_in_place` moves the blocking off the runtime's worker
        //    pool so the UI task can still make progress and render the modal.
        let decision = tokio::task::block_in_place(|| reply_rx.blocking_recv());

        let decision = match decision {
            Ok(d) => d,
            Err(_) => {
                // The GUI dropped the reply sender without answering
                // (e.g. the modal was dismissed). Default to deny so we never
                // silently proceed with a privileged operation.
                tracing::warn!(
                    tool = %request.tool_name,
                    "permission reply dropped; denying tool"
                );
                return PermissionDecision::Deny;
            }
        };

        // 5. Cache permanent allows so we don't re-prompt for the same tool.
        if let PermissionDecision::AllowPermanently = decision {
            self.always_allow.lock().insert(request.tool_name.clone());
        }

        decision
    }
}
