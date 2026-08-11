# Golden captures — the CLI output contract

These files freeze the stdout of the **Rust CLI** (`src-tauri/crates/cli`,
binary `linxiv-cli`, clap). They started life as Python/argparse captures for
the strangler-fig port; the Python CLI is gone, so today they are simply the
wire contract the Rust CLI must keep reproducing.

## Layout

```
goldens/cli/<slug>.json   read-only data commands (JSON on stdout, empty DB)
goldens/cli/<slug>.txt    command-tree reference (help text)
```

`<slug>` is the argv with leading dashes stripped, tokens joined by `_`
(e.g. `tag list-all` -> `tag_list-all.json`, `project --help` -> `project_help.txt`).

- **`.json`** — byte-for-byte contract. The Rust CLI's empty-DB output must
  match exactly (keys stay in Python-parity insertion order via serde_json's
  `preserve_order`).
- **`.txt`** — **structural reference only**. These are still in the frozen
  argparse help format; clap renders help differently (`Usage:`, `Commands:`,
  different wrapping), so compare command/flag *presence* against
  `linxiv-cli ... --help`, not exact text. They are hand-maintained when the
  command tree changes — no tool regenerates them.

## Refresh (manual — there is no capture script)

`scripts/capture_cli_goldens.py` no longer exists. To refresh the `.json`
goldens, run the built binary with a **fresh temp `LINXIV_DATA_DIR` per
command** so every capture is an empty-DB result and the real data dir is
never touched:

```sh
cd src-tauri && cargo build --release -p linxiv-cli && cd ..
cli=src-tauri/target/release/linxiv-cli
for argv in stats list categories "tag list-all" "author list" \
            "project list" "trash list" "note list" "settings get"; do
  slug=$(printf %s "$argv" | tr ' ' '_')
  LINXIV_DATA_DIR=$(mktemp -d) $cli $argv > "goldens/cli/$slug.json"
done
```

(POSIX sh/bash — zsh needs `${=cli} ${=argv}` for the word splitting.)

A changed `.json` means the CLI's output contract changed — treat that as a
deliberate decision, not noise. `--version` is intentionally uncaptured; it
bumps every release.

## Caveat: nothing runs these

**No test or CI job compares `goldens/cli/` against the CLI.** Drift is
silent — it has already accumulated once (missing commands and settings keys
were backfilled by hand). Wiring a runner is tracked in TODO.md
("Nothing runs the CLI goldens", Surface parity section).

## Corpus

Only commands whose stdout is deterministic and path-free on an empty DB:
help text for the root and each top-level group, plus the data commands
listed in the refresh loop above. Network commands (`search`, `fetch`, `doi`,
`pdf download`), seeded-DB reads, path-bearing output (`pdf storage`),
mutations, and file-input commands (`pdf import`, `bibtex import`,
`project import/export`) are not captured — each needs fixtures or output
normalization that doesn't exist yet.
