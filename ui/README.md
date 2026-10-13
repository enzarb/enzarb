# enzarb-ui

SvelteKit (adapter-node) app for enzarb. Remote functions (`*.remote.ts`) are the logic layer.

## Develop

```sh
cp .env.example .env     # see docs/contracts.md section 9 in the enzarb repo
npm run migrate          # applies migrations/*.sql, tracked in app._migrations
npm run dev              # loads .env if present
npm run check && npm run lint && npm test   # tests need Docker (testcontainers)
```

GitHub login needs an OAuth app whose callback URL is `$ENZARB_PUBLIC_URL/login/github/callback`.
The container image also carries `scripts/migrate.ts`; run it with `node scripts/migrate.ts` using the schema owner's `DATABASE_URL`.

`scripts/grants.sql` (role grants for `app_api`, the `enzarb_ui` revokes) is re-run on every migrate and is not tracked in `app._migrations`. Roles that do not exist yet are skipped with a NOTICE, so create the pgop Roles (`enzarb_ui`, `enzarb_sandbox_proxy`, `enzarb_workflows`) in `helm/` before the UI migrate Job; a later run picks up any that were late.

## End-to-end test

```sh
npx playwright install chromium   # once
npm run test:e2e                  # needs Docker (a Postgres testcontainer)
```

A Playwright smoke test (`e2e/chat.spec.ts`) opens the chat page, waits for it to connect, sends a message and checks the agent's reply. It runs against `vite dev` with an in-memory project store (`vite.e2e.config.ts`, `e2e/stubs/kube.ts`) and a signed-in session seeded by `e2e/serve.ts`. The sandbox websocket is faked in the browser (`e2e/fake-sandbox.ts`), so no cluster or GitHub is involved. Set `E2E_DATABASE_URL` to use your own Postgres instead of Docker.

## Deployment notes

- Behind the gateway the pod sees plain HTTP, and adapter-node derives the request origin (used by SvelteKit's CSRF check) from `X-Forwarded-Proto`; the image sets `PROTOCOL_HEADER=x-forwarded-proto`. The gateway must send that header.
- The About page (`/about`) shows deployed versions from the optional `ENZARB_COMPONENT_VERSIONS` env, JSON written by the chart: `{"chart":"0.4.1","components":{"enzarb-ui":"v0.9.0","enzarb-operator":"v0.7.2"}}`. Missing entries show as `unknown`. Release notes come from the `enzarb/enzarb` GitHub releases (one unauthenticated request for the newest 100, cached 10 minutes in memory), filtered per component by tag prefix `<dir>/v` (`ui/v0.9.0` matches deployed `v0.9.0`) and are omitted if GitHub is unreachable.
- Sessions are an httpOnly `enzarb_session` cookie; only the sha-256 of the token is stored in `app.sessions`.
