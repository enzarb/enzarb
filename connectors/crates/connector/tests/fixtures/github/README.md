# GitHub response fixtures

Hand-written approximations of documented GitHub REST responses. There is no
network access, so none were recorded; the exact message text is not guaranteed.
Each file has a `_source` field naming the endpoint and the behaviour it models.
`FakeConnector` reproduces these behaviours and `tests/fake.rs` cites them.

| File | Models |
|---|---|
| `pull_mergeable_null.json` | `GET .../pulls/{n}` before GitHub has computed mergeability (`mergeable: null`, state `unknown`) |
| `pull_mergeable_known.json` | the same read after the computation finished |
| `sub_issue_duplicate_422.json` | `POST .../issues/{n}/sub_issues` for an existing link: 422 |
| `dependency_duplicate_422.json` | `POST .../issues/{n}/dependencies/blocked_by` for an existing edge: 422 |

The real connector absorbs the 422s into `Ok(())`, so the fake records them
(`rejected_duplicates()`) instead of returning an error.
