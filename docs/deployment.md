# Production deployment

Production is `https://mcp.tokito.dev/mcp`. It runs the released GHCR image on
the Tokito VPS and is exposed by a token-managed Cloudflare Tunnel. TLS
terminates at Cloudflare; the server and tunnel share only their private Compose
network. No host port is required.

The private infrastructure repository remains the authority for VPS inventory
and access. `deploy/production/compose.yml` is the public, reproducible service
definition and contains no credentials.

## Required configuration

On the VPS, keep the deployment in `/opt/tokito-mcp`:

```text
/opt/tokito-mcp/
  compose.yml
  .env
```

Copy `deploy/production/compose.yml` and `.env.example`, then set:

- `TOKITO_MCP_IMAGE`: an exact release tag such as
  `ghcr.io/tokitoai/tokito-mcp:v0.1.2`. Do not deploy `latest`.
- `TOKITO_MCP_ALLOWED_HOSTS`: the public Host values the server accepts, such
  as `mcp.tokito.dev`.
- `CLOUDFLARED_IMAGE`: the operator-approved cloudflared version **and digest**,
  for example `cloudflare/cloudflared:<version>@sha256:<digest>`.
- `CLOUDFLARE_TUNNEL_TOKEN`: the tunnel token from Cloudflare Zero Trust. This
  is the only production secret in this stack.
- `TOKITO_MCP_MAX_SESSIONS` and `RUST_LOG`: optional operational tuning.

Set `.env` to mode `0600`; never commit it. The Cloudflare remotely managed
tunnel must route hostname `mcp.tokito.dev` to `http://server:8090`. The server
allows that public `Host` through `TOKITO_MCP_ALLOWED_HOSTS`; do not rewrite it
to `localhost`.

GHCR images are public, so the VPS needs no registry credential. If package
visibility changes, use a read-only package token with `docker login ghcr.io`
and keep it outside this repository.

## Deploy a release

Run from `/opt/tokito-mcp`:

```bash
set -euo pipefail
chmod 600 .env
docker compose config --quiet
docker compose pull
docker image inspect "$(awk -F= '/^TOKITO_MCP_IMAGE=/{print $2}' .env)" \
  --format '{{index .RepoDigests 0}}'
docker compose up -d --remove-orphans
docker compose ps
```

Record the resolved server digest in the deployment log. Wait until `server`
is `healthy`; if it is not, stop and inspect:

```bash
docker compose ps
docker compose logs --tail=200 server cloudflared
docker inspect "$(docker compose ps -q server)" \
  --format '{{json .State.Health}}'
```

The image healthcheck calls the installed `curl` binary against
`http://localhost:8090/v1/health`. Cloudflared starts only after this check
passes.

## Post-deploy verification

From a machine outside the VPS/network, verify DNS, edge TLS, REST health, MCP
initialization, the advertised server version, and the tool catalog:

```bash
TOKITO_MCP_EXPECTED_VERSION=0.1.2 \
  bash scripts/protocol-smoke.sh https://mcp.tokito.dev/mcp
```

The smoke test is read-only and needs no production credential. For a deeper
catalog check, run `TOKITO_MCP_URL=https://mcp.tokito.dev ./scripts/smoke.sh`.

Do not call a deployment complete until both the container healthcheck and the
external protocol smoke test pass.

## Rollback

Keep the previous release tag and digest in the deployment log. To roll back:

1. Set `TOKITO_MCP_IMAGE` in `.env` to the previous `vX.Y.Z` tag (or, during an
   incident, its recorded digest).
2. Run `docker compose pull server`.
3. Run `docker compose up -d --no-deps server`.
4. Wait for `server` to become healthy.
5. Run the external protocol smoke test with
   `TOKITO_MCP_EXPECTED_VERSION=<previous version>`.

The catalog is baked into each image and the server is read-only, so rollback
does not require a database migration or volume restore.

## Session lifecycle contract

MCP sessions are ephemeral. A disconnected or otherwise abandoned session is
retained for at most **60 seconds** and is then reaped. After expiry, a request
using the old `mcp-session-id` can be rejected as an unknown session.

Clients must treat that response, connection loss, or a server restart as a
signal to send a fresh MCP `initialize` request, store the new
`mcp-session-id`, send `notifications/initialized`, and retry only operations
that are safe to repeat. Clients must not assume session IDs survive a deploy
or remain valid indefinitely.
