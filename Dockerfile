# Headless linXiv node: `linxiv-headless` (full /api/* over HTTP incl. share
# routes + the iroh peer + background sync) with `linxiv-cli` alongside for
# exec-style queries. Both live in the Tauri-free linxiv-server/cli crates,
# so no webkit/gtk anywhere in this image.
#
# The image binds 0.0.0.0 inside the container, so the bin fails closed:
# LINXIV_API_TOKEN must be set and every request needs
# `Authorization: Bearer <token>` (see live-linxiv/docker-compose.yml).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY scripts/ scripts/
COPY src-tauri/ src-tauri/
# Runtime dlopen'd PDF library, copied into the final stage below.
RUN bash scripts/fetch_pdfium.sh
RUN cargo build --release --locked --manifest-path src-tauri/Cargo.toml \
    -p linxiv-server -p linxiv-cli --bin linxiv-headless --bin linxiv-cli

FROM debian:bookworm-slim
# curl: compose healthcheck probes /api/papers.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
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
