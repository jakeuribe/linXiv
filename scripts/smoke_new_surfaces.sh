#!/usr/bin/env bash
# Drives the CLI subcommands added by the surface-reconciliation work against a
# throwaway data dir. Hits arXiv: `fetch` is the only offline-free way to get a
# namespaced `arxiv:` source id, which every id-taking command requires.
# Usage: scripts/smoke_new_surfaces.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
export LINXIV_DATA_DIR="$SCRATCH/data"

cargo build --manifest-path "$REPO/src-tauri/Cargo.toml" -p linxiv-cli -q
# The bin is `linxiv-cli`; `target/debug/linxiv` is the staged sidecar and goes stale.
CLI="$REPO/src-tauri/target/debug/linxiv-cli"

step() { printf '\n=== %s ===\n' "$1"; }
jqp() { python3 -c "import json,sys; $1"; }

A=arxiv:1706.03762
B=arxiv:1512.03385

step "seed (network: arXiv)"
"$CLI" fetch 1706.03762 >/dev/null
"$CLI" fetch 1512.03385 >/dev/null
"$CLI" project create Smoke >/dev/null
"$CLI" list | jqp 'print("papers:", [p["source_id"] for p in json.load(sys.stdin)])'

step "project add-papers (bulk, all valid)"
"$CLI" project add-papers 1 "$A" "$B" | tee "$SCRATCH/bulk.json"
jqp 'd=json.load(sys.stdin); sys.exit(0 if d["ok"] and len(d["added"])==2 else 1)' <"$SCRATCH/bulk.json"

step "project add-papers (partial failure reported, not fatal)"
"$CLI" project add-papers 1 "$A" arxiv:0000.00000 | tee "$SCRATCH/partial.json" || true
jqp 'd=json.load(sys.stdin); sys.exit(0 if d["failed"]==["arxiv:0000.00000"] else 1)' <"$SCRATCH/partial.json"

step "author merge-candidates"
AID=$("$CLI" author list | jqp 'print(json.load(sys.stdin)[0]["author_id"])')
"$CLI" author merge-candidates "$AID"

step "author merge"
BEFORE=$("$CLI" author list | jqp 'print(len(json.load(sys.stdin)))')
OTHER=$("$CLI" author list | jqp 'print(json.load(sys.stdin)[1]["author_id"])')
"$CLI" author merge "$AID" "$OTHER" >/dev/null
AFTER=$("$CLI" author list | jqp 'print(len(json.load(sys.stdin)))')
echo "authors: $BEFORE -> $AFTER"
[ "$AFTER" -lt "$BEFORE" ] || { echo "FAIL: merge did not reduce author count"; exit 1; }

step "author merge rejects an unknown canonical"
"$CLI" author merge 99999 1 && { echo "FAIL: expected non-zero exit"; exit 1; }

step "paper doi-candidates"
"$CLI" paper doi-candidates "$A"

step "pdf list (empty before download)"
"$CLI" pdf list | jqp 'sys.exit(0 if json.load(sys.stdin)["pdfs"]==[] else 1)'

step "pdf download -> pdf list -> pdf delete"
"$CLI" pdf download "$A" https://arxiv.org/pdf/1706.03762 >/dev/null
"$CLI" pdf list | tee "$SCRATCH/pdfs.json"
jqp 'd=json.load(sys.stdin)["pdfs"]; sys.exit(0 if len(d)==1 and d[0]["size_bytes"]>0 else 1)' <"$SCRATCH/pdfs.json"
"$CLI" pdf delete "$A"
"$CLI" pdf list | jqp 'sys.exit(0 if json.load(sys.stdin)["pdfs"]==[] else 1)'
echo "pdf removed from listing after delete"

step "settings OPENALEX_MAILTO round-trip (env unset -> settings fallback)"
"$CLI" settings update OPENALEX_MAILTO smoke@example.org >/dev/null
env -u OPENALEX_MAILTO "$CLI" settings get \
  | jqp 'v=json.load(sys.stdin).get("OPENALEX_MAILTO"); print("stored:",v); sys.exit(0 if v=="smoke@example.org" else 1)'

printf '\nALL CLI SMOKE CHECKS PASSED\n'
