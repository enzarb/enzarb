`workflows/` holds verbatim copies of every `workflows/migrations/*.sql` in the monorepo (contracts §6).
`workflows-fixture.ts` applies them in filename order, so the test database is their cumulative
result (later migrations `CREATE OR REPLACE` earlier functions, e.g. `submit_action`).
`workflows-sql.sha256` records the hash of each migration; `testdata.spec.ts` fails if a copy
drifts from it or a migration is missing. To refresh after the migrations change, run
`scripts/sync-workflows-sql.sh [path/to/enzarb-workflows [git-ref]]` (default: sibling checkout;
pass e.g. `origin/main` to read a git ref) and commit the result. Grants blocks skip absent roles.
