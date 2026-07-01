//! Service layer for the UI.
//!
//! Mirrors chat-ai's `services/` structure, but the `agent` submodule is
//! re-implemented on top of this crate's `local_workflow_agent` library
//! rather than the original `smolhttp` + Anthropic-only client.

pub mod agent;
