//! Shared fixtures for `linxiv-core`'s integration tests.
//!
//! One schema-initialised in-memory DB, so a test does not hand-roll its own.
//!
//! Deliberately a copy of `src/test_support.rs`'s `db()`: that module is
//! `#[cfg(test)]`, and `tests/` is a separate compilation unit that can only
//! see `linxiv_core`'s public API. Four lines, so it stays duplicated rather
//! than exposing a test-only module from the crate's public surface.

use linxiv_core::storage;
use rusqlite::Connection;

/// A fresh in-memory DB with the schema applied. Panics on failure — a fixture
/// that cannot open a memory DB has nothing to report but a broken test run.
pub fn db() -> Connection {
    let conn = storage::open_in_memory().expect("open in-memory DB");
    storage::init_db(&conn).expect("init schema");
    conn
}
