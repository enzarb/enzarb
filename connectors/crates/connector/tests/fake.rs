#![cfg(feature = "fake")]

use enzarb_connector::fake::{Call, FakeConnector};
use enzarb_connector::*;

fn item() -> WorkItemKey {
    "github:o/r#1".parse().unwrap()
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
        mergeable: None,
        mergeable_state: "unknown".into(),
        ci: CiState::Pending,
    };
    f.set_merge_readiness(item(), r.clone());
    assert_eq!(f.get_merge_readiness(&item(), 1).await.unwrap(), r);
    assert_eq!(f.calls().len(), 2);
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
