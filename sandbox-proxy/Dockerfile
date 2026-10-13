# syntax=docker/dockerfile:1
# Build from the monorepo root (in-repo path deps on proto/ and operator/):
#   docker build -f sandbox-proxy/Dockerfile .
# Dependencies are built in a cargo-chef cook layer so they cache independent of src/.
FROM lukemathwalker/cargo-chef:latest-rust-1-alpine AS chef
RUN apk add --no-cache musl-dev
WORKDIR /src/sandbox-proxy

FROM chef AS planner
COPY sandbox-proxy/rust-toolchain.toml sandbox-proxy/Cargo.toml sandbox-proxy/Cargo.lock ./
COPY sandbox-proxy/src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY sandbox-proxy/rust-toolchain.toml ./
# Path deps outside this workspace are not in the chef recipe; copy them before cooking.
COPY proto/Cargo.toml proto/Cargo.lock /src/proto/
COPY proto/crates /src/proto/crates
COPY operator/Cargo.toml operator/Cargo.lock /src/operator/
COPY operator/crates /src/operator/crates
COPY --from=planner /src/sandbox-proxy/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY sandbox-proxy/Cargo.toml sandbox-proxy/Cargo.lock ./
COPY sandbox-proxy/src ./src
RUN cargo build --release --locked --bin enzarb-sandbox-proxy

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /src/sandbox-proxy/target/release/enzarb-sandbox-proxy /usr/local/bin/enzarb-sandbox-proxy
USER nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/enzarb-sandbox-proxy"]
