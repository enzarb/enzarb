# syntax=docker/dockerfile:1
# Sandbox image: sandbox-init (pid 1) plus the tools agents and humans use.
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

FROM ubuntu:26.04
ARG NODE_VERSION=24.21.0
ARG CLAUDE_AGENT_ACP_VERSION=0.88.0
ARG MISE_VERSION=v2026.10.4
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        git ca-certificates curl openssh-client build-essential pkg-config libssl-dev \
        ripgrep jq tree coreutils xz-utils less procps \
    && rm -rf /var/lib/apt/lists/*

# Node (pinned official tarball, checksum verified).
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x64;; arm64) a=arm64;; *) echo "unsupported arch $TARGETARCH"; exit 1;; esac; \
    f="node-v${NODE_VERSION}-linux-${a}.tar.xz"; \
    curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/${f}"; \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" | grep " ${f}\$" | sha256sum -c -; \
    tar -xJf "$f" -C /usr/local --strip-components=1 --no-same-owner; \
    rm -f "$f" /usr/local/CHANGELOG.md /usr/local/LICENSE /usr/local/README.md; \
    node --version

# ACP adapter (pulls the bundled claude CLI through its optional dependencies).
RUN npm install -g "@agentclientprotocol/claude-agent-acp@${CLAUDE_AGENT_ACP_VERSION}" \
    && npm cache clean --force
# The adapter's bundled native claude CLI, on PATH for the interactive `claude setup-token` flow.
RUN set -e; \
    root=/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai; \
    bin=$(ls "$root"/claude-agent-sdk-linux-*/claude | grep -v musl | head -1); \
    test -x "$bin"; ln -s "$bin" /usr/local/bin/claude; claude --version

# mise (pinned release binary).
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x64;; arm64) a=arm64;; esac; \
    curl -fsSL "https://github.com/jdx/mise/releases/download/${MISE_VERSION}/mise-${MISE_VERSION}-linux-${a}.tar.gz" \
      | tar -xz -C /usr/local --strip-components=1 mise/bin/mise; \
    mise --version

# uid/gid 1000 matches the operator's securityContext; ubuntu images ship an `ubuntu` user at 1000.
RUN userdel -r ubuntu 2>/dev/null || true; \
    groupadd -g 1000 enzarb && useradd -m -u 1000 -g 1000 -s /bin/bash enzarb \
    && mkdir -p /workspace/repos /workspace/worktrees /workspace/.enzarb \
    && chown -R 1000:1000 /workspace

USER enzarb
ENV HOME=/home/enzarb \
    CARGO_HOME=/home/enzarb/.cargo \
    RUSTUP_HOME=/home/enzarb/.rustup \
    PATH=/home/enzarb/.cargo/bin:/home/enzarb/.local/share/mise/shims:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
RUN curl -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain stable -c clippy -c rustfmt \
    && rm /tmp/rustup-init.sh \
    && cargo --version

COPY --from=build /sandbox-init /usr/local/bin/sandbox-init
WORKDIR /workspace
EXPOSE 7070
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:7070/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/sandbox-init"]
