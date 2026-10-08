`workflows_schema.sql` and `workflows_api.sql` are verbatim copies of
`enzarb-workflows/migrations/20261008000001_workflows_schema.sql` and `…02_workflows_api.sql`
(contracts §6). `workflows-sql.sha256` records the hashes of the source files; `testdata.spec.ts`
fails if a copy drifts from them. To refresh after the migrations change, run
`scripts/sync-workflows-sql.sh [path/to/enzarb-workflows]` (default: sibling checkout) and commit
the result. The grants block skips absent roles.
