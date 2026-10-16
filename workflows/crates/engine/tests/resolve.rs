//! Definition resolution against a cold cache: a new library sha is listed once and its files
//! fetched concurrently, so a workflow with many files resolves well within the budget even when
//! every file read is slow.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use engine::{Library, ProjectInfo};
use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{
    Actor, ChangeRequestSpec, CommentRef, CommitRef, Connector, CredScope, ItemRef, MergeOutcome,
    MergeReadiness, Permission, Progress, ProgressTarget, RepoKey, Result, ShortLivedCred, Status,
    Thread, WorkItemKey,
};
use enzarb_flow::resolve::Entry;

const LIB: &str = "enzarb/workflows";
const SHA: &str = "1111111111111111111111111111111111111111";
const PHASES: usize = 60;
const DELAY: Duration = Duration::from_millis(100);

/// The fake connector with a slow `get_file` (only what [`Library`] calls is implemented).
struct Slow {
    inner: FakeConnector,
    reads: AtomicUsize,
}

#[async_trait]
impl Connector for Slow {
    async fn fetch_thread(&self, _: &WorkItemKey) -> Result<Thread> {
        unimplemented!()
    }
    async fn post_comment(&self, _: &WorkItemKey, _: &str, _: &str) -> Result<CommentRef> {
        unimplemented!()
    }
    async fn open_change_request(&self, _: ChangeRequestSpec, _: &str) -> Result<ItemRef> {
        unimplemented!()
    }
    async fn set_status(&self, _: &CommitRef, _: Status, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn set_progress(&self, _: &ProgressTarget, _: Progress, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn merge_change_request(
        &self,
        _: &WorkItemKey,
        _: u64,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: &str,
    ) -> Result<MergeOutcome> {
        unimplemented!()
    }
    async fn get_merge_readiness(&self, _: &WorkItemKey, _: u64) -> Result<MergeReadiness> {
        unimplemented!()
    }
    async fn check_permission(&self, _: &RepoKey, _: &Actor) -> Result<Permission> {
        unimplemented!()
    }
    async fn mint_clone_credential(
        &self,
        _: &RepoKey,
        _: CredScope,
        _: &str,
    ) -> Result<ShortLivedCred> {
        unimplemented!()
    }
    async fn resolve_ref(&self, repo: &RepoKey, git_ref: &str) -> Result<Option<String>> {
        self.inner.resolve_ref(repo, git_ref).await
    }
    async fn list_files(
        &self,
        repo: &RepoKey,
        git_ref: &str,
    ) -> Result<Option<Vec<(String, u64)>>> {
        self.inner.list_files(repo, git_ref).await
    }
    async fn get_file(&self, repo: &RepoKey, git_ref: &str, path: &str) -> Result<Option<String>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(DELAY).await;
        self.inner.get_file(repo, git_ref, path).await
    }
}

fn library() -> FakeConnector {
    let gh = FakeConnector::new();
    let rk = RepoKey::new("github", "enzarb", "workflows");
    let mut phases = String::new();
    for i in 0..PHASES {
        phases.push_str(&format!("  - {{id: p{i}, use: phases/p{i}}}\n"));
        gh.set_file(
            rk.clone(),
            SHA,
            &format!("phases/p{i}.yaml"),
            "version: 1\nstart: done\nnodes:\n  done: {end: {status: done}}\n",
        );
    }
    gh.set_file(
        rk.clone(),
        SHA,
        "workflows/many.yaml",
        &format!("version: 1\nphases:\n{phases}"),
    );
    gh.set_ref(rk, "v1", SHA);
    gh
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_library_with_many_files_resolves_within_budget() {
    let slow = Arc::new(Slow {
        inner: library(),
        reads: AtomicUsize::new(0),
    });
    // One by one this needs PHASES * DELAY = 6 s; the budget is half of that.
    let lib = Library::new(slow.clone(), &format!("{LIB}@v1"), vec![])
        .with_budget(Duration::from_secs(3));
    let p = ProjectInfo {
        name: "p".into(),
        ..Default::default()
    };
    let entry = || Entry {
        repo: LIB.into(),
        rref: "v1".into(),
        path: "workflows/many".into(),
    };
    let t = Instant::now();
    let r = lib.resolve(&p, entry(), None).await.expect("resolves");
    assert_eq!(r.phases.len(), PHASES);
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(slow.inner.generic_calls("list_files").len(), 1);
    assert_eq!(
        slow.reads.load(Ordering::SeqCst),
        PHASES + 1,
        "each file read once"
    );

    // Warm: no reads at all, and absent files are known absent without a request.
    let r2 = lib.resolve(&p, entry(), None).await.expect("resolves");
    assert_eq!(r2, r);
    let (_, none) = lib.read(LIB, "v1", "nope.yaml").await.unwrap();
    assert!(none.is_none());
    assert_eq!(slow.reads.load(Ordering::SeqCst), PHASES + 1);
    assert_eq!(slow.inner.generic_calls("list_files").len(), 1);
}
