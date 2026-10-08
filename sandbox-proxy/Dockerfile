# syntax=docker/dockerfile:1
# Private enzarb git dependencies need per-repo deploy keys, passed as build secrets:
#   docker build --secret id=proto_key,src=KEYFILE --secret id=crds_key,src=KEYFILE .
# Each key is bound to its repo via an ssh host alias; keys never reach an image layer.
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev git openssh-client
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN mkdir -p -m 0700 /root/.ssh && ssh-keyscan github.com >> /root/.ssh/known_hosts 2>/dev/null
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=secret,id=proto_key --mount=type=secret,id=crds_key \
    --mount=type=cache,target=/usr/local/cargo/registry \
    set -e; \
    (cat /run/secrets/proto_key; echo) > /root/.ssh/proto_key; chmod 600 /root/.ssh/proto_key; \
    (cat /run/secrets/crds_key; echo) > /root/.ssh/crds_key; chmod 600 /root/.ssh/crds_key; \
    printf 'Host gh-proto\n  HostName github.com\n  IdentityFile /root/.ssh/proto_key\n  IdentitiesOnly yes\nHost gh-crds\n  HostName github.com\n  IdentityFile /root/.ssh/crds_key\n  IdentitiesOnly yes\n' > /root/.ssh/config; \
    git config --global url."ssh://git@gh-proto/enzarb/enzarb-proto".insteadOf "ssh://git@github.com/enzarb/enzarb-proto"; \
    git config --global url."ssh://git@gh-crds/enzarb/enzarb-sandbox-operator".insteadOf "ssh://git@github.com/enzarb/enzarb-sandbox-operator"; \
    printf 'Host gh-*\n  StrictHostKeyChecking accept-new\n' >> /root/.ssh/config; \
    cargo build --release --locked --bin enzarb-sandbox-proxy; \
    rm -f /root/.ssh/proto_key /root/.ssh/crds_key

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /src/target/release/enzarb-sandbox-proxy /usr/local/bin/enzarb-sandbox-proxy
USER nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/enzarb-sandbox-proxy"]
