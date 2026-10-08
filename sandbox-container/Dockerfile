# syntax=docker/dockerfile:1
# Builds the `sandbox-init` binary. The full sandbox image (tools, claude, git) is a separate issue.
# Private git dependencies need an ssh agent: docker build --ssh default .
FROM rust:1 AS build
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
WORKDIR /src
RUN mkdir -p -m 0700 ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
RUN --mount=type=ssh --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --release --locked && cp target/release/sandbox-init /sandbox-init

FROM ubuntu:26.04
COPY --from=build /sandbox-init /usr/local/bin/sandbox-init
EXPOSE 7070
ENTRYPOINT ["/usr/local/bin/sandbox-init"]
