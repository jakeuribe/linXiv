//! Shared in-process backend for the linXiv desktop app: the HTTP-shaped router
//! over `linxiv-core` plus app state, the `linxiv://` protocol handler, and the
//! CLI/MCP-integration commands. Consumed by two binaries — the Tauri app
//! (`src/main.rs`) and the dev-only HTTP shim (`src/bin/dev_server.rs`, D32).

pub mod full_text_worker;
pub mod integrations;
pub mod p2p_config;
pub mod protocol;
pub mod route;
pub mod share_sync;
pub mod state;
