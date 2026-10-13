#!/usr/bin/env bash
# Pin base/gvisor-installer to the digest of the image CI pushed for a tag.
# Usage: hack/pin-installer-digest.sh [tag]   (default: main)
# Resolves ghcr.io/enzarb/enzarb-gvisor-installer:<tag> to a sha256 digest and
# rewrites the `images:` entry in base/gvisor-installer/kustomization.yaml to
# `digest:` (dropping `newTag:`). Run after the image CI job has pushed, commit the
# result. Needs docker (buildx) and pull access to the package.
set -euo pipefail
cd "$(dirname "$0")/.."
TAG=${1:-main}
IMAGE=ghcr.io/enzarb/enzarb-gvisor-installer
KUSTOMIZATION=base/gvisor-installer/kustomization.yaml

digest=$(docker buildx imagetools inspect "$IMAGE:$TAG" --format '{{.Manifest.Digest}}')
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "could not resolve $IMAGE:$TAG (got '$digest')" >&2; exit 1; }

# Replace whichever of newTag/digest is present under the image entry.
sed -i -E "s#^( +)(newTag|digest): .*\$#\1digest: $digest#" "$KUSTOMIZATION"
grep -q "digest: $digest" "$KUSTOMIZATION" || { echo "failed to update $KUSTOMIZATION" >&2; exit 1; }
echo "pinned $IMAGE@$digest (from :$TAG) in $KUSTOMIZATION"
