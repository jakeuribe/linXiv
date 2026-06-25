# ADR 0019: CLI and MCP ship bundled in the app only, unsigned

## Status

Accepted

## Context

linXiv exposes three backend surfaces: the desktop app (GUI), a CLI, and an MCP
server. Today all three ship as PyInstaller sidecars bundled inside the Tauri
app; the planned Rust port (`docs/rust-port-plan.md`) keeps the same three-binary
shape — `linxiv-app` + `linxiv-cli` + `linxiv-mcp`.

The open question (plan D27) was whether the CLI and MCP should *also* be
distributed standalone — Homebrew, winget, or GitHub Release tarballs — for users
who want them without the GUI.

Convenient standalone distribution is gated on **code signing + notarization**:
unsigned binaries trip macOS Gatekeeper ("unidentified developer") and Windows
SmartScreen. Signing is not free — it needs an Apple Developer account, a
notarization step in CI, and certificate management. There is no convenient way
to ship a standalone CLI/MCP that "just runs" without solving signing first.

## Decision

1. Ship `linxiv-cli` and `linxiv-mcp` **bundled inside the Tauri app only**
   (Tauri `externalBin`), **unsigned**.
2. Do **not** publish a standalone CLI/MCP channel (no Homebrew/winget/standalone
   tarballs) for now.
3. Power users who want the CLI/MCP standalone **build from source** and self-sign
   if their OS requires it.
4. Revisit only if/when signing+notarization is adopted for the app anyway — at
   that point standalone distribution becomes cheap to add.

`integrations.rs` is unaffected by this choice; only `resolve_install_sidecar`
retargets the bundled binary names. Applies to both the current Python sidecars
and the future Rust binaries.

## Consequences

### Positive

- No signing infrastructure, cost, or CI complexity now.
- One distribution channel (the app bundle) to maintain.
- The build-from-source escape hatch covers the power-user case without us
  shipping or signing anything.

### Negative / limits

- No `brew install` / `winget` convenience; programmatic users either install the
  app or build from source.
- Self-signing is on the user.
- If demand for standalone binaries grows, this revisits — it is the lazy default
  and reversible.

## References

- `docs/rust-port-plan.md` — D27 (distribution), D26 (packaging)
- `src-tauri/src/integrations.rs` — `resolve_install_sidecar`
