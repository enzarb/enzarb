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
# BEGIN generated:copy
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY connectors/bin/receiver connectors/bin/receiver
COPY connectors/crates/connector connectors/crates/connector
COPY connectors/crates/connector-github connectors/crates/connector-github
COPY operator/crates/enzarb-crds operator/crates/enzarb-crds
COPY operator/crates/operator operator/crates/operator
COPY operator/deploy operator/deploy
COPY proto/crates/enzarb-proto proto/crates/enzarb-proto
COPY proto/vectors proto/vectors
COPY realtime realtime
COPY sandbox-container sandbox-container
COPY sandbox-proxy sandbox-proxy
COPY updater updater
COPY workflows/crates/engine workflows/crates/engine
COPY workflows/crates/enzarb-workflows workflows/crates/enzarb-workflows
COPY workflows/crates/flow workflows/crates/flow
COPY workflows/migrations workflows/migrations
# END generated:copy
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
