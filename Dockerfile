# tokito-mcp-server — production image.
#
# Build:
#   docker build -t tokito-mcp .                       # uses local symbols.sqlite if present
#   docker build --build-arg SYMBOLS_SHA=<blake3> \    # for CI: verifies the artifact
#                -t tokito-mcp .
#
# Run:
#   docker run -p 8090:8090 -e TOKITO_MCP_ADDR=0.0.0.0:8090 tokito-mcp
#
# The image bakes in `symbols.sqlite` so a `docker run` is enough to come up
# — no separate volume mount needed.

# ---- builder ----
FROM rust:1.88-slim-bookworm AS builder

# rusqlite's `bundled` feature compiles SQLite from source; needs a C toolchain.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work

# Copy manifests first so dependency build caches reuse cleanly between source
# changes. (No cargo-chef — we keep the Dockerfile dependency-free.)
COPY Cargo.toml Cargo.lock ./
COPY crates/symbols/Cargo.toml crates/symbols/
COPY crates/pack/Cargo.toml    crates/pack/
COPY crates/server/Cargo.toml  crates/server/

# Stub sources so `cargo build` resolves the workspace + caches deps.
RUN mkdir -p crates/symbols/src crates/pack/src crates/server/src \
    && echo 'fn main(){}' > crates/pack/src/main.rs \
    && echo 'fn main(){}' > crates/server/src/main.rs \
    && echo '' > crates/symbols/src/lib.rs \
    && echo '' > crates/server/src/lib.rs \
    && cargo build --release -p tokito-mcp-server || true

# Real sources.
COPY crates ./crates
RUN touch crates/server/src/main.rs crates/symbols/src/lib.rs
RUN cargo build --release -p tokito-mcp-server

# ---- runtime ----
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 tokito

WORKDIR /opt/tokito

# Server binary.
COPY --from=builder /work/target/release/tokito-mcp-server /usr/local/bin/tokito-mcp-server

# Symbol artifact baked into the image. Build context must contain it — the
# release workflow copies the artifact built by tokito-mcp-pack into the build
# context before invoking `docker build`.
COPY symbols.sqlite /opt/tokito/symbols.sqlite

USER tokito

ENV TOKITO_MCP_DB=/opt/tokito/symbols.sqlite \
    TOKITO_MCP_ADDR=0.0.0.0:8090 \
    RUST_LOG=tokito_mcp_server=info,tower_http=info

EXPOSE 8090

HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD wget --quiet --tries=1 --spider http://localhost:8090/v1/health || exit 1

ENTRYPOINT ["/usr/local/bin/tokito-mcp-server"]
