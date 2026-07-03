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

ENV DEBIAN_FRONTEND=noninteractive
ENV HOME=/home/user
ENV PATH="/home/user/.local/bin:/home/user/.local/share/mise/shims:${PATH}"

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg \
    && curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg \
    && echo "deb [arch=amd64 signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu noble stable" \
        > /etc/apt/sources.list.d/docker.list \
    && apt-get update && apt-get install -y --no-install-recommends \
        docker-ce-cli \
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
# than under the read-only DOCKER_CONFIG. MISE_TRUSTED_CONFIG_PATHS trusts the
# generated mise.toml without an interactive `mise trust`.
ENV DOCKER_CONFIG=/etc/enzarb/docker
ENV BUILDX_CONFIG=/home/user/.buildx
ENV MISE_TRUSTED_CONFIG_PATHS=/home/user

# mise (tool version manager) — install the binary to /usr/local/bin (outside
# the home PVC) so it survives an empty PVC being mounted over /home/user.
# Runtime data/shims still live in $HOME/.local/share/mise on the PVC.
RUN curl https://mise.run | MISE_INSTALL_PATH=/usr/local/bin/mise sh

# Node.js + the ACP adapter for the Agent tab (Claude Code), baked in at build
# time via a *system* mise install: MISE_DATA_DIR points outside the home PVC
# (/home/user is empty at runtime until the PVC mounts over it and would wipe
# anything installed under the default $HOME/.local/share/mise), so the
# install + shims here survive pod restarts untouched.
# Pinned explicitly: project-agent's ACP client depends on this adapter's wire
# format, and a floating @latest could silently break it on image rebuild.
ARG NODE_VERSION=lts
ARG CLAUDE_AGENT_ACP_VERSION=0.55.0
ENV MISE_DATA_DIR=/usr/local/share/mise
ENV PATH="/usr/local/share/mise/shims:${PATH}"
# Install node via mise (system-level, outside home PVC) then install the ACP
# adapter globally. A direct symlink in /usr/local/bin bypasses the mise shim
# resolver entirely: shims need a version config to activate, which isn't
# guaranteed in the user's environment, but /usr/local/bin is always on PATH.
RUN mise use -g node@${NODE_VERSION} \
    && npm install -g @agentclientprotocol/claude-agent-acp@${CLAUDE_AGENT_ACP_VERSION} \
    && ln -sf "$(mise which claude-agent-acp)" /usr/local/bin/claude-agent-acp \
    && ln -sf "$(mise which node)" /usr/local/bin/node \
    && ln -sf "$(mise which npm)"  /usr/local/bin/npm \
    && ln -sf "$(mise which npx)"  /usr/local/bin/npx \
    && rm -f /usr/local/share/mise/shims/node \
             /usr/local/share/mise/shims/npm \
             /usr/local/share/mise/shims/npx \
    && chmod -R a+rX /usr/local/share/mise

USER user
WORKDIR /home/user

ENV DOCKER_HOST=tcp://localhost:1234
ENV PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
ENV CHROME_BIN=/usr/bin/chromium

ENTRYPOINT ["/usr/local/bin/tini", "--", "/usr/local/bin/project-agent"]
