FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p enzarb-sandbox-operator

FROM alpine:3.24
COPY --from=build /src/target/release/enzarb-sandbox-operator /usr/local/bin/enzarb-sandbox-operator
ENTRYPOINT ["/usr/local/bin/enzarb-sandbox-operator"]
