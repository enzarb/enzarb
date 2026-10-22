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
# Dependency layer: cached until the dependency set changes.
RUN cargo chef cook --release --locked -p enzarb-sandbox-operator --recipe-path recipe.json
# Real sources of every workspace member (the planner copied them all), not cargo-chef 0.0.1 stubs: --locked fails otherwise.
COPY --from=planner /src/ ./
RUN cargo build --release --locked -p enzarb-sandbox-operator

FROM alpine:3.24
LABEL org.opencontainers.image.source="https://github.com/enzarb/enzarb" \
      org.opencontainers.image.description="enzarb Project operator"
COPY --from=build /src/target/release/enzarb-sandbox-operator /usr/local/bin/enzarb-sandbox-operator
# Unprivileged; the operator only talks to the Kubernetes API and serves health on 8080.
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/enzarb-sandbox-operator"]
