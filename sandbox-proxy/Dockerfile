# syntax=docker/dockerfile:1
# Build from the monorepo root (in-repo path deps on proto/ and operator/):
#   docker build -f sandbox-proxy/Dockerfile .
# Dependencies are built in a cargo-chef cook layer so they cache independent of src/.
FROM lukemathwalker/cargo-chef:latest-rust-1-alpine AS chef
RUN apk add --no-cache musl-dev
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
RUN cargo chef cook --release --locked -p enzarb-sandbox-proxy --recipe-path recipe.json
# Real sources of every workspace member (the planner copied them all), not cargo-chef 0.0.1 stubs: --locked fails otherwise.
COPY --from=planner /src/ ./
RUN cargo build --release --locked -p enzarb-sandbox-proxy --bin enzarb-sandbox-proxy

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /src/target/release/enzarb-sandbox-proxy /usr/local/bin/enzarb-sandbox-proxy
USER nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/enzarb-sandbox-proxy"]
