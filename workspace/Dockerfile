# ── Rust: cargo-chef puts dependency compilation in image layers keyed only by
# the manifests, so CI layer caching (type=gha) reuses them across builds ──────
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /src

FROM chef AS planner
COPY agent/ .
RUN cargo chef prepare --recipe-path recipe.json

# ── Rust: release build ───────────────────────────────────────────────────────
FROM chef AS rust-builder
COPY --from=planner /src/recipe.json .
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY agent/ .
RUN cargo build --release --locked && \
    cp target/release/project-agent \
       target/release/docker-credential-k8s-sa \
       target/release/kubelet-credential-enzarb /

# ── Runtime ───────────────────────────────────────────────────────────────────
# (No in-image test stage: CI's Test / Rust job gates image builds via
# ci-passed → version → build-images, so cooking deps a second time for
# `cargo test` here only doubled the build.)
FROM ubuntu:26.04

ARG BUILDX_VERSION=0.24.0
ARG TINI_VERSION=0.19.0
ARG CLAUDE_CODE_ACP_VERSION=latest
ARG GEMINI_ACP_VERSION=latest
ARG COPILOT_ACP_VERSION=latest
ARG CODEX_ACP_VERSION=latest
ARG OPENCODE_ACP_VERSION=latest

# mise's shims dir goes first on PATH so every process that inherits this
# environment — including the `project-agent` entrypoint and the
# `claude-agent-acp` child it spawns — resolves mise-managed tool versions.
# Shims are directory-aware (they pick the version from the nearest mise.toml
# at exec time), so this works even though the agent multiplexes sessions
# across different project directories. The dir is created at runtime by
# `mise install` on the PVC; listing it before it exists is harmless.
ENV DEBIAN_FRONTEND=noninteractive \
    HOME=/home/user \
    PATH="/home/user/.local/share/mise/shims:/home/user/.local/bin:${PATH}"

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

# Rename existing ubuntu user (UID 1000) to 'user' for consistency.
# Must happen before npm install: npm writes to $HOME (.npm cache), and
# usermod -m fails if /home/user already exists.
RUN usermod -l user -d /home/user -m ubuntu && groupmod -n user ubuntu

# mise (tool version manager) — binary installed to /usr/local/bin so it
# survives an empty PVC being mounted over /home/user at runtime.
RUN curl https://mise.run | MISE_INSTALL_PATH=/usr/local/bin/mise sh

# Install ACP-speaking agent CLIs for the Agent tab globally via npm — one
# `npm install -g` call so all packages resolve/fetch concurrently instead of
# across separate sequential RUN layers. Package names/flags for providers
# other than Claude are best-effort as of authoring; verify against each
# vendor's current npm distribution (see agent/src/acp/providers.rs).
RUN npm install -g \
        "@agentclientprotocol/claude-agent-acp@${CLAUDE_CODE_ACP_VERSION}" \
        "@google/gemini-cli@${GEMINI_ACP_VERSION}" \
        "@github/copilot@${COPILOT_ACP_VERSION}" \
        "@openai/codex@${CODEX_ACP_VERSION}" \
        "opencode-ai@${OPENCODE_ACP_VERSION}"

# Bake the enzarb prompt into the system-wide bash config so it applies to
# all interactive shells (tmux windows, plain `bash` processes, login shells)
# without requiring runtime patching of the user's home directory.
#
# Ubuntu ships the bash-completion sourcing block in /etc/bash.bashrc commented
# out by default, so the installed package never loads for interactive shells.
# Uncomment it (the `shopt`/`if`/`.`/`elif`/`fi` lines of that block) so tab
# completion works out of the box in every workspace shell.
COPY workspace/enzarb-prompt.sh /etc/profile.d/enzarb-prompt.sh
RUN sed -i '/^#if ! shopt -oq posix; then$/,/^#fi$/ s/^#//' /etc/bash.bashrc \
    && cat /etc/profile.d/enzarb-prompt.sh >> /etc/bash.bashrc

# Copy release binaries (root-owned)
COPY --from=rust-builder /project-agent                /usr/local/bin/project-agent
COPY --from=rust-builder /docker-credential-k8s-sa    /usr/local/bin/docker-credential-k8s-sa
COPY --from=rust-builder /kubelet-credential-enzarb   /usr/local/bin/kubelet-credential-enzarb
RUN chmod 755 /usr/local/bin/project-agent /usr/local/bin/docker-credential-k8s-sa /usr/local/bin/kubelet-credential-enzarb

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
ENV DOCKER_CONFIG=/etc/enzarb/docker \
    BUILDX_CONFIG=/home/user/.buildx \
    MISE_TRUSTED_CONFIG_PATHS=/home/user \
    DOCKER_HOST=tcp://localhost:1234 \
    PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    CHROME_BIN=/usr/bin/chromium

USER user
WORKDIR /home/user

ENTRYPOINT ["/usr/local/bin/tini", "--", "/usr/local/bin/project-agent"]
