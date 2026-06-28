FROM rust:1.88-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs \
    && cargo build --release \
    && rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release --bin enzarb-webhook

FROM gcr.io/distroless/static:nonroot
COPY --from=builder /src/target/release/enzarb-webhook /enzarb-webhook
USER nonroot:nonroot
ENTRYPOINT ["/enzarb-webhook"]
