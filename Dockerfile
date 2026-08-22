# tokito-mcp-server — production image.
#
# Build:
#   docker build -t tokito-mcp .                       # uses local symbols.sqlite if present
#   docker build --build-arg SYMBOLS_BLAKE3=<blake3> \ # verifies the baked artifact
#                -t tokito-mcp .
#
# Run:
#   docker run -p 8090:8090 -e TOKITO_MCP_ADDR=0.0.0.0:8090 tokito-mcp
#
# The image bakes in `symbols.sqlite` so a `docker run` is enough to come up
# — no separate volume mount needed.

# ---- builder ----
FROM rust:1.97-slim-bookworm@sha256:2775a09d208ff0d7c1f50490c45b62db929e87ba1dcbc3f2132ac71a704bcdd3 AS builder

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

# ---- catalog integrity gate ----
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS catalog-validator

RUN apt-get update && apt-get install -y --no-install-recommends b3sum \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /validation
COPY symbols.sqlite ./symbols.sqlite

# Release builds pass the hash emitted by tokito-mcp-pack's manifest. Keeping
# the argument optional preserves local developer builds while making a
# supplied integrity assertion enforceable rather than decorative.
ARG SYMBOLS_BLAKE3=""
RUN if [ -n "$SYMBOLS_BLAKE3" ]; then \
        printf '%s  %s\n' "$SYMBOLS_BLAKE3" symbols.sqlite | b3sum --check; \
    fi

# ---- runtime ----
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 tokito

WORKDIR /opt/tokito

# Server binary.
COPY --from=builder /work/target/release/tokito-mcp-server /usr/local/bin/tokito-mcp-server

# Symbol artifact baked into the image. Build context must contain it — the
# release workflow copies the artifact built by tokito-mcp-pack into the build
# context before invoking `docker build`. Copy only from the validation stage,
# so the runtime can never bypass a supplied digest check.
COPY --from=catalog-validator /validation/symbols.sqlite /opt/tokito/symbols.sqlite

USER tokito

ENV TOKITO_MCP_DB=/opt/tokito/symbols.sqlite \
    TOKITO_MCP_ADDR=0.0.0.0:8090 \
    RUST_LOG=tokito_mcp_server=info,tower_http=info

EXPOSE 8090

HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD curl --fail --silent --show-error --max-time 2 http://localhost:8090/v1/health || exit 1

ENTRYPOINT ["/usr/local/bin/tokito-mcp-server"]
