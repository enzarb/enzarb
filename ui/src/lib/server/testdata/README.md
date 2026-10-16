`engine-fixture.ts` applies the real engine v2 schemas (`../../../../../workflows/migrations/*.sql`,
docs/workflows/contracts-v2.md) to a test Postgres, after creating the runtime roles their grants
target, and seeds tasks, step runs, session events and resolutions (`engine.resolutions` holds the
board and workflow view that `board_v` / `workflow_v` read).
