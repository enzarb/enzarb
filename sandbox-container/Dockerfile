# syntax=docker/dockerfile:1
# Sandbox image: sandbox-init (pid 1), node + the ACP adapter, mise, Docker (dockerd + CLI, buildx, compose; dockerd is supervised by sandbox-init), a C toolchain, sccache (RUSTC_WRAPPER, cache on the PVC) and Chromium for Playwright (/opt/ms-playwright). Language toolchains come from mise at runtime.
# Build from the monorepo root (in-repo path dep on proto/):
#   docker build -f sandbox-container/Dockerfile -t enzarb-sandbox .
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /src

# Dependency recipe: only changes when a Cargo.toml/Cargo.lock changes.
FROM chef AS planner
# Every workspace member is part of the recipe: the root workspace resolves as a whole. The recipe
# only changes when a manifest or Cargo.lock does, so the cook layer below survives source edits.
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY proto/crates proto/crates
COPY operator/crates operator/crates
COPY connectors/crates connectors/crates
COPY connectors/bin connectors/bin
COPY workflows/crates workflows/crates
COPY realtime/Cargo.toml realtime/Cargo.toml
COPY realtime/src realtime/src
COPY sandbox-container/Cargo.toml sandbox-container/Cargo.toml
COPY sandbox-container/src sandbox-container/src
COPY sandbox-proxy/Cargo.toml sandbox-proxy/Cargo.toml
COPY sandbox-proxy/src sandbox-proxy/src
COPY updater/Cargo.toml updater/Cargo.toml
COPY updater/src updater/src
RUN cargo chef prepare --recipe-path recipe.json

# Dependency layer (cargo-chef cook): cached by the GHA layer cache while recipe.json is unchanged.
FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
COPY rust-toolchain.toml ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo chef cook --release --locked --recipe-path recipe.json -p sandbox-init
# Real sources of every workspace member (the planner copied them all), not cargo-chef 0.0.1 stubs: --locked fails otherwise.
COPY --from=planner /src/ ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --release --locked -p sandbox-init --bin sandbox-init \
 && cp target/release/sandbox-init /sandbox-init

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

# Docker stage: the static Docker bundle (docker, dockerd, containerd, containerd-shim-runc-v2, ctr, runc,
# docker-init, docker-proxy) + buildx + compose plugins. sandbox-init runs and supervises dockerd (ADR-0016).
# Every download is checked against a pinned sha256.
FROM ubuntu:26.04 AS dockercli
ARG DOCKER_VERSION=29.8.2
ARG BUILDX_VERSION=v0.38.0
ARG COMPOSE_VERSION=v5.6.0
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN set -e; \
    case "$TARGETARCH" in \
      amd64) da=x86_64; ba=amd64; ca=x86_64; \
        dsha=995d1ef289677f74fd58d8d2c35727b6a4ee389c69db8638a3e42d0487aa5b0f; \
        bsha=4fe4cc38adf48169132749b6ca22a990928db0118e3407584ee553723115d287; \
        csha=40343e21ca777173e69cff5dbafeb37c6f81f3b0d57d9e597f036e95eb63e76a;; \
      arm64) da=aarch64; ba=arm64; ca=aarch64; \
        dsha=76a624e4a8e5da654d1150e808175125efb5a6f1b6aa1cbd9caee18f51047a50; \
        bsha=38e890e1a162bfdbf32fbe91404991dc4fc28e55b0de901b34374a061bab4d2c; \
        csha=733ec76717ceb59052a9609b9dadfb523b2df8eab57a54212872d10a58078ea2;; \
      *) echo "unsupported arch $TARGETARCH"; exit 1;; \
    esac; \
    mkdir -p /out/bin /out/plugins; \
    curl --retry 5 --retry-all-errors -fsSL -o docker.tgz "https://download.docker.com/linux/static/stable/${da}/docker-${DOCKER_VERSION}.tgz"; \
    echo "${dsha}  docker.tgz" | sha256sum -c -; \
    tar -xzf docker.tgz -C /out/bin --strip-components=1 --no-same-owner; \
    for b in docker dockerd containerd containerd-shim-runc-v2 runc; do test -x "/out/bin/$b"; done; \
    curl --retry 5 --retry-all-errors -fsSL -o /out/plugins/docker-buildx "https://github.com/docker/buildx/releases/download/${BUILDX_VERSION}/buildx-${BUILDX_VERSION}.linux-${ba}"; \
    echo "${bsha}  /out/plugins/docker-buildx" | sha256sum -c -; \
    curl --retry 5 --retry-all-errors -fsSL -o /out/plugins/docker-compose "https://github.com/docker/compose/releases/download/${COMPOSE_VERSION}/docker-compose-linux-${ca}"; \
    echo "${csha}  /out/plugins/docker-compose" | sha256sum -c -; \
    chmod 0755 /out/plugins/*; \
    /out/bin/docker --version; /out/bin/dockerd --version; /out/bin/runc --version

# sccache stage: the static musl release, checked against the sha256 file published next to it.
FROM ubuntu:26.04 AS sccache
ARG SCCACHE_VERSION=v0.10.0
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x86_64;; arm64) a=aarch64;; *) echo "unsupported arch $TARGETARCH"; exit 1;; esac; \
    d="sccache-${SCCACHE_VERSION}-${a}-unknown-linux-musl"; \
    base="https://github.com/mozilla/sccache/releases/download/${SCCACHE_VERSION}"; \
    curl --retry 5 --retry-all-errors -fsSLO "${base}/${d}.tar.gz"; \
    curl --retry 5 --retry-all-errors -fsSLO "${base}/${d}.tar.gz.sha256"; \
    echo "$(cut -d' ' -f1 "${d}.tar.gz.sha256")  ${d}.tar.gz" | sha256sum -c -; \
    tar -xzf "${d}.tar.gz" --strip-components=1 --no-same-owner "${d}/sccache"; \
    install -m 0755 sccache /usr/local/bin/sccache; \
    /usr/local/bin/sccache --version

# Playwright stage: Chromium for the ui browser tests, installed into /opt/ms-playwright. Keep
# PLAYWRIGHT_VERSION equal to the playwright version in ui/package-lock.json
# (scripts/check-playwright-version.sh, run in CI).
# gh stage: the GitHub CLI, checked against the checksums file published with the release. Only a
# CI-fix run has a GH_TOKEN, read-only and for one repository (`gh run view --log-failed`); see the
# README.
FROM ubuntu:26.04 AS gh
ARG GH_VERSION=2.76.2
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN set -e; \
    case "$TARGETARCH" in amd64|arm64) ;; *) echo "unsupported arch $TARGETARCH"; exit 1;; esac; \
    d="gh_${GH_VERSION}_linux_${TARGETARCH}"; \
    base="https://github.com/cli/cli/releases/download/v${GH_VERSION}"; \
    curl --retry 5 --retry-all-errors -fsSLO "${base}/${d}.tar.gz"; \
    curl --retry 5 --retry-all-errors -fsSL "${base}/gh_${GH_VERSION}_checksums.txt" | grep " ${d}.tar.gz\$" | sha256sum -c -; \
    tar -xzf "${d}.tar.gz" --strip-components=2 --no-same-owner "${d}/bin/gh"; \
    install -m 0755 gh /usr/local/bin/gh; \
    /usr/local/bin/gh --version

FROM node AS playwright
ARG PLAYWRIGHT_VERSION=1.64.0
ARG TARGETARCH
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN npm install -g --fetch-retries=8 --fetch-retry-mintimeout=2000 "playwright@${PLAYWRIGHT_VERSION}"
# Installs both chromium and chromium-headless-shell; --with-deps is not used (the final stage installs the libs).
# Playwright does not know Ubuntu 26.04 and refuses to pick a build; use its 24.04 one.
RUN set -e; \
    case "$TARGETARCH" in amd64) a=x64;; arm64) a=arm64;; *) echo "unsupported arch $TARGETARCH"; exit 1;; esac; \
    PLAYWRIGHT_HOST_PLATFORM_OVERRIDE="ubuntu24.04-${a}" playwright install chromium
RUN chmod -R a+rX /opt/ms-playwright && ls /opt/ms-playwright

FROM ubuntu:26.04
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
# The second line is Chromium's runtime libraries (Playwright's own list, by hand: --with-deps does not know this release).
RUN apt-get update && apt-get install -y --no-install-recommends \
        git curl ca-certificates openssh-client build-essential pkg-config libssl-dev procps \
        libnss3 libnspr4 libatk1.0-0t64 libatk-bridge2.0-0t64 libcups2t64 libdrm2 libgbm1 libxkbcommon0 \
        libxcomposite1 libxdamage1 libxfixes3 libxrandr2 libpango-1.0-0 libcairo2 libasound2t64 \
        libx11-6 libxcb1 libxext6 libdbus-1-3 libglib2.0-0t64 fonts-liberation fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/* /var/cache/apt/*

COPY --from=sccache /usr/local/bin/sccache /usr/local/bin/sccache
COPY --from=gh /usr/local/bin/gh /usr/local/bin/gh
# Chromium lives outside /workspace: the PVC mount would shadow it.
COPY --from=playwright /opt/ms-playwright /opt/ms-playwright

# node binary only (no npm), the adapter tree and the glibc claude binary, all on PATH.
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/bin/mise /usr/local/bin/mise
# Docker daemon and client (ADR-0016). The client lives in libexec; /usr/local/bin/docker is a wrapper that
# defaults `run`/`create`/`build` to --network=host (the daemon runs with --iptables=false, so the default
# bridge has no NAT).
COPY --from=dockercli /out/bin/ /usr/local/bin/
COPY --from=dockercli /out/plugins/ /usr/local/lib/docker/cli-plugins/
COPY sandbox-container/scripts/docker-wrapper.sh /usr/local/bin/docker-wrapper
RUN mkdir -p /usr/local/libexec/docker \
    && mv /usr/local/bin/docker /usr/local/libexec/docker/docker \
    && mv /usr/local/bin/docker-wrapper /usr/local/bin/docker \
    && chmod 0755 /usr/local/bin/docker
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
    && mkdir -p /workspace/repos /workspace/worktrees /workspace/.enzarb /workspace/.toolchains /workspace/.cache \
    && chown -R 1000:1000 /workspace

# No `USER`: pid 1 (sandbox-init) starts as root to supervise dockerd, and drops every child it spawns
# (agents, git, mise, PTY shells) to uid/gid 1000 with no-new-privs (ADR-0016). `docker exec -u enzarb`
# gives the sandbox user's view.
# User and project toolchains (node, rust, python, ...) come from mise and persist on the PVC.
# MISE_YES: never prompt (no tty). MISE_TRUSTED_CONFIG_PATHS: repos cloned under /workspace are trusted.
# DOCKER_HOST: the socket of the dockerd that sandbox-init supervises (group 1000, mode 0660).
ENV HOME=/home/enzarb \
    DOCKER_HOST=unix:///var/run/docker.sock \
    MISE_DATA_DIR=/workspace/.toolchains/mise \
    MISE_CACHE_DIR=/workspace/.toolchains/cache \
    CARGO_HOME=/workspace/.toolchains/cargo \
    RUSTUP_HOME=/workspace/.toolchains/rustup \
    MISE_YES=1 \
    ENZARB_MISE_BIN=mise \
    MISE_TRUSTED_CONFIG_PATHS=/workspace \
    MISE_IDIOMATIC_VERSION_FILE_ENABLE_TOOLS=rust \
    RUSTC_WRAPPER=sccache \
    SCCACHE_DIR=/workspace/.cache/sccache \
    SCCACHE_CACHE_SIZE=20G \
    npm_config_cache=/workspace/.cache/npm \
    PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    PATH=/usr/local/bin:/workspace/.toolchains/mise/shims:/usr/bin:/bin:/usr/sbin:/sbin

COPY --from=build /sandbox-init /usr/local/bin/sandbox-init
WORKDIR /workspace
EXPOSE 7070
ENTRYPOINT ["/usr/local/bin/sandbox-init"]
