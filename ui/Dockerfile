# Build from the monorepo root (in-repo dep on proto/ via "file:../proto"):
#   docker build -f ui/Dockerfile .
FROM node:24-slim AS build
WORKDIR /repo/ui
# proto's TS package is built by ui's `prepare` script (npm run proto:build) during npm ci.
COPY proto/package.json proto/package-lock.json /repo/proto/
COPY proto/ts /repo/proto/ts
COPY ui/package.json ui/package-lock.json ui/.npmrc ./
RUN npm ci
# Built-in prompt templates, read by the UI at build time (src/lib/workflow/builtin-prompts.ts).
COPY workflows/crates/engine/prompts /repo/workflows/crates/engine/prompts
COPY ui/ .
RUN npm run build
RUN npm prune --omit=dev

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
