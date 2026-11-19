# syntax=docker/dockerfile:1
# Cargo resolves the whole workspace even for a single package. Supply just its
# manifests and target paths until a stage needs the corresponding source.
FROM rust:1.99-bookworm AS workspace
WORKDIR /workspace
COPY --parents Cargo.toml Cargo.lock crates/*/Cargo.toml xtask/Cargo.toml ./
RUN set -eu; \
    for package in crates/* xtask; do mkdir -p "$package/src"; done; \
    touch crates/backend/src/lib.rs crates/agent-service/src/lib.rs crates/db-runtime/src/lib.rs; \
    for bin in crates/backend crates/frontend xtask; do echo 'fn main() {}' > "$bin/src/main.rs"; done; \
    mkdir -p crates/db-runtime/tests && touch crates/db-runtime/tests/postgres.rs

FROM rust:1.99-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends libpq-dev cmake clang && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY --from=workspace /workspace/ ./
ARG SERVER_BUILD_JOBS=1
# Store the dependency artifacts and their source paths in an exportable image
# layer. Remove every locally compiled placeholder before copying real inputs.
RUN cargo build --release --locked --jobs "$SERVER_BUILD_JOBS" --bin hagency-server \
    && cargo clean --release -p hagency-server -p hagency-agent-service -p hagency-db-runtime
COPY crates/backend ./crates/backend
COPY crates/agent-service ./crates/agent-service
COPY crates/db-runtime ./crates/db-runtime
RUN cargo build --release --locked --jobs "$SERVER_BUILD_JOBS" --bin hagency-server \
    && cp target/release/hagency-server /build/hagency-server \
    && cargo clean --release -p hagency-server -p hagency-agent-service -p hagency-db-runtime

# Build the Dioxus WASM app with the same pinned Pasion source as the backend.
FROM rust:1.99-trixie AS web-tools
ARG TARGETARCH
# Keep simultaneous backend/WASM compilation bounded in small builders.
ENV CARGO_BUILD_JOBS=1
RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
RUN case "$TARGETARCH" in arm64) dx_arch=aarch64 ;; amd64) dx_arch=x86_64 ;; *) exit 1 ;; esac \
    && curl -fsSL "https://github.com/DioxusLabs/dioxus/releases/download/v0.7.5/dx-$dx_arch-unknown-linux-gnu.tar.gz" -o /tmp/dx.tar.gz \
    && curl -fsSL "https://github.com/DioxusLabs/dioxus/releases/download/v0.7.5/dx-$dx_arch-unknown-linux-gnu.sha256" -o /tmp/dx.sha256 \
    && sed -n '1s/  .*/  \/tmp\/dx.tar.gz/p' /tmp/dx.sha256 | sha256sum -c - \
    && tar -xzf /tmp/dx.tar.gz -C /usr/local/bin && rm /tmp/dx.tar.gz /tmp/dx.sha256
WORKDIR /build
COPY --from=workspace /workspace/ ./
COPY xtask ./xtask
COPY crates/frontend/branding ./crates/frontend/branding

FROM web-tools AS frontend-assets
COPY crates/frontend ./crates/frontend
RUN --mount=type=cache,id=hagency-frontend-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=hagency-frontend-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=hagency-frontend-target,target=/build/.run/frontend-target \
    --mount=type=cache,id=hagency-xtask-target,target=/build/target \
    cargo run --quiet --locked --manifest-path xtask/Cargo.toml -- prepare-frontend --output /frontend-resources

FROM web-tools AS pasion-assets
RUN --mount=type=cache,id=hagency-pasion-wasm-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=hagency-pasion-wasm-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=hagency-pasion-wasm-target,target=/build/.run/pasion-source/target \
    --mount=type=cache,id=hagency-xtask-target,target=/build/target \
    cargo run --quiet --locked --manifest-path xtask/Cargo.toml -- \
    prepare-pasion --output /pasion-resources

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends libpq5 ca-certificates gosu curl && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home hagency && mkdir -p /app/data && chmod 700 /app/data && chown hagency:hagency /app/data
WORKDIR /app
ARG HAGENCY_SOURCE_FINGERPRINT=unspecified
LABEL org.hagency.protocol="owner-agent-v1" \
      org.hagency.source-fingerprint="$HAGENCY_SOURCE_FINGERPRINT"
COPY --from=build /build/hagency-server /usr/local/bin/hagency-server
COPY --from=pasion-assets /pasion-resources /app/resources/pasion
COPY --from=frontend-assets /frontend-resources /app/resources/frontend/public
COPY scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
EXPOSE 8088
# The worker check also requires the mandatory startup AS roundtrip.
HEALTHCHECK --interval=5s --timeout=3s --start-period=90s --retries=6 \
    CMD curl --fail --silent --max-time 2 http://127.0.0.1:8088/workers-healthz || exit 1
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["--config", "/app/config/hagency.toml"]
