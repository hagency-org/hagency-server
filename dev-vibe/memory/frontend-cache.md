# Frontend cache and deployment

2026-10-13: user requested cache/compression, plan and review before implementation, remote release and production update. Plan: D:/Works/hagency-org/hagency-cloud-deploy-prod/static-cache-plan.zh.md.

Original GitHub checkout is this repository, remote hagency-org/hagency-server; updated main from 710a87c to 36034d4 before modifications. No deployment compilation on operator or production host. GitHub CI is the only Server image producer; production pulls fixed ghcr.io/hagency-org/hagency-server digests and verifies ARM64 and OCI labels.

Frontend files: hashed dxh 16-hex bundles immutable one year, entry and fixed names no-cache with representation SHA256 ETag, config/errors no-store. Precompressed Brotli from Dioxus and deterministic gzip from xtask. GET/HEAD, q values, identity/406, If-None-Match/304 supported. Outermost Service hoop preserves Accept-Encoding in Vary after embedded Matrix CORS.

Startup archives only generated hashed files under configured data_dir/frontend-assets/assets; a missing current hash can be served from this confined directory. Does not archive entry/config/private files. Same-name changed bytes fail startup. Deployment seeds previous image's public hashed files before first upgrade. Archive is persistent tenant data (production /opt/hagency/hagency-data/<UUID>/data/frontend-assets), included in existing data backup. No automatic garbage collection; retain needed generations and document manual cleanup only during maintenance. Auth, Matrix, business logic and database schema unchanged.

Regression tests run in GitHub CI, no local compilation. Deployment result to be appended after remote checks and production validation.
