//! GitHub webhook payload -> standard event catalog (`enzarb_flow::events`, ADR-0022 §5).
//!
//! Pure: no I/O. The engine fills `actor.can_write` and may enrich pull request
//! references with [`Connector::pr_references`](enzarb_connector::Connector::pr_references).

use chrono::{SecondsFormat, Utc};
use enzarb_connector::epic::parse_epic_marker;
use enzarb_flow::events::{
    Actor, CheckRun, CiStatus, Comment, Deployment, Event, Issue, Label, Package, Payload,
    PullRequest, Push, Release, Review, SubjectKind, SubjectRef, Tag, WorkflowRun,
};
use serde_json::Value;

/// One mapped event: a suffix that makes `<delivery>:<suffix>` unique within the
/// delivery, and the event.
pub type Mapped = (String, Event);

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("malformed {event} payload: {source}")]
    Malformed {
        event: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{event} payload is missing `{field}`")]
    Missing { event: String, field: &'static str },
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn opt_s(v: &Value) -> Option<String> {
    v.as_str().filter(|x| !x.is_empty()).map(str::to_string)
}

fn is_bot_user(u: &Value) -> bool {
    u["type"].as_str() == Some("Bot") || u["login"].as_str().is_some_and(|l| l.ends_with("[bot]"))
}

fn label_names(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| l["name"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Maps one webhook delivery (already signature-checked) to catalog events.
///
/// * `event_name`: `X-GitHub-Event`; `delivery`: `X-GitHub-Delivery` (the event `id`).
/// * `bot_handle`: the `@handle` a leading comment mention must name.
/// * `bot_login`: the App's login (`enzarb[bot]`); comments, reviews, issue and label
///   events it caused are dropped (loop protection), as are check runs/suites of its app.
///
/// Installation events, pings and unmapped actions yield nothing.
pub fn map(
    event_name: &str,
    delivery: &str,
    body: &[u8],
    bot_handle: &str,
    bot_login: &str,
) -> Result<Vec<Mapped>, MapError> {
    let p: Value = serde_json::from_slice(body).map_err(|source| MapError::Malformed {
        event: event_name.into(),
        source,
    })?;
    let missing = |field: &'static str| MapError::Missing {
        event: event_name.into(),
        field,
    };
    let Some(full) = p["repository"]["full_name"].as_str() else {
        return Ok(Vec::new());
    };
    let repo = full.to_lowercase();
    let action = p["action"].as_str().unwrap_or_default();
    let sender = &p["sender"];
    let from_bot = sender["login"]
        .as_str()
        .is_some_and(|l| l.eq_ignore_ascii_case(bot_login));
    let slug = bot_login.strip_suffix("[bot]").unwrap_or(bot_login);
    let from_our_app = |app: &Value| {
        app["slug"]
            .as_str()
            .is_some_and(|a| a.eq_ignore_ascii_case(slug))
    };
    let actor = sender["login"].as_str().map(|login| Actor {
        login: login.into(),
        can_write: false,
        bot: is_bot_user(sender),
    });
    let now = || Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let ev = |payload: Payload, subject: Option<SubjectRef>, at: &Value| Event {
        payload,
        id: Some(delivery.into()),
        repo: Some(repo.clone()),
        actor: actor.clone(),
        subject,
        at: Some(opt_s(at).unwrap_or_else(now)),
    };
    let issue_subject = |i: &Value| SubjectRef {
        kind: if i["pull_request"].is_object() {
            SubjectKind::PullRequest
        } else {
            SubjectKind::Issue
        },
        number: i["number"].as_u64().unwrap_or_default(),
    };
    let pr_subject = |n: u64| SubjectRef {
        kind: SubjectKind::PullRequest,
        number: n,
    };
    // One event per pull request a CI payload names, or one without a subject.
    let per_pr = |payload: Payload, prs: &Value, at: &Value| -> Vec<Event> {
        let nums: Vec<u64> = prs
            .as_array()
            .map(|a| a.iter().filter_map(|p| p["number"].as_u64()).collect())
            .unwrap_or_default();
        if nums.is_empty() {
            return vec![ev(payload, None, at)];
        }
        nums.into_iter()
            .map(|n| ev(payload.clone(), Some(pr_subject(n)), at))
            .collect()
    };

    let out: Vec<Event> = match (event_name, action) {
        ("issues", "opened" | "edited" | "closed" | "reopened") => {
            let i = &p["issue"];
            if !i.is_object() {
                return Err(missing("issue"));
            }
            // A sub-issue enzarb created for an epic is the one bot issue that passes.
            let epic_child = action == "opened"
                && i["body"]
                    .as_str()
                    .is_some_and(|b| parse_epic_marker(b).is_some());
            if from_bot && !epic_child {
                return Ok(Vec::new());
            }
            let payload = Payload::Issue(Issue {
                action: action.into(),
                number: i["number"].as_u64().ok_or(missing("issue.number"))?,
                title: s(&i["title"]),
                body: s(&i["body"]),
                labels: label_names(&i["labels"]),
            });
            vec![ev(payload, Some(issue_subject(i)), &i["updated_at"])]
        }
        ("issues" | "pull_request", "labeled" | "unlabeled") => {
            if from_bot {
                return Ok(Vec::new());
            }
            let obj = if event_name == "issues" {
                &p["issue"]
            } else {
                &p["pull_request"]
            };
            let subject = if event_name == "issues" {
                issue_subject(obj)
            } else {
                pr_subject(
                    obj["number"]
                        .as_u64()
                        .ok_or(missing("pull_request.number"))?,
                )
            };
            let payload = Payload::Label(Label {
                action: if action == "labeled" {
                    "added"
                } else {
                    "removed"
                }
                .into(),
                label: p["label"]["name"]
                    .as_str()
                    .ok_or(missing("label"))?
                    .to_ascii_lowercase(),
            });
            vec![ev(payload, Some(subject), &obj["updated_at"])]
        }
        ("issue_comment", "created") | ("pull_request_review_comment", "created") => {
            if from_bot {
                return Ok(Vec::new());
            }
            let c = &p["comment"];
            let subject = if event_name == "issue_comment" {
                issue_subject(&p["issue"])
            } else {
                pr_subject(
                    p["pull_request"]["number"]
                        .as_u64()
                        .ok_or(missing("pull_request"))?,
                )
            };
            let mut comment =
                Comment::parsed(c["body"].as_str().unwrap_or_default(), Some(bot_handle));
            comment.url = opt_s(&c["html_url"]);
            vec![ev(
                Payload::Comment(comment),
                Some(subject),
                &c["created_at"],
            )]
        }
        ("push", _) => {
            let Some(branch) = p["ref"]
                .as_str()
                .and_then(|r| r.strip_prefix("refs/heads/"))
            else {
                return Ok(Vec::new());
            };
            if p["deleted"].as_bool() == Some(true) {
                return Ok(Vec::new());
            }
            let payload = Payload::Push(Push {
                branch: branch.into(),
                before: s(&p["before"]),
                after: s(&p["after"]),
                commits: p["commits"].as_array().map_or(0, |a| a.len() as u32),
                forced: p["forced"].as_bool().unwrap_or(false),
            });
            vec![ev(payload, None, &p["head_commit"]["timestamp"])]
        }
        ("create" | "delete", _) if p["ref_type"].as_str() == Some("tag") => {
            let payload = Payload::Tag(Tag {
                action: if event_name == "create" {
                    "created"
                } else {
                    "deleted"
                }
                .into(),
                name: s(&p["ref"]),
                sha: String::new(),
            });
            vec![ev(payload, None, &Value::Null)]
        }
        (
            "pull_request",
            "opened" | "closed" | "synchronize" | "edited" | "reopened" | "ready_for_review",
        ) => {
            let pr = &p["pull_request"];
            let number = pr["number"]
                .as_u64()
                .ok_or(missing("pull_request.number"))?;
            let body = s(&pr["body"]);
            let payload = Payload::PullRequest(PullRequest {
                action: action.into(),
                number,
                title: s(&pr["title"]),
                references: closing_references(&body, &repo),
                body,
                head: s(&pr["head"]["ref"]),
                base: s(&pr["base"]["ref"]),
                head_sha: s(&pr["head"]["sha"]),
                merged: pr["merged"].as_bool().unwrap_or(false),
                draft: pr["draft"].as_bool().unwrap_or(false),
            });
            vec![ev(payload, Some(pr_subject(number)), &pr["updated_at"])]
        }
        ("check_run", _) => {
            let cr = &p["check_run"];
            if !cr.is_object() {
                return Err(missing("check_run"));
            }
            if from_our_app(&cr["app"]) {
                return Ok(Vec::new());
            }
            let payload = Payload::CheckRun(CheckRun {
                sha: s(&cr["head_sha"]),
                name: s(&cr["name"]),
                status: s(&cr["status"]),
                conclusion: opt_s(&cr["conclusion"]),
            });
            let at = if cr["completed_at"].is_string() {
                &cr["completed_at"]
            } else {
                &cr["started_at"]
            };
            per_pr(payload, &cr["pull_requests"], at)
        }
        ("check_suite", "completed") => {
            let cs = &p["check_suite"];
            if !cs.is_object() {
                return Err(missing("check_suite"));
            }
            if from_our_app(&cs["app"]) {
                return Ok(Vec::new());
            }
            let state = match cs["conclusion"].as_str() {
                Some("success" | "neutral" | "skipped") => "success",
                _ => "failure",
            };
            let payload = Payload::CiStatus(CiStatus {
                sha: s(&cs["head_sha"]),
                state: state.into(),
                context: opt_s(&cs["app"]["slug"]),
            });
            per_pr(payload, &cs["pull_requests"], &cs["updated_at"])
        }
        ("status", _) => {
            let payload = Payload::CiStatus(CiStatus {
                sha: p["sha"].as_str().ok_or(missing("sha"))?.into(),
                state: s(&p["state"]),
                context: opt_s(&p["context"]),
            });
            vec![ev(payload, None, &p["updated_at"])]
        }
        ("pull_request_review", "submitted") => {
            if from_bot {
                return Ok(Vec::new());
            }
            let r = &p["review"];
            let number = p["pull_request"]["number"]
                .as_u64()
                .ok_or(missing("pull_request"))?;
            let payload = Payload::Review(Review {
                number,
                state: s(&r["state"]).to_ascii_lowercase(),
                body: s(&r["body"]),
            });
            vec![ev(payload, Some(pr_subject(number)), &r["submitted_at"])]
        }
        ("release", _) => {
            let r = &p["release"];
            let payload = Payload::Release(Release {
                action: action.into(),
                tag: r["tag_name"]
                    .as_str()
                    .ok_or(missing("release.tag_name"))?
                    .into(),
                name: s(&r["name"]),
                prerelease: r["prerelease"].as_bool().unwrap_or(false),
            });
            let at = if r["published_at"].is_string() {
                &r["published_at"]
            } else {
                &r["created_at"]
            };
            vec![ev(payload, None, at)]
        }
        ("registry_package" | "package", "published") => {
            let pk = if event_name == "package" {
                &p["package"]
            } else {
                &p["registry_package"]
            };
            let pv = &pk["package_version"];
            let payload = Payload::Package(Package {
                action: action.into(),
                name: s(&pk["name"]),
                version: s(&pv["version"]),
                ecosystem: s(&pk["package_type"]).to_ascii_lowercase(),
            });
            vec![ev(payload, None, &pv["created_at"])]
        }
        ("deployment_status", _) => {
            let ds = &p["deployment_status"];
            let d = &p["deployment"];
            let environment = opt_s(&ds["environment"]).unwrap_or_else(|| s(&d["environment"]));
            let payload = Payload::Deployment(Deployment {
                environment,
                state: s(&ds["state"]),
                sha: s(&d["sha"]),
            });
            vec![ev(payload, None, &ds["updated_at"])]
        }
        ("workflow_run", _) => {
            let w = &p["workflow_run"];
            if !w.is_object() {
                return Err(missing("workflow_run"));
            }
            let payload = Payload::WorkflowRun(WorkflowRun {
                name: s(&w["name"]),
                status: s(&w["status"]),
                conclusion: opt_s(&w["conclusion"]),
                sha: s(&w["head_sha"]),
                url: opt_s(&w["html_url"]),
            });
            per_pr(payload, &w["pull_requests"], &w["updated_at"])
        }
        _ => Vec::new(),
    };
    Ok(out
        .into_iter()
        .enumerate()
        .map(|(i, e)| (i.to_string(), e))
        .collect())
}

const CLOSING_KEYWORDS: &[&str] = &[
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// Issue numbers of `repo` (`owner/repo`, lowercase) that closing keywords in `body`
/// reference: `Fixes #1`, `closes: owner/repo#2`, `Resolves https://github.com/owner/repo/issues/3`.
/// Sorted, without duplicates.
pub fn closing_references(body: &str, repo: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let words: Vec<&str> = body.split_whitespace().collect();
    for pair in words.windows(2) {
        let kw = pair[0]
            .trim_end_matches(':')
            .trim_start_matches(|c: char| !c.is_ascii_alphabetic())
            .to_ascii_lowercase();
        if !CLOSING_KEYWORDS.contains(&kw.as_str()) {
            continue;
        }
        let target = pair[1].trim_end_matches(['.', ',', ';', ')', ':']);
        if let Some(n) = issue_ref(target, repo) {
            out.push(n);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn issue_ref(target: &str, repo: &str) -> Option<u64> {
    let (prefix, num) = if let Some(rest) = target
        .strip_prefix("https://github.com/")
        .or_else(|| target.strip_prefix("http://github.com/"))
    {
        let (r, n) = rest.rsplit_once("/issues/")?;
        (r, n)
    } else {
        target.rsplit_once('#')?
    };
    if !prefix.is_empty() && !prefix.eq_ignore_ascii_case(repo) {
        return None;
    }
    num.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::closing_references;

    #[test]
    fn closing_keywords() {
        let r = "o/r";
        assert_eq!(closing_references("Fixes #12", r), vec![12]);
        assert_eq!(
            closing_references(
                "closes: o/r#3, resolves https://github.com/O/R/issues/4.",
                r
            ),
            vec![3, 4]
        );
        assert_eq!(
            closing_references("refs #5 and fixes x/y#6", r),
            Vec::<u64>::new()
        );
        assert_eq!(closing_references("- Closed #7\n- fix #7", r), vec![7]);
    }
}
