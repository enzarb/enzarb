# syntax=docker/dockerfile:1
# Build from the monorepo root (one Cargo workspace; no private dependencies):
#   docker build -f realtime/Dockerfile .
# Dependencies are built in a cargo-chef cook layer so they cache independent of src/.
FROM lukemathwalker/cargo-chef:latest-rust-1-alpine AS chef
RUN apk add --no-cache musl-dev
WORKDIR /src

FROM chef AS planner
# Every workspace member is part of the recipe: the root workspace resolves as a whole. The recipe
# only changes when a manifest or Cargo.lock does, so the cook layer below survives source edits.
# BEGIN generated:copy
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY connectors/bin/receiver connectors/bin/receiver
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
RUN cargo chef cook --release --locked -p enzarb-realtime --recipe-path recipe.json
# Real sources of every workspace member (the planner copied them all), not cargo-chef 0.0.1 stubs: --locked fails otherwise.
COPY --from=planner /src/ ./
RUN cargo build --release --locked -p enzarb-realtime --bin enzarb-realtime

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /src/target/release/enzarb-realtime /usr/local/bin/enzarb-realtime
USER nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/enzarb-realtime"]
