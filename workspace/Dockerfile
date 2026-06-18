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

# project-agent and credential helper: root-owned, world-executable
COPY project-agent /usr/local/bin/project-agent
COPY docker-credential-k8s-sa /usr/local/bin/docker-credential-k8s-sa
RUN chmod 755 /usr/local/bin/project-agent /usr/local/bin/docker-credential-k8s-sa

# mise (tool version manager) — installed as user so shims land in home (PVC)
USER user
RUN curl https://mise.run | sh

WORKDIR /home/user

ENV DOCKER_HOST=tcp://localhost:1234

ENTRYPOINT ["/usr/local/bin/tini", "--", "/usr/local/bin/project-agent"]
