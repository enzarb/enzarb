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
    cp /src/target/release/docker-credential-k8s-sa /docker-credential-k8s-sa

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
        jq \
        openssh-client \
        build-essential \
        pkg-config \
        libssl-dev \
        python3 \
        python3-pip \
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

# Gate on tests passing, then copy release binaries (root-owned)
COPY --from=rust-test /test-ok /tmp/rust-test-ok
COPY --from=rust-builder /project-agent                /usr/local/bin/project-agent
COPY --from=rust-builder /docker-credential-k8s-sa    /usr/local/bin/docker-credential-k8s-sa
RUN chmod 755 /usr/local/bin/project-agent /usr/local/bin/docker-credential-k8s-sa \
    && rm /tmp/rust-test-ok

# mise (tool version manager) — install the binary to /usr/local/bin (outside
# the home PVC) so it survives an empty PVC being mounted over /home/user.
# Runtime data/shims still live in $HOME/.local/share/mise on the PVC.
RUN curl https://mise.run | MISE_INSTALL_PATH=/usr/local/bin/mise sh

USER user
WORKDIR /home/user

ENV DOCKER_HOST=tcp://localhost:1234

ENTRYPOINT ["/usr/local/bin/tini", "--", "/usr/local/bin/project-agent"]
