FROM ubuntu:24.04

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

# mise (tool version manager)
USER user
RUN curl https://mise.run | sh

# project-agent binary (built by GitHub Actions, copied at build time)
COPY --chown=user:user project-agent /home/user/.local/bin/project-agent

# docker credential helper for K8s SA token → registry auth
COPY --chown=user:user docker-credential-k8s-sa /home/user/.local/bin/docker-credential-k8s-sa

# Docker config pointing at credential helper
RUN mkdir -p /home/user/.docker && echo '{"credHelpers":{"registry.enzarb.dev":"k8s-sa"}}' \
    > /home/user/.docker/config.json

# Default kubectl config uses in-cluster SA token
RUN mkdir -p /home/user/.kube && printf '%s\n' \
    'apiVersion: v1' \
    'kind: Config' \
    'clusters:' \
    '- cluster:' \
    '    server: https://kubernetes.default.svc' \
    '    certificate-authority: /var/run/secrets/kubernetes.io/serviceaccount/ca.crt' \
    '  name: in-cluster' \
    'contexts:' \
    '- context:' \
    '    cluster: in-cluster' \
    '    user: sa' \
    '  name: in-cluster' \
    'current-context: in-cluster' \
    'users:' \
    '- name: sa' \
    '  user:' \
    '    tokenFile: /var/run/secrets/kubernetes.io/serviceaccount/token' \
    > /home/user/.kube/config

WORKDIR /home/user

ENV DOCKER_HOST=tcp://localhost:1234

ENTRYPOINT ["/usr/local/bin/tini", "--", "/home/user/.local/bin/project-agent"]
