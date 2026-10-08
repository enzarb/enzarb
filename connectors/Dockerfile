FROM rust:1-bookworm AS build
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
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
