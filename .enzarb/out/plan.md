**Changes from the previous version:** I took the maintainer's answers on board. The plan is otherwise unchanged.
- The `PR_SET_CHILD_SUBREAPER` fallback is dropped entirely. It is no longer a "next step", only a follow-up if a real failure shows up.
- The file-local `SERIAL` in the new binary stays, for plain `cargo test`.
- CI runs `cargo nextest run` (one process per test), so it never saw the flake. The bug only shows with plain `cargo test` (local runs, git hooks). The flake check now uses plain `cargo test`, not nextest.
- Removing the lib `SERIAL` and the unused `gone` helper is confirmed.

## Summary
Reap-all mode calls `waitpid(-1)`, which steals the exit status of any child in the process. Two tests in the `sandbox_init` lib test binary use `Reaper::new(true)`. Under plain `cargo test` they run in parallel with other lib tests that use `Command::output()` (`retarget.rs`, `agent.rs`, `docker_prune.rs`, `chat/tests.rs`), and those get ECHILD. The fix is to move the reap-all tests into their own integration-test binary, `sandbox-container/tests/reaper_all.rs`. Production code is unchanged.

## Approach
- Cargo builds each `tests/*.rs` as a separate binary and runs binaries one after another, so a reap-all test no longer shares a process with the lib tests. The existing `tests/*.rs` already use `Reaper::new(false)` through the public `sandbox_init` API, so no Cargo changes are needed.
- Inside the new binary, tests run in parallel by default and both reap-all tests call `waitpid(-1)`. They would steal each other's children, so they take a file-local `static SERIAL: Mutex<()>`. Rejected: merging them into one `#[test]`, which is less readable and gives worse failure output.
- `delivers_exit_status_of_registered_children` loops over `reap_all in [false, true]`. Split it. The `false` case stays in `src/reaper.rs`, because registered-only mode only waits on its own pids. The `true` case moves to the new binary.
- Rejected: the forked child with `PR_SET_CHILD_SUBREAPER`. Not needed, and CI is not gVisor, so it is left out entirely.
- No test-only reaper modes exist. `reap_all` is a production flag (`for_current_process`, `ENZARB_REAP_ALL`), so no `Reaper` API is removed. The only thing that becomes unnecessary is the lib `SERIAL` mutex and its comment, plus the `gone` helper if unused.

## Changes
- `sandbox-container/tests/reaper_all.rs` (new):
  - Module doc comment: this file must stay a separate binary, because reap-all steals every child's status and must not share a process with tests that spawn children.
  - `static SERIAL: Mutex<()>` and a `gone(pid)` helper (`kill(pid, None)` gives ESRCH).
  - `reap_all_reaps_unregistered_orphans`, moved as is.
  - `reap_all_delivers_exit_status_of_registered_children`: the `true` half of the old test, asserting `Exited(7)` and `registered() == 0`.
  - Imports follow `tests/mux.rs` (I will check the exact module path for `Reaper` and `ExitStatus` there).
- `sandbox-container/src/reaper.rs`, `#[cfg(test)] mod tests`:
  - Remove `reap_all_reaps_unregistered_orphans`, the `gone` helper (it is unused after the move), and the `SERIAL` static and its guards.
  - `delivers_exit_status_of_registered_children` becomes a single registered-only case, renamed `registered_only_delivers_exit_status`.
  - Keep `terminate_all_signals_registered_children` (uses `Reaper::new(false)`).
  - Fix imports so clippy reports no unused imports. The top-level `Mutex` import stays because `Reaper` uses it.
  - Update the module doc line "registered-only (everything else, e.g. tests)" to point at `tests/reaper_all.rs` for the reap-all tests.

## Tests
- Acceptance check: `grep -rn "Reaper::new(true)\|reaps_all" sandbox-container/src` should match no test code.
- In `sandbox-container/`: `cargo clippy --all-targets && cargo test`.
- Flake check with plain `cargo test` (where the bug lived), not nextest: `for i in $(seq 20); do cargo test 2>&1 | grep -E "ECHILD|No child processes|FAILED" && break; done`. I will report the actual outcome. A stress loop cannot prove the absence of flakes, and I will say so.
- Only `sandbox-container/` is touched. There is no Dockerfile, proto or Helm impact.

## Risks and open questions
- **Binary ordering:** relies on plain `cargo test` running test binaries sequentially, which is its default. Nextest gives each test its own process, so it is safe too.
- **Orphans in the new binary:** `waitpid(-1)` also reaps any child of the harness. The tests are serialised and the harness spawns no children, so `waitpid(-1)` only sees that test's own children.
- **Assumption:** "lib test binary has no reap-all `waitpid(-1)`" means no `src/` test drives `reap()` on a reap-all `Reaper`. The maintainer confirmed this.

## Out of scope
- Changes to `Reaper` production behaviour or API (`for_current_process`, `ENZARB_REAP_ALL`).
- Any `PR_SET_CHILD_SUBREAPER` / fork approach.
- Other test flakiness in `sandbox-container`, and CI config changes.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
