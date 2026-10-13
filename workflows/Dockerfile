# syntax=docker/dockerfile:1.7
#
# Build from the monorepo root (in-repo path deps on proto/, operator/, connectors/):
#   docker build -f workflows/Dockerfile .
#
# Stages (cargo-chef): planner -> cook (dependency layer, cached) -> build (workspace crates only).
FROM rust:1-bookworm AS chef
# cmake + clang: aws-lc-sys (via rustls/jsonwebtoken/reqwest).
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang \
 && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked --version ^0.1
WORKDIR /src/workflows

FROM chef AS planner
# cargo metadata needs the path deps to resolve.
COPY proto/Cargo.toml proto/Cargo.lock /src/proto/
COPY proto/crates /src/proto/crates
COPY operator/Cargo.toml operator/Cargo.lock /src/operator/
COPY operator/crates /src/operator/crates
COPY connectors/Cargo.toml connectors/Cargo.lock /src/connectors/
COPY connectors/crates /src/connectors/crates
COPY connectors/bin /src/connectors/bin
COPY workflows/ .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# Path deps outside this workspace are not in the chef recipe; copy them before cooking.
COPY proto/Cargo.toml proto/Cargo.lock /src/proto/
COPY proto/crates /src/proto/crates
COPY operator/Cargo.toml operator/Cargo.lock /src/operator/
COPY operator/crates /src/operator/crates
COPY connectors/Cargo.toml connectors/Cargo.lock /src/connectors/
COPY connectors/crates /src/connectors/crates
COPY connectors/bin /src/connectors/bin
COPY --from=planner /src/workflows/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p enzarb-workflows
COPY workflows/ .
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
