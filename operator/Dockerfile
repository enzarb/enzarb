FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p enzarb-sandbox-operator

FROM alpine:3.24
LABEL org.opencontainers.image.source="https://github.com/enzarb/enzarb-sandbox-operator" \
      org.opencontainers.image.description="enzarb Project operator"
COPY --from=build /src/target/release/enzarb-sandbox-operator /usr/local/bin/enzarb-sandbox-operator
# Unprivileged; the operator only talks to the Kubernetes API and serves health on 8080.
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/enzarb-sandbox-operator"]
