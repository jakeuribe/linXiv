"""Tests for linxiv_cli.py (headless CLI).

All cases require subprocess invocation or argparse harness setup.
Marked TODO pending a test harness decision (subprocess vs. direct call).
"""

from __future__ import annotations


# ---------------------------------------------------------------------------
# Argument validation
# ---------------------------------------------------------------------------

class TestArgValidation:
    # TODO: test that invalid arXiv ID format exits with error JSON on stderr
    # TODO: test that valid new-style ID (YYMM.NNNNN) passes validation
    # TODO: test that valid old-style ID (category/NNNNNNN) passes validation
    # TODO: test that missing required args print usage and exit non-zero
    pass


# ---------------------------------------------------------------------------
# search subcommand
# ---------------------------------------------------------------------------

class TestSearchCommand:
    # TODO: mock ArxivSource.search; verify output is JSON with "results" key
    # TODO: test --save flag calls db.save_paper_metadata for each result
    # TODO: test --max-results is forwarded to the source
    # TODO: test --source openalex routes to OpenAlexSource
    # TODO: test error from source prints JSON error and exits non-zero
    pass


# ---------------------------------------------------------------------------
# fetch subcommand
# ---------------------------------------------------------------------------

class TestFetchCommand:
    # TODO: mock fetch call; verify paper metadata printed as JSON
    # TODO: test --save flag persists paper to DB
    # TODO: test invalid ID caught before network call
    pass


# ---------------------------------------------------------------------------
# list subcommand
# ---------------------------------------------------------------------------

class TestListCommand:
    # TODO: seed temp DB; verify output contains saved paper IDs
    # TODO: test --limit and --offset flags
    # TODO: test empty DB produces empty JSON array
    pass


# ---------------------------------------------------------------------------
# projects subcommand
# ---------------------------------------------------------------------------

class TestProjectsCommand:
    # TODO: test create project, list projects, delete project roundtrip
    # TODO: test add-paper / remove-paper
    # TODO: test invalid project ID exits with error
    pass


# ---------------------------------------------------------------------------
# notes subcommand
# ---------------------------------------------------------------------------

class TestNotesCommand:
    # TODO: test create note and list notes for a paper
    # TODO: test --project-id filter
    pass
