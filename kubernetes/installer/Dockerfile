# gVisor installer image for base/gvisor-installer. Build from the repo root:
#   set -a; source hack/versions.env; set +a; docker buildx build -f installer/Dockerfile \
#     --build-arg GVISOR_VERSION --build-arg GVISOR_SHA512_X86_64 --build-arg GVISOR_SHA512_AARCH64 .
# The tarball is downloaded and sha512-verified at build time only; the final
# image needs no network access and carries no package manager state.
ARG ALPINE=alpine:3.22@sha256:5291449c3df73caf6ed85e649dec1b9e818b39a5d8c871e97afc13e9cd5e8fa8

FROM --platform=$BUILDPLATFORM ${ALPINE} AS fetch
ARG TARGETARCH
ARG GVISOR_VERSION
ARG GVISOR_SHA512_X86_64
ARG GVISOR_SHA512_AARCH64
RUN apk add --no-cache curl bzip2
WORKDIR /work
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64; want=$GVISOR_SHA512_X86_64 ;; \
      arm64) arch=aarch64; want=$GVISOR_SHA512_AARCH64 ;; \
      *) echo "unsupported arch $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    [ -n "$GVISOR_VERSION" ] && [ -n "$want" ] || { echo "missing GVISOR_* build args" >&2; exit 1; }; \
    curl -fsSL --retry 5 --retry-all-errors -o g.tar.bz2 "https://github.com/google/gvisor/releases/download/release-$GVISOR_VERSION/gvisor-$arch.tar.bz2"; \
    echo "$want  g.tar.bz2" | sha512sum -c -; \
    mkdir /gvisor; tar -xjf g.tar.bz2 -C /gvisor; rm g.tar.bz2; \
    test -x /gvisor/runsc && test -x /gvisor/containerd-shim-runsc-v1 && test -d /gvisor/gvisor-bin

FROM ${ALPINE}
ARG GVISOR_VERSION
# install.sh needs: sh/cp/cmp/grep/nsenter (busybox) and curl (node label PATCH).
RUN apk add --no-cache curl && rm -rf /var/cache/apk/*
COPY --from=fetch /gvisor /opt/gvisor
COPY hack/runsc.toml /opt/enzarb/runsc.toml
COPY --chmod=0755 installer/install.sh /usr/local/bin/install.sh
ENV GVISOR_VERSION=$GVISOR_VERSION
LABEL org.opencontainers.image.title="enzarb-gvisor-installer" \
      org.opencontainers.image.source="https://github.com/enzarb/enzarb" \
      org.opencontainers.image.version=$GVISOR_VERSION
ENTRYPOINT ["/usr/local/bin/install.sh"]
