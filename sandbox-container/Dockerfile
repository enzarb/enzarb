# syntax=docker/dockerfile:1
# Sandbox image: sandbox-init (pid 1), node + the ACP adapter, mise and a C toolchain. Language toolchains come from mise at runtime.
# Private enzarb git dependencies are fetched with a deploy key passed as a build secret:
#   docker build --secret id=proto_key,src=KEYFILE -t enzarb-sandbox .
# The key is bound to enzarb-proto via an ssh host alias and never reaches an image layer.
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN mkdir -p -m 0700 /root/.ssh && ssh-keyscan github.com >> /root/.ssh/known_hosts 2>/dev/null
WORKDIR /src

# Dependency recipe: only changes when Cargo.toml/Cargo.lock change.
FROM chef AS planner
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

# Dependency layer (cargo-chef cook): cached by the GHA layer cache while recipe.json is unchanged.
FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
COPY rust-toolchain.toml ./
RUN --mount=type=secret,id=proto_key \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    set -e; \
    (cat /run/secrets/proto_key; echo) > /root/.ssh/proto_key; chmod 600 /root/.ssh/proto_key; \
    printf 'Host gh-proto\n  HostName github.com\n  IdentityFile /root/.ssh/proto_key\n  IdentitiesOnly yes\n  StrictHostKeyChecking accept-new\n' > /root/.ssh/config; \
    git config --global url."ssh://git@gh-proto/enzarb/enzarb-proto".insteadOf "ssh://git@github.com/enzarb/enzarb-proto"; \
    cargo chef cook --release --locked --recipe-path recipe.json; \
    rm -f /root/.ssh/proto_key /root/.gitconfig
# Workspace crates only.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=secret,id=proto_key \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    set -e; \
    (cat /run/secrets/proto_key; echo) > /root/.ssh/proto_key; chmod 600 /root/.ssh/proto_key; \
    printf 'Host gh-proto\n  HostName github.com\n  IdentityFile /root/.ssh/proto_key\n  IdentitiesOnly yes\n  StrictHostKeyChecking accept-new\n' > /root/.ssh/config; \
    git config --global url."ssh://git@gh-proto/enzarb/enzarb-proto".insteadOf "ssh://git@github.com/enzarb/enzarb-proto"; \
    cargo build --release --locked --bin sandbox-init; \
    cp target/release/sandbox-init /sandbox-init; \
    rm -f /root/.ssh/proto_key /root/.gitconfig

# Node stage: download node, install the adapter, drop what the runtime does not need.
FROM ubuntu:26.04 AS node
ARG NODE_VERSION=24.21.0
ARG CLAUDE_AGENT_ACP_VERSION=0.88.0
ARG MISE_VERSION=v2026.10.4
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates xz-utils \
    && rm -rf /var/lib/apt/lists/*
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x64;; arm64) a=arm64;; *) echo "unsupported arch $TARGETARCH"; exit 1;; esac; \
    f="node-v${NODE_VERSION}-linux-${a}.tar.xz"; \
    curl --retry 20 --retry-all-errors -C - -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/${f}"; \
    curl --retry 5 --retry-all-errors -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" | grep " ${f}\$" | sha256sum -c -; \
    tar -xJf "$f" -C /usr/local --strip-components=1 --no-same-owner; \
    node --version
RUN npm install -g --fetch-retries=8 --fetch-retry-mintimeout=2000 "@agentclientprotocol/claude-agent-acp@${CLAUDE_AGENT_ACP_VERSION}"
# Keep only the glibc claude binary for this arch: drop musl and other platforms.
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x64;; arm64) a=arm64;; esac; \
    root=/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai; \
    ls "$root"; \
    for d in "$root"/claude-agent-sdk-*; do \
        case "$d" in "$root/claude-agent-sdk-linux-$a") ;; "$root/claude-agent-sdk") ;; *) rm -rf "$d";; esac; \
    done; \
    test -x "$root/claude-agent-sdk-linux-$a/claude"; \
    ls "$root"; du -sh "$root"/*
# mise (pinned release binary).
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x64;; arm64) a=arm64;; esac; \
    f="mise-${MISE_VERSION}-linux-${a}.tar.gz"; \
    curl --retry 20 --retry-all-errors -C - -fsSLO "https://github.com/jdx/mise/releases/download/${MISE_VERSION}/${f}"; \
    tar -xzf "$f" -C /usr/local --strip-components=1 mise/bin/mise; rm -f "$f"; \
    mise --version

FROM ubuntu:26.04
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        git ca-certificates openssh-client build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/* /var/cache/apt/*

# node binary only (no npm), the adapter tree and the glibc claude binary, all on PATH.
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/bin/mise /usr/local/bin/mise
COPY --from=node /usr/local/lib/node_modules/@agentclientprotocol /usr/local/lib/node_modules/@agentclientprotocol
# The adapter's bin script has `#!/usr/bin/env node`; node is on PATH, so a symlink is the launcher.
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x64;; arm64) a=arm64;; esac; \
    m=/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp; \
    bin=$(node -p "require('$m/package.json').bin['claude-agent-acp']"); \
    chmod +x "$m/$bin"; ln -s "$m/$bin" /usr/local/bin/claude-agent-acp; \
    ln -s "$m/node_modules/@anthropic-ai/claude-agent-sdk-linux-$a/claude" /usr/local/bin/claude; \
    claude-agent-acp --version; claude --version

# uid/gid 1000 matches the operator's securityContext; ubuntu images ship an `ubuntu` user at 1000.
RUN userdel -r ubuntu 2>/dev/null || true; \
    groupadd -g 1000 enzarb && useradd -m -u 1000 -g 1000 -s /bin/bash enzarb \
    && mkdir -p /workspace/repos /workspace/worktrees /workspace/.enzarb /workspace/.toolchains \
    && chown -R 1000:1000 /workspace

USER enzarb
# User and project toolchains (node, rust, python, ...) come from mise and persist on the PVC.
# MISE_YES: never prompt (no tty). MISE_TRUSTED_CONFIG_PATHS: repos cloned under /workspace are trusted.
ENV HOME=/home/enzarb \
    MISE_DATA_DIR=/workspace/.toolchains/mise \
    MISE_CACHE_DIR=/workspace/.toolchains/cache \
    CARGO_HOME=/workspace/.toolchains/cargo \
    RUSTUP_HOME=/workspace/.toolchains/rustup \
    MISE_YES=1 \
    MISE_TRUSTED_CONFIG_PATHS=/workspace \
    MISE_IDIOMATIC_VERSION_FILE_ENABLE_TOOLS=rust \
    PATH=/usr/local/bin:/workspace/.toolchains/mise/shims:/usr/bin:/bin:/usr/sbin:/sbin

COPY --from=build /sandbox-init /usr/local/bin/sandbox-init
WORKDIR /workspace
EXPOSE 7070
ENTRYPOINT ["/usr/local/bin/sandbox-init"]
