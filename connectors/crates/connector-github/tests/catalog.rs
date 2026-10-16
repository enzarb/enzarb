//! Golden tests: fixture webhook JSON -> expected catalog events
//! (`tests/golden/<fixture>.json`). Regenerate with `UPDATE_GOLDEN=1 cargo test`.

use enzarb_connector_github::catalog::map;
use serde_json::{Value, json};

const BOT_HANDLE: &str = "enzarb";
const BOT_LOGIN: &str = "enzarb[bot]";

const CASES: &[(&str, &str)] = &[
    ("issues", "issues_opened"),
    ("issues", "issues_edited"),
    ("issues", "issues_closed"),
    ("issues", "issues_opened_by_bot"),
    ("issues", "issues_opened_epic_child"),
    ("issues", "issues_labeled_other"),
    ("issues", "issues_labeled_by_bot"),
    ("issue_comment", "issue_comment_created"),
    ("issue_comment", "issue_comment_mention_approve_on_pr"),
    ("issue_comment", "issue_comment_by_bot"),
    ("issue_comment", "issue_comment_edited"),
    (
        "pull_request_review_comment",
        "pull_request_review_comment_created",
    ),
    ("push", "push"),
    ("push", "push_tag"),
    ("create", "create_tag"),
    ("create", "create_branch"),
    ("delete", "delete_tag"),
    ("pull_request", "pull_request_opened"),
    ("pull_request", "pull_request_opened_by_bot"),
    ("pull_request", "pull_request_closed_merged"),
    ("pull_request", "pull_request_synchronize"),
    ("pull_request", "pull_request_reopened"),
    ("pull_request", "pull_request_unlabeled"),
    ("check_run", "check_run_completed"),
    ("check_run", "check_run_by_bot_app"),
    ("check_suite", "check_suite_completed_failure"),
    ("check_suite", "check_suite_completed_no_pr"),
    ("status", "status_success"),
    (
        "pull_request_review",
        "pull_request_review_submitted_changes",
    ),
    ("release", "release_published"),
    ("registry_package", "registry_package_published"),
    ("package", "package_published"),
    ("deployment_status", "deployment_status"),
    ("workflow_run", "workflow_run_completed"),
    ("issue_comment", "issue_comment_approve"),
    ("issue_comment", "issue_comment_command"),
    ("issue_comment", "issue_comment_mention"),
    ("issue_comment", "issue_comment_quoted_approve"),
    ("issues", "issues_labeled_approved"),
    ("issues", "issues_labeled_epic"),
    ("issues", "issues_opened_plain"),
    ("pull_request", "pull_request_closed_unmerged"),
    ("pull_request", "pull_request_labeled_approved"),
    ("pull_request_review", "pull_request_review_submitted"),
    ("installation", "installation_created"),
    ("installation", "installation_deleted"),
    (
        "installation_repositories",
        "installation_repositories_added",
    ),
    (
        "installation_repositories",
        "installation_repositories_removed",
    ),
    ("ping", "ping"),
];

fn dir() -> String {
    format!("{}/tests", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn golden() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures = Vec::new();
    for (event, name) in CASES {
        let body = std::fs::read(format!("{}/fixtures/{name}.json", dir())).unwrap();
        let text = String::from_utf8_lossy(&body).to_string();
        let mapped = map(event, "d-1", &body, BOT_HANDLE, BOT_LOGIN).unwrap();
        let got: Vec<Value> = mapped
            .into_iter()
            .map(|(suffix, e)| {
                let mut v = serde_json::to_value(&e).unwrap();
                // A time the payload does not carry is the mapping time.
                if let Some(at) = v["at"].as_str()
                    && !text.contains(at)
                {
                    v["at"] = json!("<now>");
                }
                json!({"suffix": suffix, "type": e.type_name(), "event": v})
            })
            .collect();
        let got = Value::Array(got);
        let path = format!("{}/golden/{name}.json", dir());
        if update {
            std::fs::create_dir_all(format!("{}/golden", dir())).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
            continue;
        }
        let want: Value = serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path}")),
        )
        .unwrap();
        if got != want {
            failures.push(format!(
                "{name}:\n got {}\nwant {}",
                serde_json::to_string_pretty(&got).unwrap(),
                serde_json::to_string_pretty(&want).unwrap()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn malformed_payload_is_an_error() {
    assert!(map("issues", "d", b"not json", BOT_HANDLE, BOT_LOGIN).is_err());
    assert!(
        map(
            "issues",
            "d",
            br#"{"action":"opened","repository":{"full_name":"a/b"}}"#,
            BOT_HANDLE,
            BOT_LOGIN
        )
        .is_err()
    );
}
