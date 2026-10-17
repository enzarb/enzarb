# syntax=docker/dockerfile:1.7
#
# Build from the monorepo root (in-repo path deps on proto/, operator/, connectors/):
#   docker build -f workflows/Dockerfile .
#
# One Cargo workspace at the root. Stages (cargo-chef): planner -> cook (dependency layer, cached) -> build.
FROM rust:1-bookworm AS chef
# cmake + clang: aws-lc-sys (via rustls/jsonwebtoken/reqwest).
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang \
 && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked --version ^0.1
WORKDIR /src

FROM chef AS planner
# Every workspace member is part of the recipe: the root workspace resolves as a whole. The recipe
# only changes when a manifest or Cargo.lock does, so the cook layer below survives source edits.
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY proto/crates proto/crates
COPY operator/crates operator/crates
COPY connectors/crates connectors/crates
COPY connectors/bin connectors/bin
COPY workflows/crates workflows/crates
COPY realtime/Cargo.toml realtime/Cargo.toml
COPY realtime/src realtime/src
COPY sandbox-container/Cargo.toml sandbox-container/Cargo.toml
COPY sandbox-container/src sandbox-container/src
COPY sandbox-proxy/Cargo.toml sandbox-proxy/Cargo.toml
COPY sandbox-proxy/src sandbox-proxy/src
COPY updater/Cargo.toml updater/Cargo.toml
COPY updater/src updater/src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY rust-toolchain.toml ./
COPY --from=planner /src/recipe.json recipe.json
# Dependency layer: every workspace member is a stub here, so nothing compiles against a partial crate.
RUN cargo chef cook --release --locked --recipe-path recipe.json -p enzarb-workflows
# Real sources of every workspace member (the planner copied them all), not cargo-chef 0.0.1 stubs: --locked fails otherwise.
COPY --from=planner /src/ ./
RUN cargo build --release --locked -p enzarb-workflows \
 && cp target/release/enzarb-workflows /enzarb-workflows

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 65532 --no-create-home --shell /usr/sbin/nologin enzarb
COPY --from=build /enzarb-workflows /usr/local/bin/enzarb-workflows
USER 65532:65532
# 8081: executor callback + /healthz. 8082: /healthz for reducer/ticker-only processes.
EXPOSE 8081 8082
ENTRYPOINT ["/usr/local/bin/enzarb-workflows"]
CMD ["run", "--roles", "reducer,executor,ticker"]
