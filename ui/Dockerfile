# syntax=docker/dockerfile:1
# Build from the monorepo root (in-repo dep on proto/ via "link:../proto"):
#   docker build -f ui/Dockerfile .
FROM node:24-slim AS build
# pnpm at the version in package.json "packageManager" (corepack ships with node 24).
ENV PNPM_HOME=/pnpm COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable
WORKDIR /repo/ui
# proto's TS package is built by ui's `prepare` script (pnpm run proto:build) during pnpm install.
# BEGIN generated:copy
COPY proto/package.json proto/pnpm-lock.yaml proto/.npmrc /repo/proto/
COPY proto/ts /repo/proto/ts
# END generated:copy
COPY ui/package.json ui/pnpm-lock.yaml ui/.npmrc ./
RUN --mount=type=cache,id=pnpm-store,target=/pnpm/store \
    pnpm config set store-dir /pnpm/store --location=global && pnpm install --frozen-lockfile
COPY ui/ .
RUN pnpm run build
RUN --mount=type=cache,id=pnpm-store,target=/pnpm/store \
    pnpm prune --prod --ignore-scripts

FROM node:24-slim
ENV NODE_ENV=production PORT=3000 HOST=0.0.0.0 PROTOCOL_HEADER=x-forwarded-proto
# Same layout as the build stage: node_modules/@enzarb/proto is a symlink to ../../../proto.
WORKDIR /repo/ui
COPY --from=build --chown=node:node /repo/proto/package.json /repo/proto/package.json
COPY --from=build --chown=node:node /repo/proto/ts/dist /repo/proto/ts/dist
COPY --from=build --chown=node:node /repo/ui/build ./build
COPY --from=build --chown=node:node /repo/ui/node_modules ./node_modules
COPY --from=build --chown=node:node /repo/ui/package.json ./
# Migrations run from this image in the chart's pre-install Job: `node scripts/migrate.ts`.
COPY --from=build --chown=node:node /repo/ui/migrations ./migrations
COPY --from=build --chown=node:node /repo/ui/scripts ./scripts
USER node
EXPOSE 3000
CMD ["node", "build"]
