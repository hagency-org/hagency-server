# syntax=docker/dockerfile:1.7
# Linux release build of the Hagency server stack and the `hagency` CLI.
# Build context: the hagency workspace root (parent of hagency-server/ and
# hagency-client/), because the server needs the plain-file Palpo copy under
# hagency-server/.run/ and the CLI lives in a sibling repository.
#   docker buildx build -f hagency-server/deploy/linux/Dockerfile --target server ...
# Palpo and Pasion are libraries embedded in the hagency-server binary
# (Cargo.toml [patch] -> .run/palpo-8c1759f; pasion-backend is a git dependency),
# so the server image runs ONE process; there is no separate palpo/pasion binary.
ARG SERVER_RUST=1.99.0
ARG CLIENT_RUST=1.98.0
ARG BUILD_JOBS=6

FROM rust:${SERVER_RUST}-bookworm AS server-build
ARG BUILD_JOBS
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends libpq-dev cmake clang \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/hagency-server
COPY hagency-server/rust-toolchain.toml hagency-server/Cargo.toml hagency-server/Cargo.lock ./
COPY hagency-server/crates ./crates
COPY hagency-server/xtask ./xtask
COPY hagency-server/.run/palpo-8c1759f ./.run/palpo-8c1759f
# Cache mounts keep the registry and target dir across rebuilds (incremental).
RUN --mount=type=cache,id=hl-srv-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=hl-srv-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=hl-srv-target-${TARGETARCH},target=/src/hagency-server/target \
    cargo build --release --locked --jobs ${BUILD_JOBS} -p hagency-server --bin hagency-server \
    && install -D target/release/hagency-server /out/hagency-server

FROM rust:${CLIENT_RUST}-bookworm AS cli-build
ARG BUILD_JOBS
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/hagency-client
COPY hagency-client/rust-toolchain.toml hagency-client/Cargo.toml hagency-client/Cargo.lock ./
COPY hagency-client/native ./native
COPY hagency-client/docs ./docs
RUN --mount=type=cache,id=hl-cli-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=hl-cli-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=hl-cli-target-${TARGETARCH},target=/src/hagency-client/target \
    cargo build --release --locked --jobs ${BUILD_JOBS} -p hagency \
    && install -D target/release/hagency /out/hagency

FROM debian:bookworm-slim AS server
RUN apt-get update && apt-get install -y --no-install-recommends libpq5 ca-certificates gosu curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home hagency && mkdir -p /app/data && chmod 700 /app/data \
    && chown hagency:hagency /app/data
WORKDIR /app
COPY --from=server-build /out/hagency-server /usr/local/bin/hagency-server
# Frontend (Dioxus WASM) and Pasion assets are architecture-independent; they are
# taken prebuilt from the host (xtask prepare-frontend / prepare-pasion). The
# upstream hagency-server/Dockerfile shows the in-Docker build if reproducibility
# from a clean checkout is required.
COPY hagency-server/resources/pasion /app/resources/pasion
COPY hagency-server/resources/frontend/public /app/resources/frontend/public
COPY hagency-server/scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
EXPOSE 8088
HEALTHCHECK --interval=5s --timeout=3s --start-period=90s --retries=6 \
    CMD curl --fail --silent --max-time 2 http://127.0.0.1:8088/readyz || exit 1
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["--config", "/app/config/hagency.toml"]

# Headless `hagency` CLI for server-hosted agents and the concierge. Vendor
# agent runtimes (Claude Code, Codex) are NOT installed here; see README.md.
FROM debian:bookworm-slim AS cli
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git curl \
    && rm -rf /var/lib/apt/lists/* && useradd --uid 10002 --create-home agent
COPY --from=cli-build /out/hagency /usr/local/bin/hagency
USER agent
ENTRYPOINT ["/usr/local/bin/hagency"]
CMD ["--help"]
