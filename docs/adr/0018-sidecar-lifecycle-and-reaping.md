# ADR 0018: Sidecar identity, reaping, and orphan cleanup

## Status

Accepted

## Context

The Tauri launcher (`src-tauri/src/main.rs`) runs the Python backend as a child
process: a PyInstaller `--onefile` sidecar (`linxiv-api`) in release, `uv run
python -m api` in dev. Two defects made rebuilds appear to have no effect:

1. **Adoption of a stale process.** The launcher picked a port, then trusted any
   process answering `GET /api/health` with `{"service": "linxiv-api"}`. A stale
   build still running on the default port `:8000` passed that check and was
   reused — so the app kept talking to old code while the new build sat idle.
   (This is what made the exclude-single-paper-authors filter look broken: the
   code was fine, the served process wasn't.)
2. **No reaping.** The launcher never killed its sidecar, so every launch left the
   previous one running. They piled up — 34+ orphaned `linxiv-api` processes over a
   day of rebuilds — the oldest squatting on `:8000`.

A `PID` in the health response is **not** a usable identity check: the `--onefile`
bootloader forks, so the Python `os.getpid()` differs from the PID the launcher
spawned.

## Decision

Three independent mechanisms, one per failure mode.

### 1. Per-launch token — never adopt a stale/foreign process

The launcher generates a nonce (`make_health_token`: launcher PID + nanosecond
timestamp), passes it to the backend via `LINXIV_HEALTH_TOKEN`, and `/api/health`
echoes it back. `wait_for_api` accepts the process only if `service ==
"linxiv-api"` **and** the echoed token matches this launch's. A stale build
carries a different (or empty) token and is rejected, so the launcher keeps its
own process instead of adopting old code. Unlike a PID, the token survives the
onefile fork; the threat model is our own stale processes, not an adversary
forging tokens. With `find_free_port` binding a fresh port each launch, a `:8000`
squatter never collides in the first place.

### 2. Reap on exit

The child is held in Tauri managed state (`ApiProcessState`) and signalled from
`reap_api` on `RunEvent::ExitRequested`/`Exit`. We use `SIGTERM`, not `SIGKILL`,
for a clean shutdown:

- **Release:** re-validate the PID against `/proc` (it may have died and the PID
  been recycled), then SIGTERM both the bootloader and its forked Python child
  (`signal_sidecar_children`). The child holds the port, so reaping must not depend
  on the bootloader forwarding the signal.
- **Dev:** the child leads its own process group (`process_group(0)`), so signalling
  the group exits `uv` → `python` → uvicorn together.

### 3. Startup orphan sweep

A SIGKILL of the launcher itself (e.g. the toolchain killing it on rebuild) fires
no exit event, so the next launch runs `sweep_orphaned_sidecars`: it scans `/proc`
for `linxiv-api` processes and SIGTERMs any **not owned by a live launcher**.

Ownership is decided by `owned_by_live_launcher`, which walks the parent chain
(through bootloader links, which share argv[0] `linxiv-api`) for a live ancestor
that is our launcher. We deliberately do **not** gate on `PPID == 1`: under a
`systemd --user` session the launcher is a child subreaper, so an orphaned sidecar
re-parents to `systemd --user` rather than init, and `PPID == 1` would never catch
it. The ancestor is matched on its **resolved executable** (`/proc/<pid>/exe`
basename, with the kernel's `" (deleted)"` suffix stripped) against the running
launcher's own — not on argv[0]. So a wrapper that rewrites argv[0], or a
self-update that unlinks the on-disk binary, can't trick us into killing a
concurrent instance's live API.

## Consequences

### Positive

- Rebuilds always reflect new code: the launcher only ever talks to the process it
  just spawned.
- Normal launch/quit leaves no orphan; strays from prior SIGKILLs are cleaned on
  the next start.
- A second concurrent instance's healthy sidecar is never reaped, even without a
  single-instance plugin, because the ownership walk reaches its live launcher.

### Negative / limits

- **Dev orphans are not swept.** The sweep keys on argv[0] `linxiv-api`; the dev API
  runs as `uv`/`python`. Dev strays from a SIGKILLed launcher are cleaned only by
  the graceful-exit process-group reap. They are harmless (the token check stops
  them being adopted) but can accumulate across hard-killed dev rebuilds.
- **Exit reap is SIGTERM-only**, with no SIGKILL escalation or `waitpid`. A child
  that ignores SIGTERM survives until the next startup sweep — acceptable, since
  uvicorn handles SIGTERM and blocking app exit to escalate is worse UX.
- **Unix/`/proc`-only.** The sweep and tree-aware reap rely on `/proc`; elsewhere
  they degrade to a no-op / best-effort `kill`. The reported pile-up was on Linux.
- `wait_for_api` blocks the setup thread on the failure path (pre-existing); not
  addressed here.

## References

- `src-tauri/src/main.rs` — `make_health_token`, `wait_for_api`, `reap_api`,
  `sweep_orphaned_sidecars`, `owned_by_live_launcher`, `proc_exe_basename`,
  `proc_argv0_basename`, `proc_ppid`, `signal_sidecar_children`
- `api/app.py` — `/api/health` (`token` echo)
- `src-tauri/Cargo.toml` — `libc` (unix) for `kill`/process-group signalling
- ADR 0014 — `LINXIV_DATA_DIR` (the env channel the launcher also uses)
