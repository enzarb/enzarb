# ── Rust: fetch deps (cached layer) ──────────────────────────────────────────
FROM rust:latest AS rust-deps
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
    cargo build --release

# ── Go: fetch deps (cached layer) ─────────────────────────────────────────────
FROM golang:1.26.4-alpine AS go-deps
WORKDIR /src
COPY go.mod go.sum ./
RUN --mount=type=cache,target=/root/go/pkg/mod \
    go mod download

# ── Go: test (parallel with go-builder) ───────────────────────────────────────
FROM go-deps AS go-test
COPY . .
RUN --mount=type=cache,target=/root/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    go test ./operator/... ./metering/... ./billing/... && touch /test-ok

# ── Go: credential-helper build (parallel with go-test) ───────────────────────
FROM go-deps AS go-builder
COPY workspace/credential-helper/ ./workspace/credential-helper/
RUN --mount=type=cache,target=/root/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 go build -ldflags="-s -w" \
        -o /docker-credential-k8s-sa ./workspace/credential-helper/

# ── Runtime ───────────────────────────────────────────────────────────────────
FROM ubuntu:26.04

ARG BUILDX_VERSION=0.24.0
ARG TINI_VERSION=0.19.0

ENV DEBIAN_FRONTEND=noninteractive
ENV HOME=/home/user
ENV PATH="/home/user/.local/bin:/home/user/.local/share/mise/shims:${PATH}"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    gnupg \
    tmux \
    jq \
    openssh-client \
    build-essential \
    pkg-config \
    libssl-dev \
    python3 \
    python3-pip \
    && rm -rf /var/lib/apt/lists/*

# Docker CE CLI (no daemon)
RUN curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg \
    && echo "deb [arch=amd64 signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu noble stable" \
    > /etc/apt/sources.list.d/docker.list \
    && apt-get update && apt-get install -y --no-install-recommends docker-ce-cli \
    && rm -rf /var/lib/apt/lists/*

# Docker buildx plugin
RUN mkdir -p /usr/local/lib/docker/cli-plugins \
    && curl -fsSL "https://github.com/docker/buildx/releases/download/v${BUILDX_VERSION}/buildx-v${BUILDX_VERSION}.linux-amd64" \
    -o /usr/local/lib/docker/cli-plugins/docker-buildx \
    && chmod +x /usr/local/lib/docker/cli-plugins/docker-buildx

# tini (PID 1 signal forwarding)
RUN curl -fsSL "https://github.com/krallin/tini/releases/download/v${TINI_VERSION}/tini-amd64" \
    -o /usr/local/bin/tini && chmod +x /usr/local/bin/tini

# Rename existing ubuntu user (UID 1000) to 'user' for consistency
RUN usermod -l user -d /home/user -m ubuntu && groupmod -n user ubuntu

# Gate on both test stages passing, then copy release binaries (root-owned)
COPY --from=rust-test /test-ok /tmp/rust-test-ok
COPY --from=go-test   /test-ok /tmp/go-test-ok
COPY --from=rust-builder /src/target/release/project-agent      /usr/local/bin/project-agent
COPY --from=go-builder   /docker-credential-k8s-sa              /usr/local/bin/docker-credential-k8s-sa
RUN chmod 755 /usr/local/bin/project-agent /usr/local/bin/docker-credential-k8s-sa \
    && rm /tmp/rust-test-ok /tmp/go-test-ok

# mise (tool version manager) — installed as user so shims land in home (PVC)
USER user
RUN curl https://mise.run | sh

WORKDIR /home/user

ENV DOCKER_HOST=tcp://localhost:1234

ENTRYPOINT ["/usr/local/bin/tini", "--", "/usr/local/bin/project-agent"]
