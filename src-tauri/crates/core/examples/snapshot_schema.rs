//! Dumps the exact schema a fresh install produces at this commit: every
//! table/index/view/trigger definition from `sqlite_master`, sorted for a
//! deterministic diff. Used by scripts/snapshot_schema.sh to archive the real
//! per-release schema (built from this commit's actual init_db, not a
//! hand-typed reconstruction) into the private docs repo -- see that script
//! and TODO.md for why it's kept out of the public repo.
//!
//! Run directly with `cargo run -p linxiv-core --example snapshot_schema`.

fn main() {
    let conn = linxiv_core::storage::open_in_memory().expect("open in-memory db");
    linxiv_core::storage::init_db(&conn).expect("init_db");

    // FTS5 shadow tables (papers_fts_data/_idx/_docsize/_config, etc.) are
    // auto-generated storage internals whose exact DDL can drift with the
    // bundled SQLite/FTS5 version -- excluding them keeps release-to-release
    // diffs meaningful (real schema changes only), rather than noisy with
    // version-driven shadow-table churn. `pragma_table_list` (SQLite 3.37+)
    // tags them `type = 'shadow'`, so this excludes by that property rather
    // than guessing at name suffixes.
    let mut stmt = conn
        .prepare(
            "SELECT sql FROM sqlite_master \
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
             AND name NOT IN (SELECT name FROM pragma_table_list() WHERE type = 'shadow') \
             ORDER BY type, name",
        )
        .expect("prepare");
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query");
    for sql in rows {
        println!("{};\n", sql.expect("row").trim_end());
    }
}
