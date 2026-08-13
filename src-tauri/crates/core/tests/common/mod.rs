//! Shared fixtures for `linxiv-core`'s integration tests.
//!
//! One schema-initialised in-memory DB, so a test does not hand-roll its own —
//! roughly 20 in-module unit-test modules each define a private `mem()`/`db()`
//! copy; migrating those is a separate pass, new tests start here.

use linxiv_core::storage;
use rusqlite::Connection;

/// A fresh in-memory DB with the schema applied. Panics on failure — a fixture
/// that cannot open a memory DB has nothing to report but a broken test run.
pub fn db() -> Connection {
    let conn = storage::open_in_memory().expect("open in-memory DB");
    storage::init_db(&conn).expect("init schema");
    conn
}
