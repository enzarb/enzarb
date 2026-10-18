#![cfg(feature = "fake")]

use enzarb_connector::fake::{Call, FakeConnector};
use enzarb_connector::*;

fn item() -> WorkItemKey {
    "github:o/r#1".parse().unwrap()
}

#[tokio::test]
async fn update_issue_replaces_the_body_and_replays() {
    let f = FakeConnector::new();
    let key = f
        .create_issue(&item().repo, "T", "old", &[], "k")
        .await
        .unwrap();
    f.update_issue(&key, "new").await.unwrap();
    f.update_issue(&key, "new").await.unwrap();
    assert_eq!(f.created_issues()[0].2, "new");
    let calls = f.calls();
    assert!(!calls[1].replayed);
    assert!(calls[2].replayed);
    let missing = f.update_issue(&item(), "x").await.unwrap_err();
    assert!(matches!(missing, Error::NotFound(_)), "{missing:?}");
}

#[tokio::test]
async fn post_comment_is_idempotent_and_recorded() {
    let f = FakeConnector::new();
    let a = f.post_comment(&item(), "hi", "k1").await.unwrap();
    let b = f.post_comment(&item(), "hi", "k1").await.unwrap();
    let c = f.post_comment(&item(), "hi", "k2").await.unwrap();
    assert_eq!(a, b);
    assert_ne!(a.id, c.id);
    assert_eq!(f.calls().len(), 3);
    assert_eq!(f.applied_calls().len(), 2);
    assert!(f.calls()[1].replayed);
    assert_eq!(f.comments(&item()).len(), 2);
}

#[tokio::test]
async fn progress_status_and_change_request() {
    let f = FakeConnector::new();
    let t = ProgressTarget {
        item: item(),
        comment_id: Some(5),
        comment_kind: Default::default(),
    };
    f.set_progress(&t, Progress::Received, "p1").await.unwrap();
    f.set_progress(&t, Progress::Working, "p2").await.unwrap();
    f.set_progress(&t, Progress::Done, "p2").await.unwrap(); // replay: ignored
    assert_eq!(f.progress(&t), Some(Progress::Working));

    let commit = CommitRef {
        repo: item().repo,
        sha: "abc".into(),
    };
    let st = Status {
        state: StatusState::Success,
        context: "enzarb".into(),
        description: String::new(),
        target_url: None,
    };
    f.set_status(&commit, st, "s1").await.unwrap();
    assert_eq!(f.statuses().len(), 1);

    let spec = ChangeRequestSpec {
        repo: item().repo,
        title: "t".into(),
        body: "b".into(),
        head_ref: "enzarb/issue-1".into(),
        base_ref: "main".into(),
        draft: false,
    };
    let a = f.open_change_request(spec.clone(), "cr").await.unwrap();
    let b = f.open_change_request(spec, "cr").await.unwrap();
    assert_eq!(a, b);
    assert_eq!(f.change_requests().len(), 1);
    assert_eq!(a.kind, ItemKind::ChangeRequest);
}

#[tokio::test]
async fn permissions_credentials_threads_and_failures() {
    let f = FakeConnector::new();
    let repo = item().repo;
    let ruckc = Actor {
        login: "ruckc".into(),
        id: 1,
        is_bot: false,
    };
    let other = Actor {
        login: "other".into(),
        id: 2,
        is_bot: false,
    };
    f.set_permission(repo.clone(), "ruckc", Permission::Admin);
    assert_eq!(
        f.check_permission(&repo, &ruckc).await.unwrap(),
        Permission::Admin
    );
    assert_eq!(
        f.check_permission(&repo, &other).await.unwrap(),
        Permission::None
    );
    f.set_default_permission(Permission::Read);
    assert_eq!(
        f.check_permission(&repo, &other).await.unwrap(),
        Permission::Read
    );

    let c1 = f
        .mint_clone_credential(&repo, CredScope::ReadWrite, "m")
        .await
        .unwrap();
    let c2 = f
        .mint_clone_credential(&repo, CredScope::ReadWrite, "m")
        .await
        .unwrap();
    assert_eq!(c1, c2);
    assert_eq!(c1.repo, repo);

    assert!(matches!(
        f.fetch_thread(&item()).await,
        Err(Error::NotFound(_))
    ));
    let thread = Thread {
        item: ItemRef {
            kind: ItemKind::Issue,
            number: 1,
            url: "u".into(),
            title: "t".into(),
            body: "b".into(),
            author: "a".into(),
            head_ref: None,
            base_ref: None,
            head_sha: None,
        },
        comments: vec![],
    };
    f.add_thread(item(), thread);
    f.post_comment(&item(), "reply", "k").await.unwrap();
    assert_eq!(f.fetch_thread(&item()).await.unwrap().comments.len(), 1);

    f.fail_next(Error::Transient("boom".into()));
    let e = f.post_comment(&item(), "x", "k9").await.unwrap_err();
    assert!(e.is_retryable());
    f.post_comment(&item(), "x", "k9").await.unwrap(); // failure is one-shot
    assert!(matches!(
        f.calls().last().unwrap().call,
        Call::PostComment { .. }
    ));
}

#[tokio::test]
async fn usable_as_trait_object() {
    let f: Box<dyn Connector> = Box::new(FakeConnector::new());
    f.post_comment(&item(), "hi", "k").await.unwrap();
}

#[tokio::test]
async fn merge_is_scripted_idempotent_and_recorded() {
    let f = FakeConnector::new();
    f.queue_merge_outcome(MergeOutcome::HeadMoved);
    let a = f
        .merge_change_request(&item(), 1, "abc", "feat: x", Some("body"), "m1")
        .await
        .unwrap();
    assert_eq!(a, MergeOutcome::HeadMoved);
    // Same key replays the first outcome, without consuming the queue.
    f.queue_merge_outcome(MergeOutcome::NotMergeable {
        reason: "conflicts".into(),
    });
    let b = f
        .merge_change_request(&item(), 1, "abc", "feat: x", Some("body"), "m1")
        .await
        .unwrap();
    assert_eq!(a, b);
    let c = f
        .merge_change_request(&item(), 1, "abc", "feat: x", None, "m2")
        .await
        .unwrap();
    assert!(matches!(c, MergeOutcome::NotMergeable { .. }));
    let d = f
        .merge_change_request(&item(), 1, "abc", "feat: x", None, "m3")
        .await
        .unwrap();
    assert!(matches!(d, MergeOutcome::Merged { .. }));
    assert_eq!(f.calls().len(), 4);
    assert!(f.calls()[1].replayed);
    assert!(matches!(
        &f.calls()[0].call,
        Call::MergeChangeRequest { expected_head_sha, commit_body: Some(_), .. }
            if expected_head_sha == "abc"
    ));
}

#[tokio::test]
async fn readiness_is_seeded() {
    let f = FakeConnector::new();
    assert!(matches!(
        f.get_merge_readiness(&item(), 1).await,
        Err(Error::NotFound(_))
    ));
    let r = MergeReadiness {
        head_sha: "abc".into(),
        base_sha: Some("def".into()),
        mergeable: None,
        mergeable_state: "unknown".into(),
        ci: CiState::Pending,
    };
    f.set_merge_readiness(item(), r.clone());
    assert_eq!(f.get_merge_readiness(&item(), 1).await.unwrap(), r);
    assert_eq!(f.calls().len(), 2);
}

#[test]
fn readiness_without_base_sha_deserializes_to_none() {
    let r: MergeReadiness = serde_json::from_value(serde_json::json!({
        "head_sha": "abc",
        "mergeable": true,
        "mergeable_state": "clean",
        "ci": "success",
    }))
    .unwrap();
    assert_eq!(r.base_sha, None);
}

#[tokio::test]
async fn branch_head_is_seeded_and_recorded() {
    let f = FakeConnector::new();
    let repo = RepoKey::new("github", "enzarb", "enzarb-proto");
    assert_eq!(f.branch_head(&repo, "main").await.unwrap(), None);
    f.set_branch_head(repo.clone(), "main", "abc123");
    assert_eq!(
        f.branch_head(&repo, "main").await.unwrap().as_deref(),
        Some("abc123")
    );
    assert_eq!(f.branch_head(&repo, "other").await.unwrap(), None);
    let calls = f.calls();
    assert_eq!(calls.len(), 3);
    assert!(matches!(
        &calls[1].call,
        Call::BranchHead { branch, .. } if branch == "main"
    ));
}

#[tokio::test]
async fn get_file_is_seeded_and_recorded() {
    let f = FakeConnector::new();
    let repo = RepoKey::new("github", "enzarb", "enzarb-proto");
    let path = ".enzarb/workflows/issue.yaml";
    assert_eq!(f.get_file(&repo, "abc", path).await.unwrap(), None);
    f.set_file(repo.clone(), "abc", path, "version: 1\n");
    assert_eq!(
        f.get_file(&repo, "abc", path).await.unwrap().as_deref(),
        Some("version: 1\n")
    );
    assert_eq!(f.get_file(&repo, "other", path).await.unwrap(), None);
    assert!(matches!(
        &f.calls()[1].call,
        Call::GetFile { git_ref, .. } if git_ref == "abc"
    ));
}

#[tokio::test]
async fn read_credential_records_call_and_validates() {
    let f = FakeConnector::new();
    let repos = vec![RepoKey::new("github", "enzarb", "enzarb-proto")];
    let cred = f
        .mint_read_credential(&repos, "deps")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cred.repos, repos);
    assert!(!format!("{cred:?}").contains(&cred.token));
    assert!(matches!(
        f.mint_read_credential(&[], "deps").await,
        Err(Error::Invalid(_))
    ));
    let calls = f.calls();
    assert_eq!(
        calls[0].call,
        Call::MintReadCredential {
            repos,
            purpose: "deps".into()
        }
    );
}

#[tokio::test]
async fn set_base_retargets_seeded_change_request_idempotently() {
    let f = FakeConnector::new();
    let key: WorkItemKey = "github:o/r#5".parse().unwrap();
    f.add_thread(
        key.clone(),
        Thread {
            item: ItemRef {
                kind: ItemKind::ChangeRequest,
                number: 5,
                url: "u".into(),
                title: "t".into(),
                body: String::new(),
                author: "a".into(),
                head_ref: Some("feat".into()),
                base_ref: Some("parent".into()),
                head_sha: Some("abc".into()),
            },
            comments: vec![],
        },
    );
    f.set_base(&key, 5, "main", "k").await.unwrap();
    f.set_base(&key, 5, "main", "k").await.unwrap();
    let t = f.fetch_thread(&key).await.unwrap();
    assert_eq!(t.item.base_ref.as_deref(), Some("main"));
    assert_eq!(f.set_base_calls().len(), 2);
    assert_eq!(f.calls().iter().filter(|c| c.replayed).count(), 1);
    let missing: WorkItemKey = "github:o/r#9".parse().unwrap();
    let err = f.set_base(&missing, 9, "main", "k2").await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn delete_branch_is_idempotent_and_forgets_the_head() {
    let f = FakeConnector::new();
    let repo = item().repo;
    f.set_branch_head(repo.clone(), "enzarb/x", "abc");
    f.delete_branch(&repo, "enzarb/x", "k").await.unwrap();
    f.delete_branch(&repo, "enzarb/x", "k").await.unwrap();
    // a branch that is already gone is success too
    f.delete_branch(&repo, "enzarb/never", "k2").await.unwrap();
    assert_eq!(f.branch_head(&repo, "enzarb/x").await.unwrap(), None);
    assert_eq!(f.delete_branch_calls().len(), 3);
    assert!(f.calls()[1].replayed);
    assert!(!f.calls()[2].replayed);
}

#[tokio::test]
async fn delete_branch_failure_is_injected() {
    let f = FakeConnector::new();
    f.fail_next(Error::PermissionDenied("no".into()));
    let err = f.delete_branch(&item().repo, "b", "k").await.unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)), "{err:?}");
}

#[tokio::test]
async fn close_issue_records_the_reason_and_replays() {
    let f = FakeConnector::new();
    assert_eq!(f.closed_issue(&item()), None);
    f.close_issue(&item(), CloseReason::Completed)
        .await
        .unwrap();
    f.close_issue(&item(), CloseReason::Completed)
        .await
        .unwrap();
    assert_eq!(f.closed_issue(&item()), Some(CloseReason::Completed));
    let calls = f.calls();
    assert_eq!(
        calls[0].call,
        Call::CloseIssue {
            item: item(),
            reason: CloseReason::Completed
        }
    );
    assert!(!calls[0].replayed);
    assert!(calls[1].replayed);
}

#[tokio::test]
async fn upsert_issue_dedupes_by_key_and_updates_changed_text() {
    let f = FakeConnector::new();
    let repo = item().repo;
    let a = f.upsert_issue(&repo, "T", "B", &[], "k1").await.unwrap();
    assert_eq!(a.outcome, UpsertOutcome::Created);
    let b = f.upsert_issue(&repo, "T", "B", &[], "k1").await.unwrap();
    assert_eq!(
        (b.item.clone(), b.outcome),
        (a.item.clone(), UpsertOutcome::Unchanged)
    );
    assert_eq!(f.created_issues().len(), 1);
    let c = f.upsert_issue(&repo, "T2", "B", &[], "k1").await.unwrap();
    assert_eq!(
        (c.item.clone(), c.outcome),
        (a.item.clone(), UpsertOutcome::Updated)
    );
    let d = f.upsert_issue(&repo, "T2", "B2", &[], "k1").await.unwrap();
    assert_eq!(d.outcome, UpsertOutcome::Updated);
    let issues = f.created_issues();
    assert_eq!(issues.len(), 1);
    assert_eq!((issues[0].1.as_str(), issues[0].2.as_str()), ("T2", "B2"));
    let e = f.upsert_issue(&repo, "T", "B", &[], "k2").await.unwrap();
    assert_eq!(e.outcome, UpsertOutcome::Created);
    assert_ne!(e.item, a.item);
    assert_eq!(f.created_issues().len(), 2);
    let replayed: Vec<bool> = f.calls().iter().map(|c| c.replayed).collect();
    assert_eq!(replayed, [false, true, false, false, false]);
}
