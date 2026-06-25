#!/usr/bin/env python3
"""Phase-0 golden-capture harness for the strangler-fig parity tests.

Plan refs: docs/rust-port-plan.md §7 Phase 0, §10 (Testing), D31.

Runs the Python CLI (`python -m linxiv_cli ...`) for a curated corpus of argv
against a FRESH temp LINXIV_DATA_DIR per command (never the real user data dir),
and writes each stdout byte-for-byte to goldens/cli/<slug>.{json,txt}. These
files are the frozen contract the Rust CLI must reproduce (canonicalized JSON)
before the Python CLI is deleted (D31).

Run with the project venv so the CLI's deps resolve:
    .venv/bin/python scripts/capture_cli_goldens.py

Lazy by design: no test framework, no normalization layer. The corpus is
restricted to commands whose stdout is deterministic and path-free on an empty
DB; everything that needs network, a seeded DB, file inputs, mutation, or path
normalization is parked in TODO_CORPUS below (NOT run).
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GOLDENS = REPO / "goldens" / "cli"

# ---------------------------------------------------------------------------
# SAFE CORPUS — read-only / structural, deterministic on a fresh empty DB.
# These actually run.
# ---------------------------------------------------------------------------

# Structural: argparse --help for the root + every top-level subcommand. Freezes
# the command tree the Rust clap derive (D9) must reproduce.
_HELP_CORPUS: list[list[str]] = [["--help"]] + [
    [cmd, "--help"]
    for cmd in (
        "search", "fetch", "list", "paper", "tag", "project", "note",
        "pdf", "trash", "doi", "author", "bibtex", "stats", "categories",
        "settings",
    )
]

# Data: read-only JSON commands that return a stable, path-free result on an
# empty DB.
_DATA_CORPUS: list[list[str]] = [
    ["stats"],
    ["list"],
    ["categories"],
    ["tag", "list-all"],
    ["author", "list"],
    ["project", "list"],
    ["trash", "list"],
    ["note", "list"],
    ["settings", "get"],
]

CORPUS: list[list[str]] = _HELP_CORPUS + _DATA_CORPUS

# ---------------------------------------------------------------------------
# TODO_CORPUS — DO NOT RUN NOW. Each needs a prerequisite that Phase 0's empty
# fresh-DB harness cannot provide deterministically. Capture in later phases
# once the listed prerequisite exists (D31). Kept as data, never iterated.
# ---------------------------------------------------------------------------

TODO_CORPUS: list[tuple[list[str], str]] = [
    # --- network: needs recorded wire fixtures (arXiv/OpenAlex/Crossref/DOI) ---
    (["search", "transformers"], "network: arXiv search; record wire bytes"),
    (["fetch", "2204.12985"], "network: arXiv fetch; record wire bytes"),
    (["doi", "resolve", "10.1145/3292500.3330701"], "network: DOI resolve"),
    (["doi", "save", "10.1145/3292500.3330701"], "network + mutating"),
    (["pdf", "download", "arxiv:2204.12985", "<url>"], "network + writes PDF"),
    # --- needs a seeded, deterministic fixture DB ---
    (["paper", "get", "arxiv:2204.12985"], "needs seeded DB"),
    (["paper", "versions", "arxiv:2204.12985"], "needs seeded DB"),
    (["paper", "search", "neural"], "needs seeded DB (FTS5)"),
    (["author", "get", "1"], "needs seeded DB"),
    (["tag", "list", "arxiv:2204.12985"], "needs seeded DB"),
    (["tag", "list-project", "1"], "needs seeded DB"),
    (["project", "get", "1"], "needs seeded DB"),
    (["note", "get", "1"], "needs seeded DB"),
    (["pdf", "path", "arxiv:2204.12985"], "needs seeded DB"),
    # --- path-bearing output: needs the D25 normalizer allowlist (R8) ---
    (["pdf", "storage"], "stdout embeds absolute pdf_dir under the data dir"),
    # --- mutating: changes DB state; capture as before/after pairs later ---
    (["project", "create", "My Project"], "mutating"),
    (["project", "update", "1", "--name", "X"], "mutating"),
    (["project", "delete", "1"], "mutating"),
    (["project", "archive", "1"], "mutating"),
    (["project", "restore", "1"], "mutating"),
    (["project", "hard-delete", "1"], "mutating"),
    (["project", "add-paper", "1", "arxiv:2204.12985"], "mutating"),
    (["project", "remove-paper", "1", "arxiv:2204.12985"], "mutating"),
    (["tag", "create", "ml"], "mutating"),
    (["tag", "delete", "1"], "mutating"),
    (["tag", "add", "arxiv:2204.12985", "ml"], "mutating"),
    (["tag", "remove", "arxiv:2204.12985", "ml"], "mutating"),
    (["tag", "add-project", "1", "ml"], "mutating"),
    (["tag", "remove-project", "1", "ml"], "mutating"),
    (["note", "create", "arxiv:2204.12985", "body"], "mutating"),
    (["note", "update", "1", "--content", "x"], "mutating"),
    (["note", "delete", "1"], "mutating"),
    (["author", "update", "1", "--orcid", "x"], "mutating"),
    (["author", "delete", "1"], "mutating"),
    (["paper", "delete", "arxiv:2204.12985"], "mutating"),
    (["paper", "repair", "arxiv:2204.12985"], "mutating"),
    (["paper", "restore", "arxiv:2204.12985"], "mutating"),
    (["paper", "hard-delete", "arxiv:2204.12985"], "mutating"),
    (["paper", "remove-from-all-projects", "arxiv:2204.12985"], "mutating"),
    (["trash", "restore", "arxiv:2204.12985"], "mutating"),
    (["trash", "hard-delete", "arxiv:2204.12985"], "mutating"),
    (["trash", "restore-project", "1"], "mutating"),
    (["trash", "hard-delete-project", "1"], "mutating"),
    (["settings", "update", "tex_rendering_enabled", "false"], "mutating"),
    # --- file-input: needs committed sample .pdf / .bib / .lxproj fixtures ---
    (["pdf", "import", "<file.pdf>"], "needs PDF fixture + mutating"),
    (["bibtex", "import", "<file.bib>"], "needs .bib fixture + mutating"),
    (["project", "import", "<file.lxproj>"], "needs .lxproj fixture + mutating"),
    (["project", "export", "1", "<dest>"], "needs seeded DB; writes a file"),
    (["project", "export-bibtex", "1", "<dest>"], "needs seeded DB; writes a file"),
    (["project", "export-obsidian", "1", "<dest>"], "needs seeded DB; writes a file"),
    # --- intentionally excluded: version string churns on every release ---
    (["--version"], "brittle: bumps every release; trivial parity, not frozen"),
]


def slug(argv: list[str]) -> str:
    """argv -> filesystem-safe base name (no extension). Leading dashes drop,
    other punctuation -> '-', tokens join with '_'."""
    parts = []
    for a in argv:
        a = a.lstrip("-")
        parts.append(re.sub(r"[^A-Za-z0-9.-]+", "-", a))
    return "_".join(parts)


def _is_text(argv: list[str]) -> bool:
    return any(a in ("--help", "-h", "--version") for a in argv)


def golden_path(argv: list[str]) -> Path:
    return GOLDENS / (slug(argv) + (".txt" if _is_text(argv) else ".json"))


def capture(argv: list[str]) -> tuple[Path, int, bytes]:
    """Run the CLI for argv against a fresh temp data dir; return (path, rc, stdout)."""
    with tempfile.TemporaryDirectory(prefix="linxiv-golden-") as data_dir:
        env = dict(os.environ)
        env["LINXIV_DATA_DIR"] = data_dir  # fresh + isolated; never the real dir
        env["COLUMNS"] = "80"              # pin argparse help wrapping
        proc = subprocess.run(
            [sys.executable, "-m", "linxiv_cli", *argv],
            cwd=REPO, env=env, capture_output=True,
        )
    out = golden_path(argv)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(proc.stdout)
    if proc.returncode != 0:
        sys.stderr.write(
            f"  WARN rc={proc.returncode} for {argv}\n{proc.stderr.decode(errors='replace')}\n"
        )
    return out, proc.returncode, proc.stdout


def _self_check() -> None:
    assert slug(["--help"]) == "help"
    assert slug(["project", "--help"]) == "project_help"
    assert slug(["tag", "list-all"]) == "tag_list-all"
    assert slug(["settings", "get"]) == "settings_get"
    assert golden_path(["--help"]).name == "help.txt"
    assert golden_path(["stats"]).name == "stats.json"


def main() -> int:
    _self_check()
    print(f"Capturing {len(CORPUS)} goldens -> {GOLDENS}")
    failures = 0
    for argv in CORPUS:
        path, rc, out = capture(argv)
        failures += rc != 0
        print(f"  [{'ok ' if rc == 0 else 'ERR'}] {' '.join(argv):40s} -> {path.name} ({len(out)} B)")
    print(f"Done. {len(CORPUS) - failures}/{len(CORPUS)} ok. "
          f"{len(TODO_CORPUS)} commands parked in TODO_CORPUS (not run).")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
