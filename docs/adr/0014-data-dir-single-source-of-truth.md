# ADR 0014: LINXIV_DATA_DIR is the single source of truth for the runtime data dir

## Status

Accepted

## Context

linXiv writes several kinds of per-user runtime data to disk: the SQLite database (`papers.db`), managed PDFs (`pdfs/`), user settings (`user_settings.json`), the Obsidian export vault (`obsidian_vault/arXivVault/`), and the arXiv rate-limit timestamp (`.arxiv_ratelimit`). All four processes — the Tauri desktop app, the headless CLI, the MCP server, and the migration tool — must agree on where the library lives.

Previously `config.data_dir()` returned `$LINXIV_DATA_DIR` when set (Tauri sets it on the spawned subprocess) and **otherwise fell back to the repository root** (`_PROJECT_ROOT`). That fallback was convenient in development but had real problems:

- It conflated the **source tree** (a code artifact) with **user data** (runtime state). A developer running the CLI or MCP directly accumulated `papers.db`, `pdfs/`, and `.arxiv_ratelimit` inside the checkout.
- The location was resolved inconsistently: some sites (`storage/db.py`, `storage/config/core.py`) froze `DB_PATH` once at import time; others resolved per call.
- Not every path went through `data_dir()` at all. The Obsidian vault and the arXiv rate-limit file were pinned to `Path(__file__).parent.parent` (the source tree), so they did not follow `$LINXIV_DATA_DIR` even in a packaged build — notes and the rate-limit file were written next to the source instead of into the user's data dir.

A full audit of every explicit and implicit hardcoding of the DB location preceded this decision (working note `DB_LOCATION_AUDIT.md` at the repo root).

## Decision

`$LINXIV_DATA_DIR` is the single source of truth for the runtime data dir. The repo root is no longer a fallback for runtime data.

`config.py` exposes:

- **`data_dir()`** — returns `Path($LINXIV_DATA_DIR)` if set, else `_default_data_dir()`. Resolved on **every call** so it tracks the env var dynamically; never cached at import.
- **`_default_data_dir()`** — the OS per-user app-data dir for the bundle identifier `com.linxiv.app`, matching Tauri's `app_data_dir()` per platform (Linux: `$XDG_DATA_HOME` or `~/.local/share`; macOS: `~/Library/Application Support`; Windows: `%APPDATA%`). Used only when `$LINXIV_DATA_DIR` is unset (dev / CLI / MCP launched without Tauri). Never the repo.
- **`init_data_dir()`** — resolves `data_dir()`, writes it back to `$LINXIV_DATA_DIR` (pinning it for the process lifetime and inherited by child processes), and `mkdir`s it. Called once at startup of **every** entry point: `linxiv_cli.py:main()`, `linxiv_mcp.py` (module load), `api/app.py` FastAPI lifespan, `migrate_db.py:main()`. This is what "initialize the data dir on any run" means in practice.
- **`repo_dir()`** — the source-tree root, for developer/repo artifacts only. The dev `.env` stays repo-anchored: `ENV_PATH = repo_dir() / ".env"`. Deliberately distinct from `data_dir()`.
- **`resources_dir()`** — read-only bundled resources (SQL schema, format templates; `sys._MEIPASS` when frozen). A separate axis from `data_dir()`; unchanged by this decision.

All runtime paths resolve through `data_dir()` per use:

| Path | Resolver |
|---|---|
| `papers.db` | `storage/paths.py:db_path()` |
| `pdfs/` | `storage/paths.py:pdf_dir()` |
| `obsidian_vault/arXivVault/` | `sources/fetch_paper_metadata.py:_vault_dir()` |
| `.arxiv_ratelimit` | `sources/fetch_paper_metadata.py:_ratelimit_file()` |
| `user_settings.json` | `user_settings.py` |

Tests pin `LINXIV_DATA_DIR` to a `tmp_path` via `monkeypatch.setenv` (in `tests/conftest.py`'s `tmp_db` fixture and `tests/test_fetch_paper_metadata.py`'s `temp_ratelimit` fixture) so they never read or write the real OS data dir. Because `pdf_dir()` resolves dynamically, the former per-module `_pdf_dir` monkeypatches were removed as redundant.

The alternative — keeping the repo-root dev fallback — was rejected because it is the root cause of source-tree pollution and of the "which DB am I actually using?" ambiguity that motivated the audit.

## Consequences

### Positive

- One unambiguous answer to "where is the library?" across all four processes.
- Directly-launched CLI/MCP/API use the same OS location as the packaged app, instead of scattering data into the checkout.
- The Obsidian vault and arXiv rate-limit file now follow `$LINXIV_DATA_DIR` like everything else.
- `init_data_dir()` guarantees the directory exists before any DB/PDF/vault access on any run.

### Negative / limits

- `init_data_dir()` pins `$LINXIV_DATA_DIR` for the process lifetime. Tests that redirect the data dir must set/restore the env var (pytest's `monkeypatch.setenv` handles this); documented in the function's docstring.
- The dev `.env` and runtime data now live in different places by design. Anyone expecting a single repo-local folder must look in the OS app-data dir (or set `$LINXIV_DATA_DIR`).
- Two import-time-frozen `DB_PATH` constants remain (`storage/db.py`, `storage/config/core.py`). Collapsing them so `_connect()` resolves `db_path()` per connection is the prerequisite for any runtime DB relocation; it is a behaviour change and was deliberately left out of this ADR.
- A repo-dir ⇄ data-dir **sync** feature (dev ⇄ installed data) is potentially destructive (direction, conflict policy, atomicity unresolved) and must be designed before implementation.

## References

- `config.py` — `data_dir`, `_default_data_dir`, `init_data_dir`, `repo_dir`, `resources_dir`, `ENV_PATH`
- `storage/paths.py` — `db_path`, `pdf_dir`, `old_pdf_dir`
- `sources/fetch_paper_metadata.py` — `_vault_dir`, `_ratelimit_file`
- `linxiv_cli.py`, `linxiv_mcp.py`, `api/app.py`, `migrate_db.py` — `init_data_dir()` startup calls
- `src-tauri/src/main.rs`, `src-tauri/src/integrations.rs` — `LINXIV_DATA_DIR` producer (`app_data_dir()`)
- `tests/conftest.py`, `tests/test_fetch_paper_metadata.py` — env-pin test isolation
- `DB_LOCATION_AUDIT.md` — preceding hardcoding audit
