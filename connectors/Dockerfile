FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /src

FROM chef AS planner
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
COPY rust-toolchain.toml ./
# Dependency layer: cached until the dependency graph changes.
RUN cargo chef cook --release --locked --recipe-path recipe.json -p enzarb-receiver --bin receiver
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
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
