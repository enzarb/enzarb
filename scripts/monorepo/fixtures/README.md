# Fixtures for the monorepo script tests

## `imagetools-create-wrapped.json`

The output of `docker buildx imagetools create --dry-run <single-manifest image>`: an OCI image
index wrapping the source manifest. Used by the `docker` stub in `../test-retag-or-build.sh`.

**Source:** hand-written from the Docker documentation of `docker buildx imagetools create`
(<https://docs.docker.com/reference/cli/docker/buildx/imagetools/create/>, which describes
`--dry-run` as printing the manifest that would be pushed, and `--prefer-index`, which defaults
to wrapping a single manifest in an index) and the OCI image-index spec
(<https://github.com/opencontainers/image-spec/blob/main/image-index.md>, media type
`application/vnd.oci.image.index.v1+json`). It was **not** captured from a registry, so it matches
buildx only as far as those documents do.

Placeholders the stub fills in from the source manifest: `@TYPE@` (its media type), `@DIGEST@`
(its sha256) and `@SIZE@` (its size in bytes).
