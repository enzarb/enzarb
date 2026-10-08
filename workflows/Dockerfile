# syntax=docker/dockerfile:1.7
#
# Private enzarb git dependencies need per-repo deploy keys, passed as build secrets:
#   docker build --secret id=proto_key,src=KEY --secret id=crds_key,src=KEY \
#                --secret id=connectors_key,src=KEY -t enzarb-workflows .
# (locally any key with access to all three repos works). Each key is bound to its repo via
# an ssh host alias and deleted in the same RUN; keys never reach an image layer.
FROM rust:1-bookworm AS build
# cmake + clang: aws-lc-sys (via rustls/jsonwebtoken/reqwest).
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang git openssh-client \
 && rm -rf /var/lib/apt/lists/*
RUN mkdir -p -m 0700 /root/.ssh && ssh-keyscan github.com >> /root/.ssh/known_hosts
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
WORKDIR /src
COPY . .
RUN --mount=type=secret,id=proto_key --mount=type=secret,id=crds_key \
    --mount=type=secret,id=connectors_key \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    set -e; \
    : > /root/.ssh/config; \
    for pair in proto:enzarb-proto crds:enzarb-sandbox-operator connectors:enzarb-connectors; do \
      n=${pair%%:*}; repo=${pair#*:}; \
      (cat /run/secrets/${n}_key; echo) > /root/.ssh/${n}_key; chmod 600 /root/.ssh/${n}_key; \
      printf 'Host gh-%s\n  HostName github.com\n  IdentityFile /root/.ssh/%s_key\n  IdentitiesOnly yes\n  StrictHostKeyChecking accept-new\n' "$n" "$n" >> /root/.ssh/config; \
      git config --global url."ssh://git@gh-$n/enzarb/$repo".insteadOf "ssh://git@github.com/enzarb/$repo"; \
    done; \
    cargo build --release --locked -p enzarb-workflows; \
    cp target/release/enzarb-workflows /enzarb-workflows; \
    rm -f /root/.ssh/*_key

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 65532 --no-create-home --shell /usr/sbin/nologin enzarb
COPY --from=build /enzarb-workflows /usr/local/bin/enzarb-workflows
USER 65532:65532
# 8081: executor callback + /healthz. 8082: /healthz for reducer/ticker-only processes.
EXPOSE 8081 8082
ENTRYPOINT ["/usr/local/bin/enzarb-workflows"]
CMD ["run", "--roles", "reducer,executor,ticker"]
