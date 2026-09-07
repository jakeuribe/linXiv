//! Tauri-free shared backend: the HTTP-shaped router over `linxiv-core` plus
//! app state, share sync, the full-text worker, and the remote-query node
//! half. Consumed by three front doors — the Tauri app (`linxiv-app`, which
//! layers its command wrappers and the `linxiv://` protocol on top), the
//! dev-only HTTP shim (`src/bin/dev_server.rs`, D32), and the headless node
//! (`src/bin/headless.rs`).

pub mod full_text_worker;
pub mod p2p_config;
pub mod remote_query;
pub mod route;
pub mod share_sync;
pub mod state;
#[cfg(test)]
mod ts_bindings;
