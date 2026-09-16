# tokito-mcp build recipes.
#
# CI calls these; it does not inline build logic of its own. That keeps the
# pipeline definition a thin caller and makes the Buildkite migration mechanical
# rather than interpretive.

export CARGO_TERM_COLOR := "always"
export RUST_BACKTRACE := "1"

_default:
    @just --list --unsorted

# `check` is deliberately not part of `ci`: cargo check and cargo clippy keep
# separate build fingerprints, so running both type-checks the workspace twice,
# and `clippy --all-targets` already covers everything check does.
# Everything CI runs, minus the Docker image smoke test.
ci: fmt-check clippy test version deny

# The fast inner loop: what you want before pushing.
pre-push: fmt-check clippy test

# ---------------------------------------------------------------- core gates

# Formatting is clean.
fmt-check:
    cargo fmt --all -- --check

# Formats in place. Not part of any CI aggregate.
fmt:
    cargo fmt --all

# `--locked` and `-D warnings` were both missing from the workflow. Without
# `--locked`, clippy quietly updates Cargo.lock and the `cargo test` that
# follows then passes against the rewritten lock, so a dependency drift lands
# with nothing failing. The workflow leaned on a global `RUSTFLAGS: -D warnings`
# instead, which also forced a separate rebuild of every job that set it.
# Lints, denying warnings.
clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings

# nextest over `cargo test`: it gives the retry/timeout profile the other repos
# use, and reports a failure by name.
# Workspace test suite.
test:
    cargo nextest run --workspace --locked

# The package version and the version the server advertises must agree.
version:
    python3 scripts/check-version.py

# Runs the CLI directly rather than EmbarkStudios/cargo-deny-action, which is a
# GitHub-only container action and has no Buildkite equivalent.
# Licence and advisory check.
deny:
    cargo deny check

# Lockfile and workspace resolve without changes. Fast local inner loop only.
check:
    cargo check --locked --workspace

# ------------------------------------------------------------ deploy gates

# Production control-plane overlay parses.
validate-compose:
    docker compose \
        --env-file deploy/production/.env.example \
        -f deploy/production/compose.yml \
        -f deploy/production/compose.generated.yml \
        config --quiet
