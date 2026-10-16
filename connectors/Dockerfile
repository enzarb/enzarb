# Build from the monorepo root (path dep on workflows/crates/flow):
#   docker build -f connectors/Dockerfile .
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /src/connectors

FROM chef AS planner
COPY workflows/crates/flow /src/workflows/crates/flow
COPY connectors/rust-toolchain.toml connectors/Cargo.toml connectors/Cargo.lock ./
COPY connectors/crates crates
COPY connectors/bin bin
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# Path deps outside this workspace are not in the chef recipe; copy them before cooking.
COPY workflows/crates/flow /src/workflows/crates/flow
COPY --from=planner /src/connectors/recipe.json recipe.json
COPY connectors/rust-toolchain.toml ./
# Dependency layer: cached until the dependency graph changes.
RUN cargo chef cook --release --locked --recipe-path recipe.json -p enzarb-receiver --bin receiver
COPY connectors/Cargo.toml connectors/Cargo.lock ./
COPY connectors/crates crates
COPY connectors/bin bin
RUN cargo build --release --locked -p enzarb-receiver --bin receiver

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 receiver
COPY --from=build /src/connectors/target/release/receiver /usr/local/bin/receiver
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/receiver"]
CMD ["serve"]
