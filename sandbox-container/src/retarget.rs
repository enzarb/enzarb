//! Retarget run: rebase enzarb's own child branch after its parent was
//! squash-merged (stacked change requests, issue #107).
//!
//! `git rebase --onto origin/<onto> <old_base> <head_ref>` rewrites history, so
//! the push needs a lease. It is the ONLY force-push in enzarb, which is why it
//! lives alone in this module (a source scan in the tests keeps the force flags
//! out of every other file) and why it is fenced in on every side:
//!
//! * [`validate`] runs before any git call and rejects everything that is not
//!   a plain enzarb-created branch (prefix [`BRANCH_PREFIX`], not the default
//!   branch `onto`, not in `protected`, no option-like or odd ref syntax) and
//!   non-sha `old_base` / `expected_sha`;
//! * the remote head must equal `expected_sha` (also enforced by the lease),
//!   and `old_base` must be an ancestor of it;
//! * every non-merge commit in `old_base..head` must be authored by the
//!   sandbox's own configured git identity (name and email): a human's commits
//!   on the branch are never rewritten;
//! * the rebase happens in a throwaway detached worktree; a conflict aborts
//!   the rebase and nothing is pushed;
//! * the push goes through the hardened net machinery (`netgit`: token only in
//!   the environment, no hooks, no ambient config) with an explicit lease on
//!   exactly `refs/heads/<head_ref>` and a single `<sha>:refs/heads/<head_ref>`
//!   refspec.
//!
//! The run result is a [`Outcome`], serialised as the run `summary`.

use std::path::Path;

use enzarb_proto::Retarget;
use serde_json::json;

use crate::exec::Output;

/// Branches enzarb creates start with this (`enzarb/issue-N`, `enzarb/task-N`,
/// `enzarb/<step>-N`; see `workflows/crates/engine/src/machine/issue.rs`).
/// Only such branches may be rewritten.
pub const BRANCH_PREFIX: &str = "enzarb/";

/// How a retarget run ended. Infrastructure failures are not an outcome: they
/// fail the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Rebased and pushed.
    Retargeted,
    /// The rebase conflicted; aborted, nothing pushed.
    Conflict,
    /// The remote head is not `expected_sha`, or the push lease was refused.
    LeaseFailed,
    /// `old_base` is not an ancestor of the remote head.
    NotAncestor,
    /// The branch holds a commit not authored by the sandbox.
    ForeignCommits,
    /// The request failed validation; no git call was made.
    Rejected,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retargeted => "retargeted",
            Self::Conflict => "conflict",
            Self::LeaseFailed => "lease_failed",
            Self::NotAncestor => "not_ancestor",
            Self::ForeignCommits => "foreign_commits",
            Self::Rejected => "rejected",
        }
    }
}

/// The result of a retarget run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub kind: Kind,
    /// The pushed sha for `retargeted`, else the current head (may be empty).
    pub head_sha: String,
    pub reason: String,
}

impl Outcome {
    fn new(kind: Kind, head_sha: &str, reason: impl Into<String>) -> Self {
        Self {
            kind,
            head_sha: head_sha.to_string(),
            reason: reason.into(),
        }
    }

    /// Single-line JSON: `{"outcome", "head_sha", "reason"}`.
    pub fn summary(&self) -> String {
        json!({"outcome": self.kind.as_str(), "head_sha": self.head_sha, "reason": self.reason})
            .to_string()
    }

    /// Only a completed retarget pushed something.
    pub fn pushed(&self) -> bool {
        self.kind == Kind::Retargeted
    }
}

/// A 40-character lowercase hex sha.
pub fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Plain branch-name syntax, stricter than git's: nothing that is an option, a
/// revision range, a refspec or a pattern.
pub fn valid_branch_name(n: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("invalid branch name {n:?}: {why}"));
    if n.is_empty() {
        return bad("empty");
    }
    if n.len() > 200 {
        return bad("too long");
    }
    if n.starts_with('-') {
        return bad("looks like an option");
    }
    if n.contains("..") || n.contains("@{") || n.contains("//") {
        return bad("forbidden sequence");
    }
    if n.starts_with('/') || n.ends_with('/') || n.ends_with('.') || n.ends_with(".lock") {
        return bad("forbidden start or end");
    }
    if n.split('/').any(|c| c.starts_with('.')) {
        return bad("component starts with '.'");
    }
    if n.chars().any(|c| {
        c.is_whitespace() || c.is_control() || matches!(c, ':' | '~' | '^' | '?' | '*' | '[' | '\\')
    }) {
        return bad("forbidden character");
    }
    Ok(())
}

/// Checks that need no git. `Err` is the `rejected` reason.
pub fn validate(p: &Retarget) -> Result<(), String> {
    valid_branch_name(&p.onto).map_err(|e| format!("onto: {e}"))?;
    valid_branch_name(&p.head_ref).map_err(|e| format!("head_ref: {e}"))?;
    if p.head_ref.len() <= BRANCH_PREFIX.len() || !p.head_ref.starts_with(BRANCH_PREFIX) {
        return Err(format!(
            "head_ref {:?} is not an enzarb branch (must start with {BRANCH_PREFIX:?})",
            p.head_ref
        ));
    }
    if p.head_ref == p.onto {
        return Err("head_ref is the default branch".into());
    }
    let short = |s: &str| s.strip_prefix("refs/heads/").unwrap_or(s).to_string();
    if p.protected.iter().any(|b| short(b) == p.head_ref) {
        return Err(format!("head_ref {:?} is protected", p.head_ref));
    }
    if !is_sha(&p.old_base) {
        return Err("old_base is not a 40-character hex sha".into());
    }
    if !is_sha(&p.expected_sha) {
        return Err("expected_sha is not a 40-character hex sha".into());
    }
    Ok(())
}

/// The identity commits must carry to be rewritten: the sandbox's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
}

/// What the retarget needs from its host: git in the clone, the hardened
/// fetch and the hardened push. The agent run manager implements it with the
/// run's cancellation, secrets and net environment; the tests with plain git.
#[allow(async_fn_in_trait)]
pub trait Git {
    type Err;
    /// An infrastructure failure (the run fails).
    fn fail(msg: String) -> Self::Err;
    /// Clone if needed, then fetch `refs/remotes/origin/*` into the clone.
    async fn fetch(&mut self) -> Result<(), Self::Err>;
    /// A token-free git call in `cwd`; a non-zero exit is returned, not an error.
    async fn local(&mut self, cwd: &Path, args: &[&str]) -> Result<Output, Self::Err>;
    /// `git push --no-verify <extra> -- <url> <refspec>` through the hardened path.
    async fn push(&mut self, extra: &[String], refspec: &str) -> Result<Output, Self::Err>;
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

async fn rev_parse<G: Git>(g: &mut G, repo: &Path, rev: &str) -> Result<Option<String>, G::Err> {
    let o = g
        .local(repo, &["rev-parse", "--verify", "--quiet", rev])
        .await?;
    Ok(o.success().then(|| text(&o)).filter(|s| is_sha(s)))
}

fn first_lines(s: &str, n: usize) -> String {
    let mut t: String = s.lines().take(n).collect::<Vec<_>>().join("; ");
    t.truncate(t.char_indices().nth(500).map_or(t.len(), |(i, _)| i));
    t
}

/// Run the retarget. `repo` is the clone, `worktree` a path for the throwaway
/// detached worktree (removed here, and by the caller's cleanup).
pub async fn run<G: Git>(
    g: &mut G,
    p: &Retarget,
    repo: &Path,
    worktree: &Path,
    author: &Author,
) -> Result<Outcome, G::Err> {
    if let Err(why) = validate(p) {
        let sha = if is_sha(&p.expected_sha) {
            p.expected_sha.as_str()
        } else {
            ""
        };
        return Ok(Outcome::new(Kind::Rejected, sha, why));
    }
    g.fetch().await?;

    let head_ref = format!("refs/remotes/origin/{}", p.head_ref);
    let Some(head) = rev_parse(g, repo, &format!("{head_ref}^{{commit}}")).await? else {
        return Ok(Outcome::new(
            Kind::LeaseFailed,
            "",
            "the branch no longer exists on the remote",
        ));
    };
    if head != p.expected_sha {
        return Ok(Outcome::new(
            Kind::LeaseFailed,
            &head,
            "the remote branch moved since the expected sha",
        ));
    }
    let onto_ref = format!("refs/remotes/origin/{}", p.onto);
    if rev_parse(g, repo, &format!("{onto_ref}^{{commit}}"))
        .await?
        .is_none()
    {
        return Ok(Outcome::new(
            Kind::Rejected,
            &head,
            format!("onto branch {:?} not found on the remote", p.onto),
        ));
    }

    let anc = g
        .local(repo, &["merge-base", "--is-ancestor", &p.old_base, &head])
        .await?;
    if !anc.success() {
        return Ok(Outcome::new(
            Kind::NotAncestor,
            &head,
            "old_base is not an ancestor of the branch head",
        ));
    }

    let range = format!("{}..{head}", p.old_base);
    let log = g
        .local(
            repo,
            &[
                "log",
                "--no-merges",
                "--format=%H%x1f%an%x1f%ae",
                &range,
                "--",
            ],
        )
        .await?;
    if !log.success() {
        return Err(G::fail(format!(
            "git log failed ({:?}): {}",
            log.code(),
            log.stderr.trim()
        )));
    }
    for line in text(&log).lines() {
        let mut f = line.split('\x1f');
        let (sha, name, email) = (f.next(), f.next(), f.next());
        if name != Some(author.name.as_str()) || email != Some(author.email.as_str()) {
            return Ok(Outcome::new(
                Kind::ForeignCommits,
                &head,
                format!(
                    "commit {} is not authored by the sandbox identity",
                    sha.unwrap_or("?")
                ),
            ));
        }
    }

    // Throwaway detached worktree at the remote head.
    let w = worktree.to_string_lossy().into_owned();
    let _ = std::fs::remove_dir_all(worktree);
    let _ = g.local(repo, &["worktree", "prune"]).await?;
    let add = g
        .local(repo, &["worktree", "add", "--detach", "-f", &w, &head])
        .await?;
    if !add.success() {
        return Err(G::fail(format!(
            "git worktree add failed ({:?}): {}",
            add.code(),
            add.stderr.trim()
        )));
    }
    let rb = g
        .local(
            worktree,
            &[
                "-c",
                "commit.gpgsign=false",
                "rebase",
                "--onto",
                &onto_ref,
                &p.old_base,
            ],
        )
        .await?;
    if !rb.success() {
        let _ = g.local(worktree, &["rebase", "--abort"]).await?;
        let all = format!("{}\n{}", text(&rb), rb.stderr);
        let conflicts: Vec<&str> = all.lines().filter(|l| l.contains("CONFLICT")).collect();
        let why = if conflicts.is_empty() {
            first_lines(rb.stderr.trim(), 3)
        } else {
            first_lines(&conflicts.join("\n"), 5)
        };
        return Ok(Outcome::new(
            Kind::Conflict,
            &head,
            format!("rebase aborted: {why}"),
        ));
    }
    let Some(new) = rev_parse(g, worktree, "HEAD").await? else {
        return Err(G::fail("cannot read HEAD after the rebase".into()));
    };

    // The one lease push: exactly this branch, only if it is still `expected_sha`.
    let lease = format!(
        "--force-with-lease=refs/heads/{}:{}",
        p.head_ref, p.expected_sha
    );
    let refspec = format!("{new}:refs/heads/{}", p.head_ref);
    let out = g.push(&[lease], &refspec).await?;
    if out.success() {
        return Ok(Outcome::new(Kind::Retargeted, &new, "rebased and pushed"));
    }
    let err = out.stderr.trim();
    if err.contains("stale info") || err.contains("[rejected]") {
        return Ok(Outcome::new(
            Kind::LeaseFailed,
            &head,
            "the push lease was refused: the remote branch moved",
        ));
    }
    Err(G::fail(format!(
        "git push failed ({:?}): {}",
        out.code(),
        first_lines(err, 5)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::ExitStatus;
    use std::path::PathBuf;
    use std::process::Command;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn req() -> Retarget {
        Retarget {
            head_ref: "enzarb/issue-2".into(),
            onto: "main".into(),
            old_base: SHA_A.into(),
            expected_sha: SHA_B.into(),
            protected: vec![],
        }
    }

    #[test]
    fn valid_request_passes() {
        assert_eq!(validate(&req()), Ok(()));
        let mut r = req();
        r.protected = vec!["main".into(), "release".into()];
        assert_eq!(validate(&r), Ok(()));
    }

    #[test]
    fn validation_table() {
        type Case = (&'static str, Box<dyn Fn(&mut Retarget)>);
        let cases: Vec<Case> = vec![
            ("empty head", Box::new(|r| r.head_ref = String::new())),
            ("no prefix", Box::new(|r| r.head_ref = "feature/x".into())),
            ("bare prefix", Box::new(|r| r.head_ref = "enzarb/".into())),
            ("default branch", Box::new(|r| r.head_ref = "main".into())),
            (
                "equals onto",
                Box::new(|r| {
                    r.onto = "enzarb/issue-2".into();
                }),
            ),
            (
                "protected",
                Box::new(|r| r.protected = vec!["enzarb/issue-2".into()]),
            ),
            (
                "protected full ref",
                Box::new(|r| r.protected = vec!["refs/heads/enzarb/issue-2".into()]),
            ),
            ("option", Box::new(|r| r.head_ref = "--enzarb/x".into())),
            ("dotdot", Box::new(|r| r.head_ref = "enzarb/a..b".into())),
            ("colon", Box::new(|r| r.head_ref = "enzarb/a:b".into())),
            ("space", Box::new(|r| r.head_ref = "enzarb/a b".into())),
            ("tab", Box::new(|r| r.head_ref = "enzarb/a\tb".into())),
            ("tilde", Box::new(|r| r.head_ref = "enzarb/a~1".into())),
            ("caret", Box::new(|r| r.head_ref = "enzarb/a^".into())),
            ("question", Box::new(|r| r.head_ref = "enzarb/a?".into())),
            ("star", Box::new(|r| r.head_ref = "enzarb/*".into())),
            ("bracket", Box::new(|r| r.head_ref = "enzarb/[a]".into())),
            ("backslash", Box::new(|r| r.head_ref = "enzarb/a\\b".into())),
            ("reflog", Box::new(|r| r.head_ref = "enzarb/a@{1}".into())),
            ("lock", Box::new(|r| r.head_ref = "enzarb/a.lock".into())),
            ("short old_base", Box::new(|r| r.old_base = "abc".into())),
            ("empty old_base", Box::new(|r| r.old_base = String::new())),
            (
                "upper old_base",
                Box::new(|r| r.old_base = SHA_A.to_uppercase()),
            ),
            (
                "non-hex expected",
                Box::new(|r| r.expected_sha = "z".repeat(40)),
            ),
            (
                "ref as expected",
                Box::new(|r| r.expected_sha = "refs/heads/main".into()),
            ),
            ("empty onto", Box::new(|r| r.onto = String::new())),
            ("option onto", Box::new(|r| r.onto = "-main".into())),
            ("bad onto", Box::new(|r| r.onto = "ma in".into())),
        ];
        for (name, f) in cases {
            let mut r = req();
            f(&mut r);
            assert!(validate(&r).is_err(), "{name} must be rejected: {r:?}");
        }
    }

    #[test]
    fn summary_is_one_line_json() {
        let o = Outcome::new(Kind::Conflict, SHA_A, "multi\nline \"x\"");
        let s = o.summary();
        assert!(!s.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["outcome"], "conflict");
        assert_eq!(v["head_sha"], SHA_A);
        assert!(!o.pushed());
        assert!(Outcome::new(Kind::Retargeted, SHA_A, "").pushed());
    }

    /// No other source file may know the force flags.
    #[test]
    fn force_push_flags_exist_only_in_this_module() {
        let needles = [format!("--{}", "force"), format!("{}-with-lease", "force")];
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![dir];
        let mut seen = 0;
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") || p.ends_with("retarget.rs") {
                    continue;
                }
                // The agent permission policy names the flags to DENY them.
                if p.components().any(|c| c.as_os_str() == "policy")
                    || p.file_name().is_some_and(|n| n == "policy.rs")
                {
                    continue;
                }
                seen += 1;
                for (i, l) in std::fs::read_to_string(&p).unwrap().lines().enumerate() {
                    if l.trim_start().starts_with("//") {
                        continue;
                    }
                    for n in &needles {
                        if l.contains(n.as_str()) {
                            panic!("{}:{}: {l}", p.display(), i + 1);
                        }
                    }
                }
            }
        }
        assert!(seen > 5, "scanned {seen} files");
    }

    // ---- real git ----

    const ME: (&str, &str) = ("enzarb-agent", "agent@enzarb.dev");
    const HUMAN: (&str, &str) = ("Alice", "alice@example.com");

    fn git_in(dir: &Path, who: (&str, &str), args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", who.0)
            .env("GIT_AUTHOR_EMAIL", who.1)
            .env("GIT_COMMITTER_NAME", who.0)
            .env("GIT_COMMITTER_EMAIL", who.1)
            .output()
            .unwrap()
    }

    fn ok(dir: &Path, who: (&str, &str), args: &[&str]) -> String {
        let o = git_in(dir, who, args);
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn commit(dir: &Path, who: (&str, &str), file: &str, content: &str, msg: &str) -> String {
        std::fs::write(dir.join(file), content).unwrap();
        ok(dir, who, &["add", "-A"]);
        ok(dir, who, &["commit", "-q", "-m", msg]);
        ok(dir, who, &["rev-parse", "HEAD"])
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        remote: PathBuf,
        url: String,
        seed: PathBuf,
        repo: PathBuf,
        wt: PathBuf,
    }

    impl Fx {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let remote = root.join("remote.git");
            let seed = root.join("seed");
            std::fs::create_dir_all(&remote).unwrap();
            std::fs::create_dir_all(&seed).unwrap();
            ok(&remote, ME, &["init", "-q", "--bare", "-b", "main"]);
            ok(&seed, ME, &["init", "-q", "-b", "main"]);
            let url = format!("file://{}", remote.display());
            ok(&seed, ME, &["remote", "add", "origin", &url]);
            Self {
                url,
                repo: root.join("repo"),
                wt: root.join("wt"),
                _tmp: tmp,
                remote,
                seed,
            }
        }

        fn push_seed(&self, refspec: &str) {
            ok(&self.seed, ME, &["push", "-q", "origin", refspec]);
        }

        fn remote_ref(&self, r: &str) -> String {
            ok(&self.remote, ME, &["rev-parse", r])
        }

        /// main: base. parent branch (2 commits). child cut from the parent tip
        /// (1 commit by `child_author`). main then gets the squash of the parent.
        /// Returns (parent_tip, child_tip).
        fn squash_scenario(&self, child_author: (&str, &str)) -> (String, String) {
            let s = &self.seed;
            commit(s, HUMAN, "base.txt", "base\n", "base");
            self.push_seed("main");
            ok(s, ME, &["checkout", "-q", "-b", "enzarb/issue-1"]);
            commit(s, ME, "parent.txt", "p1\n", "parent 1");
            let parent = commit(s, ME, "shared.txt", "parent\n", "parent 2");
            ok(s, ME, &["checkout", "-q", "-b", "enzarb/issue-2"]);
            let child = commit(s, child_author, "shared.txt", "child\n", "child 1");
            ok(s, ME, &["checkout", "-q", "main"]);
            ok(s, HUMAN, &["merge", "--squash", "-q", "enzarb/issue-1"]);
            ok(s, HUMAN, &["commit", "-q", "-m", "parent (#1)"]);
            self.push_seed("main");
            self.push_seed("enzarb/issue-1");
            self.push_seed("enzarb/issue-2");
            ok(
                &self.seed,
                ME,
                &["clone", "-q", &self.url, self.repo.to_str().unwrap()],
            );
            (parent, child)
        }

        fn retarget(&self, parent: &str, child: &str) -> Retarget {
            Retarget {
                head_ref: "enzarb/issue-2".into(),
                onto: "main".into(),
                old_base: parent.into(),
                expected_sha: child.into(),
                protected: vec![],
            }
        }
    }

    struct TestGit<'a> {
        fx: &'a Fx,
        pushes: usize,
        before_push: Option<Box<dyn FnMut() + 'a>>,
    }

    impl Git for TestGit<'_> {
        type Err = String;
        fn fail(msg: String) -> String {
            msg
        }
        async fn fetch(&mut self) -> Result<(), String> {
            let o = git_in(
                &self.fx.repo,
                ME,
                &[
                    "fetch",
                    "-q",
                    "--prune",
                    "--no-tags",
                    "--",
                    &self.fx.url,
                    "+refs/heads/*:refs/remotes/origin/*",
                ],
            );
            o.status
                .success()
                .then_some(())
                .ok_or_else(|| String::from_utf8_lossy(&o.stderr).into_owned())
        }
        async fn local(&mut self, cwd: &Path, args: &[&str]) -> Result<Output, String> {
            Ok(wrap(git_in(cwd, ME, args)))
        }
        async fn push(&mut self, extra: &[String], refspec: &str) -> Result<Output, String> {
            if let Some(f) = self.before_push.as_mut() {
                f();
            }
            self.pushes += 1;
            let mut args: Vec<&str> = vec!["push", "--no-verify"];
            args.extend(extra.iter().map(String::as_str));
            args.extend(["--", &self.fx.url, refspec]);
            Ok(wrap(git_in(&self.fx.repo, ME, &args)))
        }
    }

    fn wrap(o: std::process::Output) -> Output {
        Output {
            status: Some(ExitStatus::Exited(o.status.code().unwrap_or(-1))),
            stop: None,
            stdout: o.stdout,
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn me() -> Author {
        Author {
            name: ME.0.into(),
            email: ME.1.into(),
        }
    }

    async fn go(fx: &Fx, p: &Retarget, g: &mut TestGit<'_>) -> Outcome {
        run(g, p, &fx.repo, &fx.wt, &me()).await.unwrap()
    }

    fn tg(fx: &Fx) -> TestGit<'_> {
        TestGit {
            fx,
            pushes: 0,
            before_push: None,
        }
    }

    #[tokio::test]
    async fn squash_merged_parent_is_retargeted() {
        let fx = Fx::new();
        let (parent, child) = fx.squash_scenario(ME);
        let main = fx.remote_ref("main");
        let mut g = tg(&fx);
        let o = go(&fx, &fx.retarget(&parent, &child), &mut g).await;
        assert_eq!(o.kind, Kind::Retargeted, "{o:?}");
        assert!(o.pushed());
        let now = fx.remote_ref("refs/heads/enzarb/issue-2");
        assert_eq!(o.head_sha, now);
        assert_ne!(now, child);
        // Only the child's own commit sits on top of main; main is untouched.
        assert_eq!(fx.remote_ref("main"), main);
        let only = ok(
            &fx.remote,
            ME,
            &["rev-list", "--count", &format!("{main}..{now}")],
        );
        assert_eq!(only, "1");
        assert_eq!(
            ok(&fx.remote, ME, &["rev-parse", &format!("{now}~1")]),
            main
        );
        assert_eq!(
            ok(&fx.remote, ME, &["show", &format!("{now}:shared.txt")]),
            "child"
        );
        // The parent branch was not touched.
        assert_eq!(fx.remote_ref("refs/heads/enzarb/issue-1"), parent);
        let v: serde_json::Value = serde_json::from_str(&o.summary()).unwrap();
        assert_eq!(v["outcome"], "retargeted");
    }

    #[tokio::test]
    async fn conflict_aborts_and_leaves_the_remote_alone() {
        let fx = Fx::new();
        let (parent, child) = fx.squash_scenario(ME);
        // main moves on and touches the same line the child changed.
        commit(&fx.seed, HUMAN, "shared.txt", "other\n", "main moves");
        fx.push_seed("main");
        let mut g = tg(&fx);
        let o = go(&fx, &fx.retarget(&parent, &child), &mut g).await;
        assert_eq!(o.kind, Kind::Conflict, "{o:?}");
        assert!(!o.pushed());
        assert_eq!(g.pushes, 0);
        assert_eq!(fx.remote_ref("refs/heads/enzarb/issue-2"), child);
        // The throwaway worktree is not left mid-rebase.
        assert!(!fx.wt.join(".git").is_dir());
        let st = ok(&fx.wt, ME, &["status", "--porcelain"]);
        assert!(!st.contains("UU"), "{st}");
    }

    #[tokio::test]
    async fn moved_remote_head_is_a_lease_failure() {
        let fx = Fx::new();
        let (parent, child) = fx.squash_scenario(ME);
        ok(&fx.seed, ME, &["checkout", "-q", "enzarb/issue-2"]);
        let moved = commit(&fx.seed, HUMAN, "extra.txt", "x\n", "human commit");
        fx.push_seed("enzarb/issue-2");
        let mut g = tg(&fx);
        let o = go(&fx, &fx.retarget(&parent, &child), &mut g).await;
        assert_eq!(o.kind, Kind::LeaseFailed, "{o:?}");
        assert_eq!(o.head_sha, moved);
        assert_eq!(g.pushes, 0);
        assert_eq!(fx.remote_ref("refs/heads/enzarb/issue-2"), moved);
    }

    #[tokio::test]
    async fn lease_refuses_a_branch_that_moves_during_the_run() {
        let fx = Fx::new();
        let (parent, child) = fx.squash_scenario(ME);
        ok(&fx.seed, ME, &["checkout", "-q", "enzarb/issue-2"]);
        let seed = fx.seed.clone();
        let remote_url = fx.url.clone();
        let mut g = tg(&fx);
        g.before_push = Some(Box::new(move || {
            let moved = commit(&seed, HUMAN, "late.txt", "x\n", "late human commit");
            ok(
                &seed,
                ME,
                &["push", "-q", "--no-verify", &remote_url, "enzarb/issue-2"],
            );
            assert_eq!(moved.len(), 40);
        }));
        let o = go(&fx, &fx.retarget(&parent, &child), &mut g).await;
        assert_eq!(o.kind, Kind::LeaseFailed, "{o:?}");
        assert!(!o.pushed());
        // The late commit survives; the rebased history was not forced over it.
        let tip = fx.remote_ref("refs/heads/enzarb/issue-2");
        assert_ne!(tip, child);
        assert_eq!(
            ok(&fx.remote, ME, &["show", &format!("{tip}:late.txt")]),
            "x"
        );
    }

    #[tokio::test]
    async fn foreign_author_is_not_rewritten() {
        let fx = Fx::new();
        let (parent, child) = fx.squash_scenario(HUMAN);
        let mut g = tg(&fx);
        let o = go(&fx, &fx.retarget(&parent, &child), &mut g).await;
        assert_eq!(o.kind, Kind::ForeignCommits, "{o:?}");
        assert_eq!(g.pushes, 0);
        assert_eq!(fx.remote_ref("refs/heads/enzarb/issue-2"), child);
    }

    #[tokio::test]
    async fn same_name_different_email_is_foreign() {
        let fx = Fx::new();
        let (parent, child) = fx.squash_scenario((ME.0, "mallory@example.com"));
        let mut g = tg(&fx);
        let o = go(&fx, &fx.retarget(&parent, &child), &mut g).await;
        assert_eq!(o.kind, Kind::ForeignCommits, "{o:?}");
    }

    #[tokio::test]
    async fn unrelated_old_base_is_not_an_ancestor() {
        let fx = Fx::new();
        let (_parent, child) = fx.squash_scenario(ME);
        // A real commit that is not in the child's history.
        ok(&fx.seed, ME, &["checkout", "-q", "main"]);
        let other = commit(&fx.seed, HUMAN, "o.txt", "o\n", "unrelated");
        fx.push_seed("main");
        let mut g = tg(&fx);
        let o = go(&fx, &fx.retarget(&other, &child), &mut g).await;
        assert_eq!(o.kind, Kind::NotAncestor, "{o:?}");
        assert_eq!(g.pushes, 0);
        assert_eq!(fx.remote_ref("refs/heads/enzarb/issue-2"), child);
    }

    #[tokio::test]
    async fn rejected_request_makes_no_git_call() {
        let fx = Fx::new();
        let mut g = tg(&fx);
        let mut p = fx.retarget(SHA_A, SHA_B);
        p.head_ref = "main".into();
        // `repo` does not even exist: any git call would fail the run.
        let o = go(&fx, &p, &mut g).await;
        assert_eq!(o.kind, Kind::Rejected);
        assert_eq!(o.head_sha, SHA_B);
        assert_eq!(g.pushes, 0);
    }
}
