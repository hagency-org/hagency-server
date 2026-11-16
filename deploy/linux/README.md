# Linux build and smoke stack

Run everything from the hagency workspace root (parent of `hagency-server/` and
`hagency-client/`); that directory is the Docker build context.

```bash
# Server image (hagency-server with embedded Palpo + Pasion), native arm64 (Graviton)
docker buildx build --platform linux/arm64 -f hagency-server/deploy/linux/Dockerfile \
  --target server -t hagency-linux/server:arm64 --load .
# hagency CLI image
docker buildx build --platform linux/arm64 -f hagency-server/deploy/linux/Dockerfile \
  --target cli -t hagency-linux/cli:arm64 --load .
# amd64: same commands with --platform linux/amd64 (QEMU on Apple silicon: slow;
# build on an x86_64 CI runner instead).

docker compose -f hagency-server/deploy/linux/compose.yaml up -d   # 127.0.0.1:18090
curl -s http://127.0.0.1:18090/_matrix/client/versions
curl -s http://127.0.0.1:18090/readyz
docker compose -f hagency-server/deploy/linux/compose.yaml down -v
```

## Components

- `hagency-server`: one process. Palpo (homeserver) and Pasion (accounts/OAuth)
  are linked in as libraries; Palpo comes from the plain-file copy in
  `hagency-server/.run/palpo-8c1759f` (Cargo `[patch]`), Pasion from its git dependency.
- Postgres: three databases (`hagency`, `palpo`, `pasion`), see `../databases.sql`.
- Frontend and Pasion web assets are copied prebuilt from `hagency-server/resources/`
  (architecture-independent WASM/JS). Run `xtask prepare-frontend` / `prepare-pasion`
  first on a clean checkout.
- Not included: Element Web (static container, e.g. `vectorim/element-web`) and a TLS
  proxy (ALB or Caddy). Federation needs `https://<server_name>`.

## hagency CLI runtime requirements (hosted agents)

The `cli` image contains only `hagency`, git and CA certificates. A server-hosted agent
also needs, installed by the operator at deploy time and never baked with credentials:

- Claude Code CLI (`claude`, Node.js 20+) and/or Codex CLI (`codex`) on `PATH`.
- Their API keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) injected at runtime from
  AWS Secrets Manager / SSM, not from image layers or files in the repo.
- A writable state dir (`$XDG_DATA_HOME/hagency`) on a persistent volume.
