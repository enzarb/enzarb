# syntax=docker/dockerfile:1
# No private dependencies. Dependencies are built in a cargo-chef cook layer so they cache independent of src/.
FROM lukemathwalker/cargo-chef:latest-rust-1-alpine AS chef
RUN apk add --no-cache musl-dev
WORKDIR /src

FROM chef AS planner
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY rust-toolchain.toml ./
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin enzarb-realtime

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /src/target/release/enzarb-realtime /usr/local/bin/enzarb-realtime
USER nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/enzarb-realtime"]
