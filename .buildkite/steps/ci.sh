#!/usr/bin/env bash
# fmt, clippy, tests, version agreement and cargo-deny, inside the pinned image.
set -euo pipefail
exec "$(dirname "$0")/run-in-image.sh" ci
