# Contract tests and the registry

Contract tests pin the documented behaviour of each kernel action (`docs/workflows/contracts-v2.md`, `docs/contracts.md`, ADR-0021, ADR-0022) against `FakeConnector`. They live in `workflows/crates/engine/src/contract_tests/<action>.rs`, one `#[tokio::test]` per promise. Today: `create_issues`, `comment`, `open_pull_request`, `merge_pull_request`, `close_issue`, `update_branch`, `delete_branch` and `read_file`.

## Registry

`workflows/crates/engine/tests/contract-registry.yaml` maps every action, argument, result field and prompt promise to the test(s) that cover it. A later check reads it to require coverage.

```yaml
version: 1
entries:
  - kind: action | arg | result_field | prompt_promise
    action: create_issues
    name: depends_on            # arg / result field (`issues[].number`) / promise id; the action name for kind=action
    source: docs/workflows/contracts-v2.md#kernel-actions   # doc, ADR or prompt the promise comes from
    tests: [contract_tests::create_issues::depends_on_recorded_as_blocked_by]
# optional, for a promise with no test yet:
gaps:
  - {kind: arg, action: x, name: y, reason: why}
```

- `tests` are `contract_tests::<file>::<fn>`, the path `cargo test -- --list` prints under the crate.
- Entries are unique by `(kind, action, name)` and sorted by `(action, kind, name)`. An entry without a test is not allowed: list it under `gaps` with a reason.
- Add a new action's entries in the PR that adds its contract tests. `contract_tests::registry` (run by `cargo test -p engine`) checks the schema, uniqueness, order and that each `tests` path is a test function in the source.

## Naming

One promise per test, named after it (`sub_issue_links_created`); cite the doc anchor in a comment on the `check_*` helper.

## Mutation checks

Tests are written as `check_*` helpers over the fake's final state. A `mutation_tests` module re-runs the action with one behaviour switched off and requires the helper to fail. The switches are `#[cfg(test)]` only (`actions::mutation`, a thread-local `Mutation` set with `activate(..)`, restored on drop); non-test builds see constant `false`. To cover a new behaviour, add a field to `Mutation`, guard the behaviour with it, and add a `removing_*_fails_the_contract` test.
