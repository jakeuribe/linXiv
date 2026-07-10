# Golden captures — the frozen Python CLI contract

These files are the **frozen output contract** for the strangler-fig Rust port
(`docs/rust-port-plan.md` §7 Phase 0, §10, **D31**). They are byte-for-byte
captures of the Python CLI's stdout, recorded **before** any Python code is
deleted. The `.json` goldens are the **byte-for-byte (canonicalized) parity
contract** the Rust CLI must reproduce. The `.txt` `--help` goldens are a
**structural reference only** — clap cannot reproduce argparse help verbatim
(different section headers, wrapping), so compare command/flag *presence*, not
exact text — before the corresponding Python surface is removed.

> D31: "Capture Python HTTP/format goldens in Phase 0 regardless — the
> app/command surface is verified against those frozen goldens after HTTP
> deletion." This is the CLI half of that capture.

## Layout

```
goldens/cli/<slug>.json   read-only data commands (JSON on stdout, empty DB)
goldens/cli/<slug>.txt    argparse --help text (the command-tree structure / D9)
```

`<slug>` is the argv with leading dashes stripped, tokens joined by `_`
(e.g. `tag list-all` -> `tag_list-all.json`, `project --help` -> `project_help.txt`).

## Regenerate

Run with the **project venv** (the CLI needs its deps); the global interpreter
will not have them:

```sh
.venv/bin/python scripts/capture_cli_goldens.py
```

The harness:

- runs each argv via `python -m linxiv_cli ...` in a **fresh temp
  `LINXIV_DATA_DIR` per command** — it never touches the real user data dir, so
  every JSON golden is an *empty-DB* result;
- pins `COLUMNS=80` so argparse help wraps identically across machines;
- writes stdout verbatim (bytes) to `goldens/cli/`.

It is deterministic: re-running produces byte-identical files (verified with
`diff -r`). If a `.json` golden changes, the CLI's output contract changed —
treat that as a deliberate decision, not noise. Help `.txt` goldens also depend
on the **Python/argparse version**; regenerate with the pinned interpreter.

## What is captured (the safe corpus)

Only commands whose stdout is deterministic and **path-free** on an empty DB:

- structural: `--help` for the root and all 15 top-level subcommands;
- data: `stats`, `list`, `categories`, `tag list-all`, `author list`,
  `project list`, `trash list`, `note list`, `settings get`.

## What is NOT captured yet (and why)

`scripts/capture_cli_goldens.py` parks the rest in a clearly-labeled
`TODO_CORPUS` (data only, never run). Each is blocked on a Phase-0-unavailable
prerequisite:

- **network** (`search`, `fetch`, `doi resolve/save`, `pdf download`) — needs
  recorded wire-body fixtures (arXiv/OpenAlex/Crossref/DOI), per §10.
- **seeded DB** (`paper get/versions/search`, `author get`, `project get`,
  `note get`, `tag list/list-project`, `pdf path`) — needs a committed,
  deterministic fixture DB.
- **path-bearing output** (`pdf storage`) — stdout embeds the absolute data-dir
  path; needs the D25/R8 normalizer allowlist before it can be a stable golden.
- **mutating** (all `create/update/delete/restore/archive/hard-delete`,
  tag/note edits, `settings update`, project membership) — capture later as
  before/after pairs.
- **file-input** (`pdf import`, `bibtex import`, `project import/export*`) —
  needs committed `.pdf` / `.bib` / `.lxproj` sample fixtures.
- **`--version`** — intentionally excluded; the string bumps every release, so
  freezing it is pure churn. Rust parity for it is trivial and unfrozen.

When those prerequisites land in later phases, move the entry from `TODO_CORPUS`
into `CORPUS` and re-run.
