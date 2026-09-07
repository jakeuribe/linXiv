# Deliberately untyped leftovers — route/share.rs envelope pass

- `reconnect_relay` (`POST /api/share/relay/reconnect`) still returns
  `json!({"ok": true})`. No shared ok-receipt struct exists anywhere yet, and
  authors.rs / settings.rs / feed.rs / storage.rs / versions.rs (other agents'
  files) emit the same shape — unify once, in core, rather than minting a
  share-local duplicate.
- `share_sync::sync_share` envelope (`POST /api/share/{id}/sync`) lives in
  `share_sync.rs`, not the assigned route file; its `json!` is untouched.
  Hand-written TS twin: `syncShare` in `src/api/share.ts`.
- `SharedPaper::to_summary_value` (papers rows of
  `GET /api/share/received/{id}`) stays a `Value` fragment inside the typed
  `ReceivedDetail` envelope — it is already the documented one-home projection
  in linxiv-share, not an inline `json!` replaced wholesale.
