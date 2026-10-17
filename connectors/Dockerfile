# Build from the monorepo root (path dep on workflows/crates/flow):
#   docker build -f connectors/Dockerfile .
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
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
# Dependency layer: cached until the dependency graph changes.
RUN cargo chef cook --release --locked --recipe-path recipe.json -p enzarb-receiver --bin receiver
COPY Cargo.toml Cargo.lock ./
COPY workflows/crates/flow workflows/crates/flow
COPY connectors/crates connectors/crates
COPY connectors/bin connectors/bin
RUN cargo build --release --locked -p enzarb-receiver --bin receiver

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 receiver
COPY --from=build /src/target/release/receiver /usr/local/bin/receiver
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/receiver"]
CMD ["serve"]
