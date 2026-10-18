//! Generic connector actions and facts (contracts-v2, Connectors). Targets default to the task's
//! subjects; `args.number` (and `args.repo`) override. No workflow knowledge: these are building
//! blocks a workflow calls by name.

use enzarb_connector::file_issue::file_issue_key;
use enzarb_connector::{
    ChangeRequestSpec, CloseReason, Connector, Error as CError, MergeOutcome, RepoKey,
    UpdateBranchOutcome, WorkItemKey,
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
    dispatch(e.connector().as_ref(), repo, state, name, args, key).await
}

/// Runs the kernel action `name` against `c`. Split from `call` so the contract tests can drive
/// the actions with a `FakeConnector` and no `Engine`.
pub(crate) async fn dispatch(
    c: &dyn Connector,
    repo: &str,
    state: &TaskState,
    name: &str,
    args: &Map<String, Value>,
    key: &str,
) -> Result<Value, ActionError> {
    let task_branch = format!("enzarb/task-{}", state.doc.task.id);
    match name {
        "comment" => {
            let body = s(args, "body")
                .ok_or_else(|| ActionError::Refused("comment needs `body`".into()))?;
            let r = c
                .post_comment(&item(repo, state, args)?, body, &mutation::rekey(key))
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
            let it = c
                .open_change_request(spec, &mutation::rekey(key))
                .await
                .map_err(transient)?;
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
                None if mutation::ignore_readiness_head() => String::new(),
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
        "create_issues" => create_issues(c, &target_repo(repo, args), args, key).await,
        "file_issue" => file_issue(c, &target_repo(repo, args), args).await,
        "close_issue" => {
            let reason =
                if s(args, "reason") == Some("not_planned") && !mutation::ignore_close_reason() {
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
            let default = if mutation::wrong_branch_default() {
                state.doc.task.default_branch.clone()
            } else {
                task_branch
            };
            let b = s(args, "branch").map(str::to_string).unwrap_or(default);
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
        "ci" => ci_fact(c.as_ref(), &rk, head.as_deref()).await?,
        "reviews" => reviews_fact(c.as_ref(), pr_key.as_ref()).await?,
        "mergeable" => mergeable_fact(c.as_ref(), pr_key.as_ref(), MERGEABLE_POLL).await?,
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
            let deps =
                dependencies_fact(c.as_ref(), &WorkItemKey::new(rk.clone(), issue.number)).await?;
            // Record them as `blocked_by` relations first, so a dependency that ends after the
            // read below still notifies this task (`dependency_changed`) and the fact is
            // refreshed. Relations for dependencies that were removed go.
            let keys: Vec<String> = deps.iter().map(|d| d.to_string()).collect();
            sqlx::query(
                "DELETE FROM engine.relations WHERE task_id = $1 AND kind = 'blocked_by'
                 AND source = 'dependencies' AND NOT (target_key = ANY($2))",
            )
            .bind(task_id)
            .bind(&keys)
            .execute(&e.pool)
            .await
            .map_err(|e| e.to_string())?;
            for k in &keys {
                sqlx::query(
                    "INSERT INTO engine.relations (task_id, kind, target_key, source)
                     VALUES ($1, 'blocked_by', $2, 'dependencies') ON CONFLICT DO NOTHING",
                )
                .bind(task_id)
                .bind(k)
                .execute(&e.pool)
                .await
                .map_err(|e| e.to_string())?;
            }
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

/// `file_issue`: files one issue (`title`, `body`, optional `labels`) in `repo` under the
/// stable `key`. The first call creates it; later calls with the same key update that issue's
/// title and body, and do nothing when they are unchanged. The key is scoped to the repo, not
/// to the job, so it dedupes across runs.
pub(crate) async fn file_issue(
    c: &dyn Connector,
    r: &str,
    args: &Map<String, Value>,
) -> Result<Value, ActionError> {
    let key = s(args, "key").map(str::trim).unwrap_or("");
    if key.is_empty() {
        return Err(ActionError::Refused(
            "file_issue needs a non-empty `key`".into(),
        ));
    }
    let title = s(args, "title").map(str::trim).unwrap_or("");
    if title.is_empty() {
        return Err(ActionError::Refused("file_issue needs a `title`".into()));
    }
    let body = s(args, "body").unwrap_or("");
    let labels: Vec<String> = args
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let up = c
        .upsert_issue(&repo_key(r), title, body, &labels, &file_issue_key(r, key))
        .await
        .map_err(transient)?;
    Ok(json!({
        "kind": "issue",
        "repo": r,
        "number": up.item.number,
        "title": title,
        "url": format!("https://github.com/{r}/issues/{}", up.item.number),
        "key": key,
        "outcome": up.outcome.as_str(),
    }))
}

/// Test-only mutation switches for the kernel actions (`docs/workflows/contract-tests.md`): each
/// removes one documented behaviour so the contract tests can prove they notice. Compiled out
/// of non-test builds, where every switch is a constant `false`.
#[cfg(test)]
pub(crate) mod mutation {
    use std::cell::Cell;

    #[derive(Clone, Copy, Default)]
    pub(crate) struct Mutation {
        pub skip_sub_issue_link: bool,
        pub skip_dependencies: bool,
        pub skip_placeholders: bool,
        pub mergeable_known_on_first_read: bool,
        pub fresh_keys: bool,
        pub ignore_readiness_head: bool,
        pub wrong_branch_default: bool,
        pub ignore_close_reason: bool,
    }

    thread_local! {
        static ACTIVE: Cell<Mutation> = Cell::new(Mutation::default());
    }

    /// Restores the previous switches on drop.
    pub(crate) struct Guard(Mutation);

    pub(crate) fn activate(m: Mutation) -> Guard {
        Guard(ACTIVE.with(|a| a.replace(m)))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE.with(|a| a.set(self.0));
        }
    }

    pub(crate) fn skip_sub_issue_link() -> bool {
        ACTIVE.with(|a| a.get().skip_sub_issue_link)
    }
    pub(crate) fn skip_dependencies() -> bool {
        ACTIVE.with(|a| a.get().skip_dependencies)
    }
    pub(crate) fn skip_placeholders() -> bool {
        ACTIVE.with(|a| a.get().skip_placeholders)
    }
    pub(crate) fn mergeable_known_on_first_read() -> bool {
        ACTIVE.with(|a| a.get().mergeable_known_on_first_read)
    pub(crate) fn ignore_readiness_head() -> bool {
        ACTIVE.with(|a| a.get().ignore_readiness_head)
    }
    pub(crate) fn wrong_branch_default() -> bool {
        ACTIVE.with(|a| a.get().wrong_branch_default)
    }
    pub(crate) fn ignore_close_reason() -> bool {
        ACTIVE.with(|a| a.get().ignore_close_reason)
    }
    /// The idempotency key to pass on; with `fresh_keys`, a new one every time.
    pub(crate) fn rekey(key: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        if ACTIVE.with(|a| a.get().fresh_keys) {
            format!("{key}~{}", N.fetch_add(1, Ordering::Relaxed))
        } else {
            key.to_string()
        }
    }
}

#[cfg(not(test))]
mod mutation {
    pub(super) fn skip_sub_issue_link() -> bool {
        false
    }
    pub(super) fn skip_dependencies() -> bool {
        false
    }
    pub(super) fn skip_placeholders() -> bool {
        false
    }
    pub(super) fn mergeable_known_on_first_read() -> bool {
        false
    }
    pub(super) fn ignore_readiness_head() -> bool {
        false
    }
    pub(super) fn wrong_branch_default() -> bool {
        false
    }
    pub(super) fn ignore_close_reason() -> bool {
        false
    }
    pub(super) fn rekey(key: &str) -> String {
        key.to_string()
    }
}

fn resolve_refs(body: &str, by_key: &std::collections::HashMap<String, (u64, String)>) -> String {
    if mutation::skip_placeholders() {
        body.to_string()
    } else {
        resolve_issue_refs(body, by_key)
    }
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
        let body = with_footer(resolve_refs(&str_of(is, "body"), &by_key));
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
            let Some((n, _)) = by_key.get(&d).filter(|_| !mutation::skip_dependencies()) else {
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
        if let Some(p) = parent.filter(|_| !mutation::skip_sub_issue_link()) {
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
        let body = with_footer(resolve_refs(&str_of(is, "body"), &by_key));
        if body
            != with_footer(resolve_refs(
                &str_of(is, "body"),
                &prefix(&order, i, &keys, &by_key),
            ))
        {
            c.update_issue(&made[i], &body).await.map_err(transient)?;
        }
        for d in deps_of(is) {
            let Some((n, _)) = by_key.get(&d).filter(|_| !mutation::skip_dependencies()) else {
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
mod file_issue_tests {
    use super::*;
    use enzarb_connector::fake::FakeConnector;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    async fn file(c: &FakeConnector, v: Value) -> Value {
        match file_issue(c, "o/r", &args(v)).await {
            Ok(v) => v,
            Err(_) => panic!("file_issue failed"),
        }
    }

    #[tokio::test]
    async fn one_key_one_issue_and_a_new_key_a_second() {
        let c = FakeConnector::default();
        let a = file(&c, json!({"key": "k1", "title": "T", "body": "B"})).await;
        assert_eq!(a["outcome"], "created");
        assert_eq!(a["kind"], "issue");
        let b = file(&c, json!({"key": "k1", "title": "T", "body": "B"})).await;
        assert_eq!(b["outcome"], "unchanged");
        assert_eq!(b["number"], a["number"]);
        assert_eq!(c.created_issues().len(), 1);
        let d = file(&c, json!({"key": "k2", "title": "T", "body": "B"})).await;
        assert_eq!(d["outcome"], "created");
        assert_ne!(d["number"], a["number"]);
        assert_eq!(c.created_issues().len(), 2);
    }

    #[tokio::test]
    async fn changed_text_updates_the_same_issue() {
        let c = FakeConnector::default();
        let a = file(&c, json!({"key": "k1", "title": "T", "body": "B"})).await;
        let b = file(&c, json!({"key": "k1", "title": "T2", "body": "B2"})).await;
        assert_eq!(b["outcome"], "updated");
        assert_eq!(b["number"], a["number"]);
        let issues = c.created_issues();
        assert_eq!(issues.len(), 1);
        assert_eq!((issues[0].1.as_str(), issues[0].2.as_str()), ("T2", "B2"));
    }

    #[tokio::test]
    async fn a_missing_or_empty_key_is_refused() {
        let c = FakeConnector::default();
        for v in [
            json!({"title": "T"}),
            json!({"key": "  ", "title": "T"}),
            json!({"key": "k"}),
        ] {
            assert!(matches!(
                file_issue(&c, "o/r", &args(v)).await,
                Err(ActionError::Refused(_))
            ));
        }
        assert!(c.created_issues().is_empty());
    }
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

/// GitHub computes mergeability in the background: the first read starts it and reports
/// unknown (`None`). Read again a few times, so a conflict is seen instead of an unknown that
/// nothing would refresh (a conflicting PR runs no CI, so no CI event follows).
const MERGEABLE_POLL: &[std::time::Duration] = &[
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(3),
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(10),
];

/// The `ci` fact: the connector's check summary for the PR head `sha`, tagged with it; `null`
/// without a PR.
pub(crate) async fn ci_fact(
    c: &dyn Connector,
    rk: &RepoKey,
    sha: Option<&str>,
) -> Result<Value, String> {
    let Some(sha) = sha else {
        return Ok(Value::Null);
    };
    let mut v = c.ci_details(rk, sha).await.map_err(|e| e.to_string())?;
    if let Some(o) = v.as_object_mut() {
        o.insert("sha".into(), json!(sha));
    }
    Ok(v)
}

/// The `reviews` fact is the list (the connector wraps it as `{sha, reviews}`); `[]` without a PR.
pub(crate) async fn reviews_fact(
    c: &dyn Connector,
    pr: Option<&WorkItemKey>,
) -> Result<Value, String> {
    let Some(pr) = pr else {
        return Ok(json!([]));
    };
    let v = c.list_reviews(pr).await.map_err(|e| e.to_string())?;
    Ok(v.get("reviews").cloned().unwrap_or(v))
}

/// The `mergeable` fact: `true`/`false` once known, `null` while unknown or without a PR.
pub(crate) async fn mergeable_fact(
    c: &dyn Connector,
    pr: Option<&WorkItemKey>,
    delays: &[std::time::Duration],
) -> Result<Value, String> {
    match pr {
        Some(k) => Ok(json!(mergeable(c, k, delays).await?)),
        None => Ok(Value::Null),
    }
}

/// The blocked-by edges of `issue`. An error is "unknown", never "none".
pub(crate) async fn dependencies_fact(
    c: &dyn Connector,
    issue: &WorkItemKey,
) -> Result<Vec<WorkItemKey>, String> {
    c.list_dependencies(issue).await.map_err(|e| e.to_string())
}

pub(crate) async fn mergeable(
    c: &dyn Connector,
    k: &WorkItemKey,
    delays: &[std::time::Duration],
) -> Result<Option<bool>, String> {
    let first = c
        .get_merge_readiness(k, k.number)
        .await
        .map_err(|e| e.to_string())?;
    // Mutation: a fake that reports the first read as known (GitHub never does).
    let mut m = match (mutation::mergeable_known_on_first_read(), first.mergeable) {
        (true, None) if first.mergeable_state == "unknown" => Some(true),
        (_, m) => m,
    };
    for d in delays {
        if m.is_some() {
            break;
        }
        tokio::time::sleep(*d).await;
        m = c
            .get_merge_readiness(k, k.number)
            .await
            .map_err(|e| e.to_string())?
            .mergeable;
    }
    Ok(m)
}

#[cfg(test)]
mod mergeable_tests {
    use super::*;
    use enzarb_connector::{CiState, MergeReadiness};

    fn readiness(mergeable: Option<bool>) -> MergeReadiness {
        MergeReadiness {
            head_sha: "h".into(),
            base_sha: None,
            mergeable,
            mergeable_state: String::new(),
            ci: CiState::Pending,
        }
    }
    use enzarb_connector::fake::FakeConnector;

    #[tokio::test]
    async fn an_unknown_mergeable_is_read_again() {
        let c = FakeConnector::default();
        let k = WorkItemKey::new(repo_key("o/r"), 5);
        c.set_merge_readiness(k.clone(), readiness(None));
        let d = [std::time::Duration::from_millis(1); 2];
        // Still unknown after every read: unknown.
        assert_eq!(mergeable(&c, &k, &d).await.unwrap(), None);
        let reads = |c: &FakeConnector| {
            c.calls()
                .iter()
                .filter(|r| format!("{:?}", r.call).contains("MergeReadiness"))
                .count()
        };
        assert_eq!(reads(&c), 3);
        // Known: returned at once.
        c.set_merge_readiness_immediate(k.clone(), readiness(Some(false)));
        assert_eq!(mergeable(&c, &k, &d).await.unwrap(), Some(false));
        assert_eq!(reads(&c), 4);
        // Known only after GitHub computed it: the first read is unknown.
        c.set_merge_readiness(k.clone(), readiness(Some(true)));
        assert_eq!(mergeable(&c, &k, &d).await.unwrap(), Some(true));
        assert_eq!(reads(&c), 6);
    }
}
