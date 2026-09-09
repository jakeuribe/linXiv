# Running the headless server

`linxiv-headless` is the full linXiv backend without a window: the complete
`/api/*` router over HTTP — share routes included — plus the iroh peer and
the background sync loop. It lives in the Tauri-free `linxiv-server` crate,
so it builds and runs without webkit/gtk. Same dispatch surface as the
desktop app, same on-disk data layout, so it shares a library with the app,
CLI, and MCP server when pointed at the same data dir.

## Run from source

```bash
# once: the PDF routes dlopen libpdfium at runtime
bash scripts/fetch_pdfium.sh

cd src-tauri
cargo run -p linxiv-server --bin linxiv-headless
```

Defaults bind `127.0.0.1:8000` with no auth — loopback stays open for the
local dev loop. Verify with:

```bash
curl http://127.0.0.1:8000/api/status
```

## Run in a container

The repo's `Dockerfile` builds `linxiv-headless` (with `linxiv-cli`
alongside for exec-style queries) into a slim Debian image:

```bash
podman build -t linxiv-headless:dev .
podman run -d --name linxiv \
  -e LINXIV_API_TOKEN="$(openssl rand -hex 32)" \
  -e LINXIV_P2P_PASSPHRASE=change-me \
  -p 127.0.0.1:8000:8000 \
  -v linxiv-data:/data \
  linxiv-headless:dev
```

The image binds `0.0.0.0:8000` inside the container, and the bin fails
closed: a non-loopback `LINXIV_HTTP_ADDR` without `LINXIV_API_TOKEN`
refuses to start. Every request then needs
`Authorization: Bearer <token>`:

```bash
curl -H "Authorization: Bearer $LINXIV_API_TOKEN" http://127.0.0.1:8000/api/status
```

Or use the compose file next to this doc, which builds the image and
wires the token, volume, and healthcheck in one step:

```bash
LINXIV_API_TOKEN=$(openssl rand -hex 32) \
LINXIV_P2P_PASSPHRASE=change-me \
docker compose -f docs/headless/docker-compose.yml up -d --build
```

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `LINXIV_HTTP_ADDR` | `127.0.0.1:8000` | Listen address. Non-loopback requires the token. |
| `LINXIV_API_TOKEN` | unset | Bearer token gating every request; required off loopback. |
| `LINXIV_DATA_DIR` | per-user app dir | Database, PDFs, and vault location (shared with app/CLI/MCP). |
| `LINXIV_P2P_PASSPHRASE` | unset | At-rest encryption for the p2p key store where no OS keychain exists (containers). |
| `LINXIV_PDFIUM_LIB` | vendor path | Explicit `libpdfium` path; otherwise `scripts/fetch_pdfium.sh`'s output is found. |
| `LINXIV_PDF_RATE_BPS` | ~5 MB/s | Per-member PDF byte-lane rate for Remote Query Mode. |
| `LINXIV_ALLOW_SLEEP` | unset | `1` opts out of the systemd sleep/idle inhibit the node takes while running. |

## Admin and relay

- `GET /admin` serves a static admin page (secretless — served without
  auth; every API call it makes carries the bearer token). It manages the
  Remote Query Mode Member List (`/api/admin/relay/members`), shows the
  relay access log and PDF transfer log, and mints the copyable Node
  Address once a relay is configured.
- `GET /api/status` is the one-call health/config aggregate (also the
  container healthcheck target).
- Relay settings are the same on-disk user settings as the app
  (`p2p_relay_url` / `p2p_relay_auth_token` / `p2p_relay_only`): set them
  via `PATCH /api/settings`, then `POST /api/share/relay/reconnect` to
  rebind without a restart.
