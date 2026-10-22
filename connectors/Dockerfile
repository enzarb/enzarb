# Build from the monorepo root (path dep on workflows/crates/flow):
#   docker build -f connectors/Dockerfile .
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /src

FROM chef AS planner
# Every workspace member is part of the recipe: the root workspace resolves as a whole. The recipe
# only changes when a manifest or Cargo.lock does, so the cook layer below survives source edits.
# BEGIN generated:copy
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY connectors/bin/receiver connectors/bin/receiver
COPY ez/crates/ez-acp ez/crates/ez-acp
COPY ez/crates/ez-claude ez/crates/ez-claude
COPY ez/crates/ez-cli ez/crates/ez-cli
COPY ez/crates/ez-core ez/crates/ez-core
COPY ez/crates/ez-hooks ez/crates/ez-hooks
COPY ez/crates/ez-llm ez/crates/ez-llm
COPY ez/crates/ez-mcp ez/crates/ez-mcp
COPY ez/crates/ez-mise ez/crates/ez-mise
COPY ez/crates/ez-tools ez/crates/ez-tools
COPY ez/crates/ez-workflow ez/crates/ez-workflow
COPY operator/crates/enzarb-crds operator/crates/enzarb-crds
COPY operator/crates/operator operator/crates/operator
COPY operator/deploy operator/deploy
COPY proto/crates/enzarb-proto proto/crates/enzarb-proto
COPY proto/vectors proto/vectors
COPY realtime realtime
COPY sandbox-container sandbox-container
COPY sandbox-proxy sandbox-proxy
COPY updater updater
# END generated:copy
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY rust-toolchain.toml ./
COPY --from=planner /src/recipe.json recipe.json
# Dependency layer: cached until the dependency graph changes.
RUN cargo chef cook --release --locked --recipe-path recipe.json -p enzarb-receiver --bin receiver
# Real sources of every workspace member (the planner copied them all), not cargo-chef 0.0.1 stubs: --locked fails otherwise.
COPY --from=planner /src/ ./
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
