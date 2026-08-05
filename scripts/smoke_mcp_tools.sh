#!/usr/bin/env bash
# Lists the MCP server's tools over real stdio JSON-RPC and asserts the tools added
# by the surface-reconciliation work are registered. Unit tests call the methods
# directly and would not catch a macro-registration miss.
#
# Reads until the tools/list response arrives rather than sleeping a fixed interval:
# a cold cache or a large DB can push init past any fixed wait, and that would
# surface as "no tools registered" — a registration bug that isn't there.
# Usage: scripts/smoke_mcp_tools.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
export LINXIV_DATA_DIR="$SCRATCH/data"

cargo build --manifest-path "$REPO/src-tauri/Cargo.toml" -p linxiv-mcp -q

MCP="$REPO/src-tauri/target/debug/linxiv-mcp" python3 <<'PY'
import json, os, subprocess, sys

p = subprocess.Popen([os.environ["MCP"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True, bufsize=1)

def send(obj):
    p.stdin.write(json.dumps(obj) + "\n")
    p.stdin.flush()

def rpc(i, method, params):
    send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
    while True:
        line = p.stdout.readline()
        if not line:
            sys.exit(f"FAIL: server closed the stream waiting for id {i}")
        msg = json.loads(line)
        if msg.get("id") == i:
            if "error" in msg:
                sys.exit(f"FAIL: id {i} returned {msg['error']}")
            return msg["result"]

rpc(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                      "clientInfo": {"name": "smoke", "version": "0"}})
send({"jsonrpc": "2.0", "method": "notifications/initialized"})

names = [t["name"] for t in rpc(2, "tools/list", {}).get("tools", [])]
if not names:
    sys.exit("FAIL: server returned no tools")
print(f"registered tools: {len(names)}")

expected = [
    "full_text_pending",
    "backup_database",
    "restore_database",
    "list_pdfs",
    "delete_pdf",
    "find_doi_candidates",
    "author_merge_candidates",
    "add_papers_to_project",
]
for n in expected:
    print(f"  {'OK ' if n in names else 'MISS'} {n}")
missing = [n for n in expected if n not in names]
if missing:
    sys.exit(f"FAIL: not registered: {missing}")

p.stdin.close()
p.wait(timeout=10)
print("\nALL MCP TOOLS REGISTERED")
PY
