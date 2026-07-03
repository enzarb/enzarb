# ── Rust: fetch deps (cached layer) ──────────────────────────────────────────
FROM rust:1 AS rust-deps
WORKDIR /src
COPY agent/Cargo.toml agent/Cargo.lock ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo fetch --locked

# ── Rust: test (parallel with rust-builder) ───────────────────────────────────
FROM rust-deps AS rust-test
COPY agent/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo test && touch /test-ok

# ── Rust: release build (parallel with rust-test) ─────────────────────────────
FROM rust-deps AS rust-builder
COPY agent/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release && \
    cp /src/target/release/project-agent /project-agent && \
    cp /src/target/release/docker-credential-k8s-sa /docker-credential-k8s-sa && \
    cp /src/target/release/kubelet-credential-enzarb /kubelet-credential-enzarb

# ── Runtime ───────────────────────────────────────────────────────────────────
FROM ubuntu:26.04

ARG BUILDX_VERSION=0.24.0
ARG TINI_VERSION=0.19.0
ARG CLAUDE_CODE_ACP_VERSION=latest

ENV DEBIAN_FRONTEND=noninteractive
ENV HOME=/home/user
ENV PATH="/home/user/.local/bin:${PATH}"

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg \
    && curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg \
    && echo "deb [arch=amd64 signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu noble stable" \
        > /etc/apt/sources.list.d/docker.list \
    && curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - \
    && apt-get install -y --no-install-recommends \
        docker-ce-cli \
        nodejs \
        git \
        tmux \
        ncurses-term \
        bash-completion \
        jq \
        vim \
        openssh-client \
        build-essential \
        pkg-config \
        libssl-dev \
        python3 \
        python3-pip \
        chromium \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /usr/local/lib/docker/cli-plugins \
    && curl -fsSL "https://github.com/docker/buildx/releases/download/v${BUILDX_VERSION}/buildx-v${BUILDX_VERSION}.linux-amd64" \
        -o /usr/local/lib/docker/cli-plugins/docker-buildx \
    && chmod +x /usr/local/lib/docker/cli-plugins/docker-buildx \
    && curl -fsSL "https://github.com/krallin/tini/releases/download/v${TINI_VERSION}/tini-amd64" \
        -o /usr/local/bin/tini \
    && chmod +x /usr/local/bin/tini

# Install ACP adapter for the Agent tab (Claude Code) globally via npm.
RUN if [ "${CLAUDE_CODE_ACP_VERSION}" = "latest" ]; then \
        npm install -g @agentclientprotocol/claude-agent-acp; \
    else \
        npm install -g @agentclientprotocol/claude-agent-acp@${CLAUDE_CODE_ACP_VERSION}; \
    fi

# Rename existing ubuntu user (UID 1000) to 'user' for consistency
RUN usermod -l user -d /home/user -m ubuntu && groupmod -n user ubuntu

# Bake the enzarb prompt into the system-wide bash config so it applies to
# all interactive shells (tmux windows, plain `bash` processes, login shells)
# without requiring runtime patching of the user's home directory.
COPY workspace/enzarb-prompt.sh /etc/profile.d/enzarb-prompt.sh
RUN cat /etc/profile.d/enzarb-prompt.sh >> /etc/bash.bashrc

# Gate on tests passing, then copy release binaries (root-owned)
COPY --from=rust-test /test-ok /tmp/rust-test-ok
COPY --from=rust-builder /project-agent                /usr/local/bin/project-agent
COPY --from=rust-builder /docker-credential-k8s-sa    /usr/local/bin/docker-credential-k8s-sa
COPY --from=rust-builder /kubelet-credential-enzarb   /usr/local/bin/kubelet-credential-enzarb
RUN chmod 755 /usr/local/bin/project-agent /usr/local/bin/docker-credential-k8s-sa /usr/local/bin/kubelet-credential-enzarb \
    && rm /tmp/rust-test-ok

# Register the credential helpers at build time (credential-free; the helpers
# fetch the live projected SA tokens on demand). The docker config lives outside
# the home PVC so it survives an empty PVC being mounted over /home/user:
#   - docker: DOCKER_CONFIG points at a baked, user-writable dir (buildx also
#     stores its builder state here).
RUN mkdir -p /etc/enzarb/docker \
    && printf '{"credHelpers":{"registry.enzarb.dev":"k8s-sa"}}\n' > /etc/enzarb/docker/config.json \
    && chown -R user:user /etc/enzarb

# Skeleton home files copied to $HOME on first boot (before the PVC is populated).
# Add new default dotfiles/configs here; init.rs merges them idempotently.
COPY --chown=user:user workspace/skel /etc/enzarb/skel
# DOCKER_CONFIG holds the (read-only) credHelpers pointer. buildx must write its
# builder state somewhere writable, so point BUILDX_CONFIG at the home PVC rather
# than under the read-only DOCKER_CONFIG.
ENV DOCKER_CONFIG=/etc/enzarb/docker
ENV BUILDX_CONFIG=/home/user/.buildx

USER user
WORKDIR /home/user

ENV DOCKER_HOST=tcp://localhost:1234
ENV PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
ENV CHROME_BIN=/usr/bin/chromium

ENTRYPOINT ["/usr/local/bin/tini", "--", "/usr/local/bin/project-agent"]
