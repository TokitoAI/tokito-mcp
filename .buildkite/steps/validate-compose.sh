#!/usr/bin/env bash
# Production control-plane overlay parses.
#
# Runs on the host rather than through run-in-image.sh: it drives `docker
# compose` itself, and the CI image has no Docker client in it. The agent's
# rootless daemon is the one doing the work either way.
set -euo pipefail
just validate-compose
