//! Run preparation: what the sandbox's clone says about the change, before the worktree exists.
//!
//! Before an agent run starts, the executor asks the sandbox (`prepare_request`) to fetch the
//! run's refs and report the merge base of `origin/<base_ref>` and the branch, the files changed
//! since, and the components `scripts/monorepo/affected.sh` names for them. The engine has no
//! checkout, so the git work happens in the sandbox; this module only orchestrates it.
//!
//! The step is best-effort and never fails a job: a sandbox without the `prepare` feature (an
//! older image) is not asked, and an error, a timeout or an empty reply leaves the
//! [`PromptContext`] as it was, so the prompts fall back to their `(none)` wording.

use std::time::Duration;

use async_trait::async_trait;
use enzarb_proto::session::Session;
use enzarb_proto::{PrepareRequest, Prepared, Secret, feature};

use crate::machine::{AgentJobInput, PromptContext};

/// How long the engine waits for the sandbox's `prepared` (its own cap is shorter).
pub(crate) const PREPARE_TIMEOUT: Duration = Duration::from_secs(60);

/// The source of a run's preparation. [`Session`] in production; tests inject a fake.
#[async_trait]
pub(crate) trait GitPrep: Send + Sync {
    /// The sandbox's answer, or `None` when it cannot or did not give one in time.
    async fn prepare(&self, req: PrepareRequest) -> Option<Prepared>;
}

#[async_trait]
impl GitPrep for Session {
    async fn prepare(&self, req: PrepareRequest) -> Option<Prepared> {
        if !self.has_feature(feature::PREPARE) {
            tracing::debug!(run_id = %req.run_id, "sandbox has no prepare feature; skipping");
            return None;
        }
        let run_id = req.run_id.clone();
        match tokio::time::timeout(PREPARE_TIMEOUT, Session::prepare(self, req)).await {
            Ok(Ok(p)) => Some(p),
            Ok(Err(e)) => {
                tracing::warn!(%run_id, error = %e, "run preparation failed; continuing without it");
                None
            }
            Err(_) => {
                tracing::warn!(%run_id, "run preparation timed out; continuing without it");
                None
            }
        }
    }
}

/// Ask `git` to prepare the run and put the answer in `input.context`. Never fails.
pub(crate) async fn prepare_context(
    git: &dyn GitPrep,
    run_id: &str,
    repo_url: &str,
    input: &mut AgentJobInput,
    clone_token: &str,
) {
    let req = PrepareRequest {
        run_id: run_id.to_string(),
        repo_url: repo_url.to_string(),
        base_ref: input.base_ref.clone(),
        branch: input.branch.clone(),
        clone_token: Secret::new(clone_token),
    };
    let reply = git.prepare(req).await;
    apply(&mut input.context, reply);
}

/// Fill the prepared fields of `ctx`. `None`, or a field the sandbox left empty, changes nothing.
pub(crate) fn apply(ctx: &mut PromptContext, prepared: Option<Prepared>) {
    let Some(p) = prepared else { return };
    if let Some(mb) = p.merge_base_sha.filter(|s| !s.is_empty()) {
        ctx.merge_base_sha = mb;
    }
    if !p.changed_files.is_empty() {
        let mut list = p.changed_files.join("\n");
        let more = p.changed_files_total.saturating_sub(p.changed_files.len());
        if more > 0 {
            list.push_str(&format!("\n... and {more} more"));
        }
        ctx.changed_files = list;
    }
    if !p.affected.is_empty() {
        ctx.affected = p.affected.join("\n");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use enzarb_connector::RepoKey;
    use enzarb_proto::session::{Event, ServerOptions, memory::session_pair_with};
    use enzarb_proto::{ClientKind, Ctrl};

    use super::*;
    use crate::machine::{JobKind, render_prompt};

    struct Fake {
        reply: Option<Prepared>,
        seen: Mutex<Vec<PrepareRequest>>,
    }

    impl Fake {
        fn new(reply: Option<Prepared>) -> Self {
            Self {
                reply,
                seen: Mutex::default(),
            }
        }
    }

    #[async_trait]
    impl GitPrep for Fake {
        async fn prepare(&self, req: PrepareRequest) -> Option<Prepared> {
            self.seen.lock().unwrap().push(req);
            self.reply.clone()
        }
    }

    fn input() -> AgentJobInput {
        AgentJobInput {
            kind: JobKind::AgentReview,
            repo: "github:o/r".parse::<RepoKey>().unwrap(),
            base_ref: "main".into(),
            branch: "enzarb/issue-1".into(),
            push: true,
            timeout_secs: 600,
            context: PromptContext {
                base_ref: "main".into(),
                ..Default::default()
            },
            prompt: String::new(),
            recovery_used: false,
            graph: None,
            prompt_files: Default::default(),
        }
    }

    fn reply() -> Prepared {
        Prepared {
            run_id: "job-1".into(),
            merge_base_sha: Some("abc123".into()),
            changed_files: vec!["ui/a.ts".into(), "docs/b.md".into()],
            changed_files_total: 2,
            affected: vec!["proto".into(), "ui".into()],
        }
    }

    #[tokio::test]
    async fn a_reply_fills_the_context_and_the_prompt_renders_it() {
        let git = Fake::new(Some(reply()));
        let mut input = input();
        prepare_context(&git, "job-1", "https://github.com/o/r", &mut input, "ghs_x").await;
        let ctx = &input.context;
        assert_eq!(ctx.merge_base_sha, "abc123");
        assert_eq!(ctx.changed_files, "ui/a.ts\ndocs/b.md");
        assert_eq!(ctx.affected, "proto\nui");
        let out = render_prompt(JobKind::AgentReview, ctx);
        assert!(out.contains("git diff abc123...HEAD"));
        assert!(out.contains("ui/a.ts\ndocs/b.md"));
        assert!(out.contains("`proto\nui`"));
        let seen = git.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].base_ref.as_str(), seen[0].branch.as_str()),
            ("main", "enzarb/issue-1")
        );
        assert_eq!(seen[0].clone_token.expose(), "ghs_x");
    }

    #[tokio::test]
    async fn no_reply_leaves_the_context_as_it_was() {
        let before = input();
        let mut after = input();
        prepare_context(&Fake::new(None), "job-1", "u", &mut after, "t").await;
        assert_eq!(after.context, before.context);
    }

    #[test]
    fn an_empty_reply_changes_nothing() {
        let mut ctx = PromptContext::default();
        apply(
            &mut ctx,
            Some(Prepared {
                run_id: "job-1".into(),
                ..Default::default()
            }),
        );
        assert_eq!(ctx, PromptContext::default());
    }

    #[test]
    fn a_cut_short_list_says_how_many_more() {
        let mut ctx = PromptContext::default();
        apply(
            &mut ctx,
            Some(Prepared {
                run_id: "job-1".into(),
                changed_files: vec!["a".into(), "b".into()],
                changed_files_total: 7,
                ..Default::default()
            }),
        );
        assert_eq!(ctx.changed_files, "a\nb\n... and 5 more");
    }

    #[test]
    fn the_request_never_shows_the_clone_token_in_debug_output() {
        let req = PrepareRequest {
            run_id: "job-1".into(),
            repo_url: "u".into(),
            base_ref: "main".into(),
            branch: "b".into(),
            clone_token: Secret::new("ghs_super_secret"),
        };
        let shown = format!("{req:?} {:?}", Ctrl::PrepareRequest(req.clone()));
        assert!(!shown.contains("ghs_super_secret"), "{shown}");
    }

    fn server(features: &[&str]) -> ServerOptions {
        ServerOptions {
            features: features.iter().map(|f| f.to_string()).collect(),
            ..Default::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_sandbox_without_the_feature_is_never_asked() {
        let ((client, _cev), (_server, mut sev)) =
            session_pair_with(ClientKind::Executor, server(&[])).await;
        let mut input = input();
        let before = input.context.clone();
        prepare_context(&client, "job-1", "u", &mut input, "t").await;
        assert_eq!(input.context, before);
        let nothing = tokio::time::timeout(Duration::from_millis(100), sev.next()).await;
        assert!(nothing.is_err(), "the old sandbox got a prepare_request");
    }

    #[tokio::test(start_paused = true)]
    async fn a_sandbox_with_the_feature_answers() {
        let ((client, _cev), (srv, mut sev)) =
            session_pair_with(ClientKind::Executor, server(&[feature::PREPARE])).await;
        let sent = reply();
        tokio::spawn(async move {
            while let Some(ev) = sev.next().await {
                if let Event::Ctrl(Ctrl::PrepareRequest(req)) = ev {
                    let mut p = sent.clone();
                    p.run_id = req.run_id;
                    srv.send_ctrl(&Ctrl::Prepared(p)).unwrap();
                }
            }
        });
        let mut input = input();
        prepare_context(&client, "job-1", "u", &mut input, "t").await;
        assert_eq!(input.context.merge_base_sha, "abc123");
    }

    #[tokio::test(start_paused = true)]
    async fn an_error_reply_falls_back_to_the_old_context() {
        let ((client, _cev), (srv, mut sev)) =
            session_pair_with(ClientKind::Executor, server(&[feature::PREPARE])).await;
        tokio::spawn(async move {
            while let Some(ev) = sev.next().await {
                if let Event::Ctrl(Ctrl::PrepareRequest(_)) = ev {
                    srv.send_error("internal", "boom").unwrap();
                }
            }
        });
        let mut input = input();
        let before = input.context.clone();
        prepare_context(&client, "job-1", "u", &mut input, "t").await;
        assert_eq!(input.context, before);
    }

    #[tokio::test(start_paused = true)]
    async fn a_sandbox_that_never_answers_times_out_to_the_old_context() {
        let ((client, _cev), (_srv, mut sev)) =
            session_pair_with(ClientKind::Executor, server(&[feature::PREPARE])).await;
        tokio::spawn(async move { while sev.next().await.is_some() {} });
        let mut input = input();
        let before = input.context.clone();
        prepare_context(&client, "job-1", "u", &mut input, "t").await;
        assert_eq!(input.context, before);
    }
}
