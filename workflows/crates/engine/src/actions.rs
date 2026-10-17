//! Generic connector actions and facts (contracts-v2, Connectors). Targets default to the task's
//! subjects; `args.number` (and `args.repo`) override. No workflow knowledge: these are building
//! blocks a workflow calls by name.

use enzarb_connector::{
    ChangeRequestSpec, CloseReason, Connector, Error as CError, MergeOutcome, UpdateBranchOutcome,
    WorkItemKey,
};
use enzarb_flow::interp::TaskState;
use serde_json::{Map, Value, json};

use crate::engine::{Engine, repo_key, subject_key};

pub enum ActionError {
    /// The call was refused (bad arguments, a provider rule): `ok: false` to the workflow.
    Refused(String),
    /// Retry with backoff.
    Transient(String),
}

fn transient(e: CError) -> ActionError {
    match e {
        CError::Invalid(m) | CError::NotFound(m) => ActionError::Refused(m),
        other => ActionError::Transient(other.to_string()),
    }
}

fn s<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn target_repo(repo: &str, args: &Map<String, Value>) -> String {
    s(args, "repo").unwrap_or(repo).to_ascii_lowercase()
}

/// The issue (or, with `on: pull_request`, the pull request) an action addresses.
fn item(
    repo: &str,
    state: &TaskState,
    args: &Map<String, Value>,
) -> Result<WorkItemKey, ActionError> {
    let r = target_repo(repo, args);
    if let Some(n) = args.get("number").and_then(Value::as_u64) {
        return Ok(WorkItemKey::new(repo_key(&r), n));
    }
    let subj = &state.doc.subjects;
    let n = if s(args, "on") == Some("pull_request") {
        pr_number(state)
    } else {
        subj.issue
            .as_ref()
            .map(|i| i.number)
            .or_else(|| pr_number(state))
    };
    n.map(|n| WorkItemKey::new(repo_key(&r), n))
        .ok_or_else(|| ActionError::Refused("no subject to act on (pass `number`)".into()))
}

fn pr_number(state: &TaskState) -> Option<u64> {
    let prs = &state.doc.subjects.pull_requests;
    prs.iter()
        .rev()
        .find(|p| p.state == "open")
        .or(prs.last())
        .map(|p| p.number)
}

fn pr(
    repo: &str,
    state: &TaskState,
    args: &Map<String, Value>,
) -> Result<WorkItemKey, ActionError> {
    let r = target_repo(repo, args);
    args.get("number")
        .and_then(Value::as_u64)
        .or_else(|| pr_number(state))
        .map(|n| WorkItemKey::new(repo_key(&r), n))
        .ok_or_else(|| ActionError::Refused("no pull request (pass `number`)".into()))
}

fn labels(args: &Map<String, Value>) -> Vec<String> {
    match (args.get("labels"), s(args, "label")) {
        (Some(Value::Array(a)), _) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        (_, Some(l)) => vec![l.to_string()],
        _ => Vec::new(),
    }
}

pub async fn call(
    e: &Engine,
    repo: &str,
    state: &TaskState,
    name: &str,
    args: &Map<String, Value>,
    key: &str,
) -> Result<Value, ActionError> {
    let c = e.connector();
    let task_branch = format!("enzarb/task-{}", state.doc.task.id);
    match name {
        "comment" => {
            let body = s(args, "body")
                .ok_or_else(|| ActionError::Refused("comment needs `body`".into()))?;
            let r = c
                .post_comment(&item(repo, state, args)?, body, key)
                .await
                .map_err(transient)?;
            Ok(json!({"id": r.id, "url": r.url}))
        }
        "label" | "unlabel" => {
            let it = item(repo, state, args)?;
            for l in labels(args) {
                if name == "label" {
                    c.add_label(&it, &l).await.map_err(transient)?;
                } else {
                    c.remove_label(&it, &l).await.map_err(transient)?;
                }
            }
            Ok(json!({}))
        }
        "open_pull_request" => {
            let r = target_repo(repo, args);
            let base = s(args, "base")
                .map(str::to_string)
                .unwrap_or_else(|| state.doc.task.default_branch.clone());
            let spec = ChangeRequestSpec {
                repo: repo_key(&r),
                title: s(args, "title").unwrap_or("").to_string(),
                body: s(args, "body").unwrap_or("").to_string(),
                head_ref: s(args, "head").map(str::to_string).unwrap_or(task_branch),
                base_ref: base,
                draft: args.get("draft").and_then(Value::as_bool).unwrap_or(false),
            };
            let title = spec.title.clone();
            let it = c.open_change_request(spec, key).await.map_err(transient)?;
            // `subject` makes the engine link the pull request to the task.
            Ok(
                json!({"kind": "pull_request", "repo": r, "number": it.number, "title": title,
                      "url": it.url,
                      "subject": {"kind": "pull_request", "repo": r, "number": it.number, "url": it.url}}),
            )
        }
        "update_pull_request" => {
            c.update_pull_request(&pr(repo, state, args)?, s(args, "title"), s(args, "body"))
                .await
                .map_err(transient)?;
            Ok(json!({}))
        }
        "merge_pull_request" => {
            let it = pr(repo, state, args)?;
            let head = match s(args, "expected_head") {
                Some(h) => h.to_string(),
                None => {
                    c.get_merge_readiness(&it, it.number)
                        .await
                        .map_err(transient)?
                        .head_sha
                }
            };
            let title = s(args, "title").unwrap_or("").to_string();
            let out = c
                .merge_change_request(&it, it.number, &head, &title, s(args, "body"), key)
                .await
                .map_err(transient)?;
            match out {
                MergeOutcome::Merged { sha } => Ok(json!({"merged": true, "sha": sha})),
                MergeOutcome::AlreadyMerged { sha } => {
                    Ok(json!({"merged": true, "sha": sha, "already": true}))
                }
                MergeOutcome::HeadMoved => Err(ActionError::Refused("head moved".into())),
                MergeOutcome::NotMergeable { reason } => Err(ActionError::Refused(reason)),
            }
        }
        "create_issues" => create_issues(c.as_ref(), &target_repo(repo, args), args, key).await,
        "close_issue" => {
            let reason = if s(args, "reason") == Some("not_planned") {
                CloseReason::NotPlanned
            } else {
                CloseReason::Completed
            };
            c.close_issue(&item(repo, state, args)?, reason)
                .await
                .map_err(transient)?;
            Ok(json!({}))
        }
        "set_base" => {
            let it = pr(repo, state, args)?;
            let base = s(args, "base")
                .ok_or_else(|| ActionError::Refused("set_base needs `base`".into()))?;
            c.set_base(&it, it.number, base, key)
                .await
                .map_err(transient)?;
            Ok(json!({}))
        }
        "update_branch" => {
            let it = pr(repo, state, args)?;
            let head = match s(args, "expected_head") {
                Some(h) => h.to_string(),
                None => {
                    c.get_merge_readiness(&it, it.number)
                        .await
                        .map_err(transient)?
                        .head_sha
                }
            };
            match c
                .update_branch(&it, it.number, &head)
                .await
                .map_err(transient)?
            {
                UpdateBranchOutcome::Updated => Ok(json!({"updated": true})),
                UpdateBranchOutcome::Refused { reason } => Err(ActionError::Refused(reason)),
            }
        }
        "delete_branch" => {
            let r = target_repo(repo, args);
            let b = s(args, "branch").map(str::to_string).unwrap_or(task_branch);
            c.delete_branch(&repo_key(&r), &b, key)
                .await
                .map_err(transient)?;
            Ok(json!({}))
        }
        "read_file" => {
            let r = target_repo(repo, args);
            let path = s(args, "path")
                .ok_or_else(|| ActionError::Refused("read_file needs `path`".into()))?;
            let rf = s(args, "ref").unwrap_or(&state.doc.task.default_branch);
            let text = c
                .get_file(&repo_key(&r), rf, path)
                .await
                .map_err(transient)?;
            Ok(json!({"content": text}))
        }
        "resolve_ref" => {
            let r = target_repo(repo, args);
            let rf = s(args, "ref")
                .ok_or_else(|| ActionError::Refused("resolve_ref needs `ref`".into()))?;
            Ok(json!({"sha": c.resolve_ref(&repo_key(&r), rf).await.map_err(transient)?}))
        }
        other => Err(ActionError::Refused(format!("unknown action `{other}`"))),
    }
}

/// Refreshes one verified fact from GitHub (never from agent output).
pub async fn fact(
    e: &Engine,
    task_id: i64,
    repo: &str,
    state: &TaskState,
    name: &str,
) -> Result<Value, String> {
    let c = e.connector();
    let rk = repo_key(repo);
    let pr_key = pr_number(state).map(|n| WorkItemKey::new(rk.clone(), n));
    let head = match &pr_key {
        Some(k) => Some(
            c.get_merge_readiness(k, k.number)
                .await
                .map_err(|e| e.to_string())?
                .head_sha,
        ),
        None => None,
    };
    Ok(match name {
        "git" => {
            let branch = state
                .doc
                .subjects
                .pull_requests
                .iter()
                .rev()
                .find_map(|p| p.head.clone())
                .unwrap_or_else(|| format!("enzarb/task-{task_id}"));
            let h = c
                .branch_head(&rk, &branch)
                .await
                .map_err(|e| e.to_string())?;
            let ahead = match &h {
                Some(h) => c
                    .ahead_by(&rk, &state.doc.task.default_branch, h)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(0),
                None => 0,
            };
            json!({"branch": branch, "head": h.clone().unwrap_or_default(), "pushed": h.is_some(), "ahead": ahead})
        }
        "ci" => match head {
            Some(sha) => {
                let mut v = c.ci_details(&rk, &sha).await.map_err(|e| e.to_string())?;
                if let Some(o) = v.as_object_mut() {
                    o.insert("sha".into(), json!(sha));
                }
                v
            }
            None => Value::Null,
        },
        "reviews" => match &pr_key {
            Some(k) => {
                let v = c.list_reviews(k).await.map_err(|e| e.to_string())?;
                // The interpreter's `reviews` fact is the list.
                v.get("reviews").cloned().unwrap_or(v)
            }
            None => json!([]),
        },
        "mergeable" => match &pr_key {
            Some(k) => json!(
                c.get_merge_readiness(k, k.number)
                    .await
                    .map_err(|e| e.to_string())?
                    .mergeable
            ),
            None => Value::Null,
        },
        "merged" => json!(
            state
                .doc
                .subjects
                .pull_requests
                .iter()
                .any(|p| p.state == "merged")
        ),
        // Set when a release-like event whose ref contains the merge commit is routed to the
        // task; a refresh keeps the verified value.
        "contained_in_tag" => json!(state.doc.facts.contained_in_tag),
        "dependencies" => {
            let Some(issue) = &state.doc.subjects.issue else {
                return Ok(json!([]));
            };
            // An error is "unknown", never "none".
            let deps = c
                .list_dependencies(&WorkItemKey::new(rk.clone(), issue.number))
                .await
                .map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for d in deps {
                let key = d.to_string();
                let row: Option<(i64, Option<chrono::DateTime<chrono::Utc>>, String)> =
                    sqlx::query_as(
                        "SELECT t.id, t.ended_at, COALESCE(p.end_status, p.status, 'unknown')
                     FROM engine.subjects s JOIN engine.tasks t ON t.id = s.task_id
                     LEFT JOIN engine.projection p ON p.task_id = t.id
                     WHERE s.key = $1 ORDER BY s.ended, s.linked_at DESC LIMIT 1",
                    )
                    .bind(&key)
                    .fetch_optional(&e.pool)
                    .await
                    .map_err(|e| e.to_string())?;
                out.push(match row {
                    Some((id, ended, st)) => {
                        json!({"task": id.to_string(), "status": st, "ended": ended.is_some()})
                    }
                    None => json!({"task": key, "status": "untracked", "ended": false}),
                });
            }
            json!(out)
        }
        other => return Err(format!("unknown fact `{other}`")),
    })
}

/// `create_issues`: creates `issues` ([{key, title, body, labels?, depends_on?}]) in `repo`,
/// then for each one replaces `{{issue:<key>}}` in its body with `#<number> (<title>)`, appends
/// `footer`, links it as a sub-issue of `parent` and records its `depends_on` keys as
/// blocked-by relations. Every call is idempotent, so a retried job converges.
/// Returns `{issues: [{kind, key, repo, number, title, url}]}` in input order.
pub(crate) async fn create_issues(
    c: &dyn Connector,
    r: &str,
    args: &Map<String, Value>,
    key: &str,
) -> Result<Value, ActionError> {
    let issues = args
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let parent = match args.get("parent") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(t)) => t.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    };
    let footer = s(args, "footer").unwrap_or("").trim();
    let with_footer = |body: String| {
        if footer.is_empty() {
            body
        } else {
            format!("{}\n\n{footer}", body.trim_end())
        }
    };
    let keys: Vec<String> = issues.iter().map(|is| str_of(is, "key")).collect();
    let deps_of = |is: &Value| -> Vec<String> {
        is.get("depends_on")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    // Each issue is created complete as far as possible: dependencies first, so its body
    // already names them, and its sub-issue link and blocked-by relations follow at once. Its
    // task starts from the `opened` webhook, and its `dependencies` phase must see them.
    let order = topo_order(&keys, &issues.iter().map(deps_of).collect::<Vec<_>>());
    let mut made: Vec<Option<WorkItemKey>> = vec![None; issues.len()];
    let mut by_key: std::collections::HashMap<String, (u64, String)> = Default::default();
    for &i in &order {
        let is = &issues[i];
        let ls: Vec<String> = is
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let body = with_footer(resolve_issue_refs(&str_of(is, "body"), &by_key));
        let k = c
            .create_issue(
                &repo_key(r),
                &str_of(is, "title"),
                &body,
                &ls,
                &format!("{key}:{i}"),
            )
            .await
            .map_err(transient)?;
        if !keys[i].is_empty() {
            by_key.insert(keys[i].clone(), (k.number, str_of(is, "title")));
        }
        for d in deps_of(is) {
            let Some((n, _)) = by_key.get(&d) else {
                continue;
            };
            if *n == k.number {
                continue;
            }
            c.add_dependency(
                &k,
                &WorkItemKey::new(repo_key(r), *n),
                &format!("{key}:{i}:dep:{d}"),
            )
            .await
            .map_err(transient)?;
        }
        if let Some(p) = parent {
            c.link_sub_issue(&WorkItemKey::new(repo_key(r), p), &k)
                .await
                .map_err(transient)?;
        }
        made[i] = Some(k);
    }
    let made: Vec<WorkItemKey> = made
        .into_iter()
        .map(|k| k.expect("every issue created"))
        .collect();
    // References to issues created later (and dependencies in a cycle) are filled in now.
    for (i, is) in issues.iter().enumerate() {
        let body = with_footer(resolve_issue_refs(&str_of(is, "body"), &by_key));
        if body
            != with_footer(resolve_issue_refs(
                &str_of(is, "body"),
                &prefix(&order, i, &keys, &by_key),
            ))
        {
            c.update_issue(&made[i], &body).await.map_err(transient)?;
        }
        for d in deps_of(is) {
            let Some((n, _)) = by_key.get(&d) else {
                continue;
            };
            let before = order.iter().position(|&j| j == i).unwrap_or(0);
            let created_earlier = order[..before].iter().any(|&j| keys[j] == d);
            if *n != made[i].number && !created_earlier {
                c.add_dependency(
                    &made[i],
                    &WorkItemKey::new(repo_key(r), *n),
                    &format!("{key}:{i}:dep:{d}"),
                )
                .await
                .map_err(transient)?;
            }
        }
    }
    let out: Vec<Value> = issues
        .iter()
        .zip(&made)
        .map(|(is, k)| {
            json!({"kind": "issue", "repo": r, "number": k.number, "title": str_of(is, "title"),
                   "url": format!("https://github.com/{r}/issues/{}", k.number),
                   "key": subject_key(r, k.number)})
        })
        .collect();
    Ok(json!({"issues": out}))
}

/// Indices in dependency order (a dependency before its dependents), stable otherwise;
/// members of a cycle and unknown keys keep their input order.
fn topo_order(keys: &[String], deps: &[Vec<String>]) -> Vec<usize> {
    let mut done = vec![false; keys.len()];
    let mut out = Vec::with_capacity(keys.len());
    while out.len() < keys.len() {
        let ready = (0..keys.len()).find(|&i| {
            !done[i]
                && deps[i].iter().all(|d| {
                    d == &keys[i] || !keys.iter().enumerate().any(|(j, k)| k == d && !done[j])
                })
        });
        // A cycle: take the first remaining issue.
        let i = ready.unwrap_or_else(|| (0..keys.len()).find(|&i| !done[i]).unwrap());
        done[i] = true;
        out.push(i);
    }
    out
}

/// The keys created before issue `i` (what its body was resolved against at creation).
fn prefix(
    order: &[usize],
    i: usize,
    keys: &[String],
    by_key: &std::collections::HashMap<String, (u64, String)>,
) -> std::collections::HashMap<String, (u64, String)> {
    let at = order.iter().position(|&j| j == i).unwrap_or(0);
    order[..at]
        .iter()
        .filter_map(|&j| by_key.get(&keys[j]).map(|v| (keys[j].clone(), v.clone())))
        .collect()
}

/// Replaces `{{issue:<key>}}` (spaces inside the braces allowed) with `#<number> (<title>)`;
/// unknown keys are left as written.
fn resolve_issue_refs(
    body: &str,
    by_key: &std::collections::HashMap<String, (u64, String)>,
) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start..].find("}}") else {
            break;
        };
        let inner = rest[start + 2..start + len].trim();
        out.push_str(&rest[..start]);
        match inner
            .strip_prefix("issue:")
            .map(str::trim)
            .and_then(|k| by_key.get(k))
        {
            Some((n, t)) => out.push_str(&format!("#{n} ({t})")),
            None => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod create_issues_tests {
    use super::*;
    use enzarb_connector::fake::FakeConnector;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn resolves_refs_appends_the_footer_links_and_records_dependencies() {
        let c = FakeConnector::default();
        c.set_next_issue_number(10);
        let a = args(json!({
            "parent": "7",
            "footer": "Part of the epic #7",
            "issues": [
                {"key": "ci", "title": "CI job", "body": "Uses {{issue:gen}} and {{ issue:nope }}.",
                 "depends_on": ["gen", "missing"]},
                {"key": "gen", "title": "Generator", "body": "Builds the graph; see {{issue:ci}}.", "depends_on": []}
            ]
        }));
        let out = match create_issues(&c, "o/r", &a, "t1:call-4").await {
            Ok(v) => v,
            Err(_) => panic!("create_issues failed"),
        };
        // The dependency ("gen") is created first; the result keeps the input order.
        assert_eq!(out["issues"][0]["number"], 11);
        assert_eq!(out["issues"][1]["number"], 10);
        let created = c.created_issues();
        assert_eq!(
            created[0].2,
            "Builds the graph; see #11 (CI job).\n\nPart of the epic #7"
        );
        assert_eq!(
            created[1].2,
            "Uses #10 (Generator) and {{ issue:nope }}.\n\nPart of the epic #7"
        );
        // The dependent was created with its final body, and its dependency was recorded
        // right after it, before anything else.
        let calls: Vec<String> = c.calls().iter().map(|r| format!("{:?}", r.call)).collect();
        let create_ci = calls
            .iter()
            .position(|c| c.contains("CreateIssue") && c.contains("CI job"))
            .unwrap();
        assert!(
            calls[create_ci].contains("Uses #10 (Generator)"),
            "{}",
            calls[create_ci]
        );
        assert!(
            calls[create_ci + 1].contains("AddDependency"),
            "{}",
            calls[create_ci + 1]
        );
        let parent = WorkItemKey::new(repo_key("o/r"), 7);
        assert_eq!(
            c.sub_issues(&parent),
            vec![created[0].0.clone(), created[1].0.clone()]
        );
        let deps = c.list_dependencies(&created[1].0).await.unwrap();
        assert_eq!(deps, vec![created[0].0.clone()]);
        assert!(c.list_dependencies(&created[0].0).await.unwrap().is_empty());

        // A retried job converges: no new issues, same bodies, links and dependencies.
        let again = create_issues(&c, "o/r", &a, "t1:call-4")
            .await
            .ok()
            .unwrap();
        assert_eq!(again, out);
        assert_eq!(c.created_issues(), created);
        assert_eq!(c.sub_issues(&parent).len(), 2);
        assert_eq!(c.list_dependencies(&created[1].0).await.unwrap().len(), 1);
    }

    #[test]
    fn dependencies_come_first_and_cycles_keep_input_order() {
        let k = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            topo_order(&k(&["a", "b", "c"]), &[k(&["c"]), k(&[]), k(&["b"])]),
            vec![1, 2, 0]
        );
        assert_eq!(
            topo_order(&k(&["a", "b"]), &[k(&["b"]), k(&["a"])]),
            vec![0, 1]
        );
        assert_eq!(topo_order(&k(&["a"]), &[k(&["a", "zz"])]), vec![0]);
    }

    #[test]
    fn unterminated_braces_are_kept() {
        let m = std::collections::HashMap::from([("a".to_string(), (1, "A".to_string()))]);
        assert_eq!(
            resolve_issue_refs("x {{issue:a}} {{ y", &m),
            "x #1 (A) {{ y"
        );
    }
}
