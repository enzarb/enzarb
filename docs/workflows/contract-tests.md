# Contract tests and the registry

Contract tests pin the documented behaviour of each kernel action (`docs/workflows/contracts-v2.md`, `docs/contracts.md`, ADR-0021, ADR-0022) against `FakeConnector`. They live in `workflows/crates/engine/src/contract_tests/<action>.rs`, one `#[tokio::test]` per promise. Today: `create_issues`, `comment`, `open_pull_request`, `merge_pull_request`, `close_issue`, `update_branch`, `delete_branch`, `read_file` and the facts `ci`, `mergeable`, `reviews` and `dependencies`. A fact is registered as a `kind: action` entry plus one `kind: result_field` entry per field (`action` holds the fact name); `contract_tests/facts.rs` runs them through the helpers `actions::fact` calls.

## Registry

`workflows/crates/engine/tests/contract-registry.yaml` maps every action, argument, result field and prompt promise to the test(s) that cover it. `enzarb-lint contracts` reads it to require coverage (see "Library check").

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

## Library check

```
enzarb-lint contracts <library-dir> --registry workflows/crates/engine/tests/contract-registry.yaml [--tests-dir <dir>] [--format text|github]
```

It reads the library's workflow YAML (`phases/`, `steps/`, `workflows/`, also under `.enzarb/`) and prompts (`prompts/*.md`), and diffs them against the registry. It does not run the tests; `cargo test -p engine` does. The registry is also the list of kernel actions: `enzarb-flow` cannot depend on `engine`, so an action is "implemented" when it has a `kind: action` entry.

**What is extracted.** Each `action: {name, args}` gives an action and one arg per top-level key of `args` (`issues[].x` for item keys of a literal list). Result fields are references `result.<path>` or `last.result.<path>` in the same node, and `outputs.<as>.<path>` anywhere in the file when `<as>` names an action; indexes are normalised to `[]` (`result.issues[0].number` is `issues[].number`). Extraction is textual, so a reference reached dynamically (`result[field]`) is only reported as a warning.

**Prompt promises** are explicit markers, one per line, never scanned from prose:

```
<!-- promise: create_issues.title_format -->
```

`<action>.<id>` is the `action` and `name` of a `kind: prompt_promise` entry. Set that entry's `source` to the prompt path in the library (`prompts/breakdown.md`); entries whose `source` is not a prompt of the checked library are ignored by the carried-by-the-prompt rule, so the snapshot and `enzarb/workflows@v1` share one registry.

**Errors** (exit 1): a used action, arg, result field or marker with no registry entry; an entry with no `tests` or a test path that is not a `fn` under `--tests-dir` (default `<registry dir>/../src/contract_tests`); a `prompt_promise` whose `source` is a prompt of the library that does not carry the marker.

**Warnings** (exit 0): a used item that matches a `gaps` entry (printed with its reason; a documented gap excuses missing behaviour, not a missing entry for an item you do not list as a gap), and dynamic references. `--format github` prints them as `::warning`.

**Where the manifest lives.** In this repo, next to the contract tests, so the library repo needs no schema change and the test ids are checked beside the tests.

**CI** (`ci-workflows.yml`): `contracts-stdlib` runs the check on `workflows/crates/engine/tests/stdlib` and blocks. `contracts-v1` checks out `enzarb/workflows` at `v1` into `.lib/workflows` and runs the same command with `continue-on-error: true`, because the library prompts carry no markers yet. The library is private, so it is checked out with a GitHub App installation token (`actions/create-github-app-token`, `contents: read` on `enzarb/workflows`), not `GITHUB_TOKEN`; a failed token mint or checkout fails the job instead of skipping the lint, and the log prints `linting enzarb/workflows@v1 (<sha>)` before the result. Make it blocking (drop `continue-on-error`) once `v1` carries the markers.
