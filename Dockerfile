# Headless linXiv node: `linxiv-headless` (full /api/* over HTTP incl. share
# routes + the iroh peer + background sync) with `linxiv-cli` alongside for
# exec-style queries. No Tauri window is ever opened, but the headless bin
# links the app lib, which links tauri — hence the webkit/gtk packages.
# ponytail: image carries webkit it never renders with; extract route/state
# into a tauri-free crate if image size ever matters.
#
# The image binds 0.0.0.0 inside the container, so the bin fails closed:
# LINXIV_API_TOKEN must be set and every request needs
# `Authorization: Bearer <token>` (see live-linxiv/docker-compose.yml).
FROM rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
       libayatana-appindicator3-dev librsvg2-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY scripts/ scripts/
COPY src-tauri/ src-tauri/
RUN bash scripts/fetch_pdfium.sh
# Builds + stages the cli/mcp sidecars tauri-build's externalBin check needs.
RUN bash scripts/stage_rust_bins.sh
RUN cargo build --release --locked --manifest-path src-tauri/Cargo.toml --bin linxiv-headless

FROM debian:bookworm-slim
# curl: compose healthcheck probes /api/papers.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl libwebkit2gtk-4.1-0 libxdo3 libayatana-appindicator3-1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/src-tauri/target/release/linxiv-headless \
                  /src/src-tauri/target/release/linxiv-cli /usr/local/bin/
COPY --from=build /src/src-tauri/vendor/pdfium/lib/libpdfium.so /usr/local/lib/pdfium/libpdfium.so
ENV LINXIV_PDFIUM_LIB=/usr/local/lib/pdfium/libpdfium.so \
    LINXIV_DATA_DIR=/data \
    LINXIV_HTTP_ADDR=0.0.0.0:8000
VOLUME /data
EXPOSE 8000
CMD ["linxiv-headless"]
