//! Tauri shell of the linXiv desktop app: the invoke-command layer over
//! `linxiv-server` (which owns the router, state, and background tasks), the
//! `linxiv://` protocol handler, and the CLI/MCP-integration commands.
//! Consumed by `src/main.rs`; the headless/dev-server bins live in
//! `crates/server` and never touch this crate.

pub mod commands;
pub mod integrations;
pub mod protocol;
pub mod remote_backend;

// The shared backend, re-exported so app modules (and tests) keep their
// `crate::route`-style paths.
pub use linxiv_server::{full_text_worker, p2p_config, remote_query, route, share_sync, state};
