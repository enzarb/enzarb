//! Generic connector actions and facts (contracts-v2, Connectors). Targets default to the task's
//! subjects; `args.number` (and `args.repo`) override. No workflow knowledge: these are building
//! blocks a workflow calls by name.

use enzarb_connector::{ChangeRequestSpec, CloseReason, Error as CError, MergeOutcome, UpdateBranchOutcome, WorkItemKey};
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
fn item(repo: &str, state: &TaskState, args: &Map<String, Value>) -> Result<WorkItemKey, ActionError> {
    let r = target_repo(repo, args);
    if let Some(n) = args.get("number").and_then(Value::as_u64) {
        return Ok(WorkItemKey::new(repo_key(&r), n));
    }
    let subj = &state.doc.subjects;
    let n = if s(args, "on") == Some("pull_request") {
        pr_number(state)
    } else {
        subj.issue.as_ref().map(|i| i.number).or_else(|| pr_number(state))
    };
    n.map(|n| WorkItemKey::new(repo_key(&r), n))
        .ok_or_else(|| ActionError::Refused("no subject to act on (pass `number`)".into()))
}

fn pr_number(state: &TaskState) -> Option<u64> {
    let prs = &state.doc.subjects.pull_requests;
    prs.iter().rev().find(|p| p.state == "open").or(prs.last()).map(|p| p.number)
}

fn pr(repo: &str, state: &TaskState, args: &Map<String, Value>) -> Result<WorkItemKey, ActionError> {
    let r = target_repo(repo, args);
    args.get("number")
        .and_then(Value::as_u64)
        .or_else(|| pr_number(state))
        .map(|n| WorkItemKey::new(repo_key(&r), n))
        .ok_or_else(|| ActionError::Refused("no pull request (pass `number`)".into()))
}

fn labels(args: &Map<String, Value>) -> Vec<String> {
    match (args.get("labels"), s(args, "label")) {
        (Some(Value::Array(a)), _) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
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
            let body = s(args, "body").ok_or_else(|| ActionError::Refused("comment needs `body`".into()))?;
            let r = c.post_comment(&item(repo, state, args)?, body, key).await.map_err(transient)?;
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
            let base = s(args, "base").map(str::to_string).unwrap_or_else(|| state.doc.task.default_branch.clone());
            let spec = ChangeRequestSpec {
                repo: repo_key(&r),
                title: s(args, "title").unwrap_or("").to_string(),
                body: s(args, "body").unwrap_or("").to_string(),
                head_ref: s(args, "head").map(str::to_string).unwrap_or(task_branch),
                base_ref: base,
                draft: args.get("draft").and_then(Value::as_bool).unwrap_or(false),
            };
            let it = c.open_change_request(spec, key).await.map_err(transient)?;
            Ok(json!({"number": it.number, "url": it.url,
                      "subject": {"kind": "pull_request", "repo": r, "number": it.number, "url": it.url}}))
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
                None => c.get_merge_readiness(&it, it.number).await.map_err(transient)?.head_sha,
            };
            let title = s(args, "title").unwrap_or("").to_string();
            let out = c
                .merge_change_request(&it, it.number, &head, &title, s(args, "body"), key)
                .await
                .map_err(transient)?;
            match out {
                MergeOutcome::Merged { sha } => Ok(json!({"merged": true, "sha": sha})),
                MergeOutcome::AlreadyMerged { sha } => Ok(json!({"merged": true, "sha": sha, "already": true})),
                MergeOutcome::HeadMoved => Err(ActionError::Refused("head moved".into())),
                MergeOutcome::NotMergeable { reason } => Err(ActionError::Refused(reason)),
            }
        }
        "create_issues" => {
            let r = target_repo(repo, args);
            let mut out = Vec::new();
            let issues = args.get("issues").and_then(Value::as_array).cloned().unwrap_or_default();
            for (i, is) in issues.iter().enumerate() {
                let title = is.get("title").and_then(Value::as_str).unwrap_or("");
                let body = is.get("body").and_then(Value::as_str).unwrap_or("");
                let ls: Vec<String> = is.get("labels").and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let k = c.create_issue(&repo_key(&r), title, body, &ls, &format!("{key}:{i}")).await.map_err(transient)?;
                out.push(json!({"number": k.number, "repo": r, "key": subject_key(&r, k.number)}));
            }
            Ok(json!({"issues": out}))
        }
        "close_issue" => {
            let reason = if s(args, "reason") == Some("not_planned") { CloseReason::NotPlanned } else { CloseReason::Completed };
            c.close_issue(&item(repo, state, args)?, reason).await.map_err(transient)?;
            Ok(json!({}))
        }
        "set_base" => {
            let it = pr(repo, state, args)?;
            let base = s(args, "base").ok_or_else(|| ActionError::Refused("set_base needs `base`".into()))?;
            c.set_base(&it, it.number, base, key).await.map_err(transient)?;
            Ok(json!({}))
        }
        "update_branch" => {
            let it = pr(repo, state, args)?;
            let head = match s(args, "expected_head") {
                Some(h) => h.to_string(),
                None => c.get_merge_readiness(&it, it.number).await.map_err(transient)?.head_sha,
            };
            match c.update_branch(&it, it.number, &head).await.map_err(transient)? {
                UpdateBranchOutcome::Updated => Ok(json!({"updated": true})),
                UpdateBranchOutcome::Refused { reason } => Err(ActionError::Refused(reason)),
            }
        }
        "delete_branch" => {
            let r = target_repo(repo, args);
            let b = s(args, "branch").map(str::to_string).unwrap_or(task_branch);
            c.delete_branch(&repo_key(&r), &b, key).await.map_err(transient)?;
            Ok(json!({}))
        }
        "read_file" => {
            let r = target_repo(repo, args);
            let path = s(args, "path").ok_or_else(|| ActionError::Refused("read_file needs `path`".into()))?;
            let rf = s(args, "ref").unwrap_or(&state.doc.task.default_branch);
            let text = c.get_file(&repo_key(&r), rf, path).await.map_err(transient)?;
            Ok(json!({"content": text}))
        }
        "resolve_ref" => {
            let r = target_repo(repo, args);
            let rf = s(args, "ref").ok_or_else(|| ActionError::Refused("resolve_ref needs `ref`".into()))?;
            Ok(json!({"sha": c.resolve_ref(&repo_key(&r), rf).await.map_err(transient)?}))
        }
        other => Err(ActionError::Refused(format!("unknown action `{other}`"))),
    }
}

/// Refreshes one verified fact from GitHub (never from agent output).
pub async fn fact(e: &Engine, task_id: i64, repo: &str, state: &TaskState, name: &str) -> Result<Value, String> {
    let c = e.connector();
    let rk = repo_key(repo);
    let pr_key = pr_number(state).map(|n| WorkItemKey::new(rk.clone(), n));
    let head = match &pr_key {
        Some(k) => Some(c.get_merge_readiness(k, k.number).await.map_err(|e| e.to_string())?.head_sha),
        None => None,
    };
    Ok(match name {
        "git" => {
            let branch = state.doc.subjects.pull_requests.iter().rev()
                .find_map(|p| p.head.clone())
                .unwrap_or_else(|| format!("enzarb/task-{task_id}"));
            let h = c.branch_head(&rk, &branch).await.map_err(|e| e.to_string())?;
            let ahead = match &h {
                Some(h) => c.ahead_by(&rk, &state.doc.task.default_branch, h).await.ok().flatten().unwrap_or(0),
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
            Some(k) => json!(c.get_merge_readiness(k, k.number).await.map_err(|e| e.to_string())?.mergeable),
            None => Value::Null,
        },
        "merged" => json!(state.doc.subjects.pull_requests.iter().any(|p| p.state == "merged")),
        "contained_in_tag" => Value::Null,
        "dependencies" => {
            let Some(issue) = &state.doc.subjects.issue else { return Ok(json!([])) };
            let deps = c.list_dependencies(&WorkItemKey::new(rk.clone(), issue.number)).await.unwrap_or_default();
            let mut out = Vec::new();
            for d in deps {
                let key = d.to_string();
                let row: Option<(i64, Option<chrono::DateTime<chrono::Utc>>, String)> = sqlx::query_as(
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
                    Some((id, ended, st)) => json!({"task": id.to_string(), "status": st, "ended": ended.is_some()}),
                    None => json!({"task": key, "status": "untracked", "ended": false}),
                });
            }
            json!(out)
        }
        other => return Err(format!("unknown fact `{other}`")),
    })
}
