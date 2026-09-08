//! Shared fixtures for `linxiv-core`'s integration tests: one schema-initialised
//! in-memory DB. Deliberately a copy of `src/test_support.rs::db()` — that module
//! is `#[cfg(test)]` and `tests/` only sees the public API; four lines stay
//! duplicated rather than exposing a test-only module publicly.

use linxiv_core::storage;
use rusqlite::Connection;

/// A fresh in-memory DB with the schema applied. Panics on failure — a fixture
/// that cannot open a memory DB has nothing to report but a broken test run.
pub fn db() -> Connection {
    let conn = storage::open_in_memory().expect("open in-memory DB");
    storage::init_db(&conn).expect("init schema");
    conn
}
