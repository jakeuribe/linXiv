//! Tauri-free shared backend: the HTTP-shaped router over `linxiv-core`.
//! Consumed by the Tauri app, the dev-only HTTP shim (D32), and the headless node.

pub mod full_text_worker;
pub mod journal;
pub mod p2p_config;
pub mod remote_query;
pub mod route;
pub mod share_sync;
pub mod state;
#[cfg(test)]
mod ts_bindings;
