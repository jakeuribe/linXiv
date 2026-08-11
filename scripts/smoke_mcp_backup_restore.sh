#!/usr/bin/env bash
# Exercises the MCP backup_database / restore_database tools over real stdio JSON-RPC
# against a throwaway data dir. These are the destructive additions and cannot be
# covered by the in-process unit tests (restore replaces the file the tests run on).
#
# The server answers requests CONCURRENTLY, so this drives it one call at a time,
# reading each response before sending the next — piping all requests at once
# lets restore run before backup has written its file.
# Usage: scripts/smoke_mcp_backup_restore.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
export LINXIV_DATA_DIR="$SCRATCH/data"

cargo build --manifest-path "$REPO/src-tauri/Cargo.toml" -p linxiv-mcp -q

MCP="$REPO/src-tauri/target/debug/linxiv-mcp" BAK="$SCRATCH/snap.db" python3 <<'PY'
import json, os, subprocess, sys

bak = os.environ["BAK"]
p = subprocess.Popen([os.environ["MCP"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True, bufsize=1)

def send(obj):
    p.stdin.write(json.dumps(obj) + "\n")
    p.stdin.flush()

def rpc(i, method, params):
    """Send one request and block until its own id comes back."""
    send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
    while True:
        line = p.stdout.readline()
        if not line:
            sys.exit(f"FAIL: server closed the stream waiting for id {i}")
        msg = json.loads(line)
        if msg.get("id") != i:
            continue
        if "error" in msg:
            return {"__rpc_error__": msg["error"]}
        content = msg["result"].get("content") or []
        return json.loads(content[0]["text"]) if content else msg["result"]

def call(i, name, args):
    return rpc(i, "tools/call", {"name": name, "arguments": args})

def names(v):
    rows = v if isinstance(v, list) else v.get("projects", [])
    return [r.get("name") for r in rows]

rpc(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                      "clientInfo": {"name": "smoke", "version": "0"}})
send({"jsonrpc": "2.0", "method": "notifications/initialized"})

call(2, "create_project", {"name": "Survivor"})
print("backup     :", call(3, "backup_database", {"dest": bak}))
call(4, "hard_delete_project", {"project_id": 1})

after_delete = call(5, "list_projects", {})
print("after wipe :", names(after_delete))
if names(after_delete):
    sys.exit(f"FAIL: project survived hard_delete: {after_delete}")

print("restore    :", call(6, "restore_database", {"src": bak}))
after_restore = call(7, "list_projects", {})
print("after undo :", names(after_restore))
if "Survivor" not in names(after_restore):
    sys.exit(f"FAIL: restore did not bring the project back: {after_restore}")

# The server must keep working on its reopened handle, not just report ok.
live = call(8, "create_project", {"name": "PostRestore"})
if not isinstance(live, dict) or live.get("name") != "PostRestore":
    sys.exit(f"FAIL: server unusable after restore: {live}")
print("post-restore write: ok")

rel = call(9, "backup_database", {"dest": "relative.db"})
if "__rpc_error__" not in rel and not (isinstance(rel, dict) and rel.get("isError")):
    sys.exit(f"FAIL: a relative dest was accepted: {rel}")
print("relative   : refused as expected")

p.stdin.close()
p.wait(timeout=10)
print("\nMCP BACKUP/RESTORE ROUND-TRIP PASSED")
PY
