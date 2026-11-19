# GitHub CI and container delivery

`.github/workflows/ci.yml` runs on main pushes, PRs, version tags and manual dispatch.

Quality gates use Rust 1.99.0: rustfmt, strict Clippy for backend/Agent service/xtask,
backend and development-tool tests, Agent tests including the normally ignored
PostgreSQL cases (four test processes sharing a PostgreSQL 17 service, each with
its own Agent and usage databases and serial fixtures),
OpenAPI drift guard and its self-tests, and a wasm32 frontend compile check.
The Appservice identity upgrade runs in the regular backend test build against a
dedicated empty database. A version tag must equal
`v` plus `workspace.package.version` in Cargo.toml, including any prerelease suffix.

In this workflow, PRs and manual dispatch run quality checks only; its publishing
container jobs are skipped. The independent `Embedded Deployment` workflow
builds a local amd64 image on code and configuration PRs and runs real PKCE, AS
delivery, worker
health, SIGTERM drain and restart checks without publishing it. PR/weekly/manual
backup drills also restore three new databases and private files from the same
image's executable and assets. PRs limited to Markdown in docs or dev-vibe, or
the two top-level READMEs, run spelling and dependency policy and skip the
Rust and deployment jobs. Changes to test scripts or workflows run the full checks.
The `Dependency risk policy` workflow audits
PRs/main and weekly, retaining full findings and rejecting unreviewed, changed,
expired or stale risk dispositions; see [dependency security](DEPENDENCY_SECURITY.md).
Only a push to main or a `v*` version tag can publish image digests, after quality
succeeds. Native amd64 and arm64 runners build the complete existing
Dockerfile, including both browser applications. Both architectures check executable
startup (`--help`) and packaged browser assets. The amd64 runner also executes the Matrix/Pasion/Appservice
deployment smoke on its published candidate digest before exporting it for the manifest.

Main/tag image builds push by digest to
`ghcr.io/<repository-owner>/hagency-server`, with OCI source/revision labels,
SBOM and provenance. The final job publishes tags only after **both** smoke tests
succeed and verifies that the manifest contains both Linux architectures:

- `latest`, `edge`: main branch builds, published after both architectures pass.
- `sha-<short-commit>`: immutable source identification (reruns can rebuild it).
- `0.1.0`, `0.1`: example stable version tags for a `v0.1.0` source tag.
- Prerelease versions retain their full suffix; they do not update stable aliases.
- Stable version releases also update `latest`; prereleases do not.

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
The workflow has been pushed and GitHub runs are tracked separately. No multi-architecture image
or live integration qualification is claimed by these local checks.

Trigger correction: PRs (including Dependabot PRs) and manual dispatch cannot
start the container matrix or manifest publication. Both jobs explicitly require
a push to main or a version tag.
