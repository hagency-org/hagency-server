# GitHub CI and container delivery

`.github/workflows/ci.yml` runs on main pushes, PRs, version tags and manual dispatch.

Quality gates use Rust 1.99.0: rustfmt, strict Clippy for backend/Agent service/xtask,
backend and development-tool tests, Agent tests including the normally ignored
PostgreSQL cases (a disposable PostgreSQL 17 service, serialized fixtures),
OpenAPI drift guard and its self-tests, and a wasm32 frontend compile check.
CI overrides the local absolute Cargo target directory. A version tag must equal
`v` plus `workspace.package.version` in Cargo.toml, including any prerelease suffix.

After quality succeeds, native amd64 and arm64 runners build the complete existing
Dockerfile, including both browser applications. Each image is smoke-tested for
executable startup (`--help`) and packaged browser assets. This is not a running
Matrix/Pasion/Appservice deployment or a `/readyz` integration test.

PR builds do not push. Main/tag/manual builds push by digest to
`ghcr.io/<repository-owner>/hagency-server`, with OCI source/revision labels,
SBOM and provenance. The final job publishes tags only after **both** smoke tests
succeed and verifies that the manifest contains both Linux architectures:

- `edge`: main branch builds.
- `sha-<short-commit>`: immutable source identification (reruns can rebuild it).
- `0.1.0`, `0.1`: example stable version tags for a `v0.1.0` source tag.
- Prerelease versions retain their full suffix; they do not update stable aliases.
- Docker metadata's stable semver policy also publishes `latest` for stable releases.

The workflow uses the repository's GITHUB_TOKEN with `packages: write`; no Docker
Hub credentials are required. The organization must permit Actions and package
publication. Check initial GHCR package visibility/access before directing public
users to it. No workflow performs deployment or changes existing databases.

To ship: first merge a passing main build, update workspace version/Cargo.lock,
then push the matching version tag. Verify the Actions run and manifest before
using the published image. For Docker Hub distribution, configure a separate
registry credential and destination explicitly.

Dependabot proposes monthly GitHub Actions updates. It does not auto-merge them.

## Verification checkpoint (2026-10-10)

Workflow syntax/actionlint passed. OpenAPI validates 52 owner operations and 4
Appservice operations, with 10 guard tests passing. The 5 xtask tests passed.
Publication check: rustfmt passes after formatting reply-policy changes and
re-reviewing the router fingerprint. Agent-service unit tests pass (15 tests);
43 PostgreSQL-dependent tests remain ignored locally and are enabled by CI.
The new GitHub workflow has not yet been pushed/run. No multi-architecture image
or live integration qualification is claimed by these local checks.
