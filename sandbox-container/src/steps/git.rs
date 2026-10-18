//! Git for step workspaces: clone/fetch with the workspace token, branch
//! checkout, commit and the leased push.
//!
//! Tokens never touch disk or a repo's config: every authenticated call runs
//! in a scratch bare repo borrowing the workspace's objects, with the
//! hardened environment of [`crate::netgit`] and the auth header passed only
//! as `GIT_CONFIG_*` environment.

use std::path::{Path, PathBuf};
use std::time::Duration;

use enzarb_proto::{StepGit, StepWorkspace};

use crate::config::Config;
use crate::exec::{self, CmdSpec, Ctl, LineSink, Output};
use crate::netgit::{self, NetDir};
use crate::reaper::Reaper;

/// Git context of one step.
pub struct Git<'a> {
    pub cfg: &'a Config,
    pub reaper: &'a Reaper,
    pub log: LineSink,
}

/// The base64 basic-auth value of `token` (also a redaction form).
pub fn basic_b64(token: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"))
}

/// A branch name sandbox-init accepts.
pub fn valid_branch(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with(['-', '/', '.'])
        && !s.ends_with(['/', '.'])
        && !s.ends_with(".lock")
        && !s.contains("..")
        && !s.contains("//")
        && !s.contains("@{")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
}

/// `owner/repo` with safe segments.
pub fn valid_repo(s: &str) -> bool {
    let mut parts = s.split('/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && !p.starts_with('.')
                && p.len() <= 100
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

impl Git<'_> {
    fn identity(&self) -> Vec<(String, String)> {
        let c = self.cfg;
        vec![
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_AUTHOR_NAME".into(), c.git_name.clone()),
            ("GIT_AUTHOR_EMAIL".into(), c.git_email.clone()),
            ("GIT_COMMITTER_NAME".into(), c.git_name.clone()),
            ("GIT_COMMITTER_EMAIL".into(), c.git_email.clone()),
        ]
    }

    fn net_env(&self, ws: &StepWorkspace) -> Result<Vec<(String, String)>, String> {
        let header = format!("Authorization: basic {}", basic_b64(ws.token.expose()));
        let header = (!ws.token.expose().is_empty()).then_some(header);
        netgit::env(&ws.url, header.as_deref(), &self.identity())
    }

    /// A token-free git call in `cwd` (hooks and fsmonitor disabled).
    pub async fn local(&self, ctl: &mut Ctl, cwd: &Path, args: &[&str]) -> Result<Output, String> {
        let mut spec = CmdSpec::new(&self.cfg.git_bin, &netgit::local_args(args));
        spec.cwd = Some(cwd.to_path_buf());
        spec.env = self.identity();
        let out = exec::run(self.reaper, spec, ctl, Some(self.log.clone()))
            .await
            .map_err(|e| format!("cannot run git: {e}"))?;
        if let Some(s) = out.stop {
            return Err(format!(
                "git {} stopped: {s:?}",
                args.first().unwrap_or(&"")
            ));
        }
        Ok(out)
    }

    async fn local_ok(&self, ctl: &mut Ctl, cwd: &Path, args: &[&str]) -> Result<Output, String> {
        let out = self.local(ctl, cwd, args).await?;
        if out.success() {
            Ok(out)
        } else {
            Err(format!(
                "git {} failed ({:?}): {}",
                args.first().unwrap_or(&""),
                out.code(),
                out.stderr.trim()
            ))
        }
    }

    async fn rev(&self, ctl: &mut Ctl, cwd: &Path, r: &str) -> Result<Option<String>, String> {
        let out = self
            .local(ctl, cwd, &["rev-parse", "--verify", "--quiet", r])
            .await?;
        Ok(out
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
    }

    /// An authenticated call with retries on transient network errors.
    async fn net(
        &self,
        ctl: &mut Ctl,
        env: &[(String, String)],
        cwd: &Path,
        args: &[&str],
        cap: Duration,
    ) -> Result<Output, String> {
        let attempts = (self.cfg.net_retry_delays.len() + 1).min(netgit::NET_ATTEMPTS);
        let mut attempt = 1;
        loop {
            let mut spec = CmdSpec::new(&self.cfg.git_bin, args);
            spec.cwd = Some(cwd.to_path_buf());
            spec.env = env.to_vec();
            spec.env_clear = true;
            let mut child = ctl.clone();
            let attempt_end = tokio::time::Instant::now() + cap;
            child.deadline = Some(child.deadline.map_or(attempt_end, |d| d.min(attempt_end)));
            let out = exec::run(self.reaper, spec, &mut child, Some(self.log.clone()))
                .await
                .map_err(|e| format!("cannot run git: {e}"))?;
            if *ctl.cancel.borrow() {
                return Err("stopped".into());
            }
            let transient = out.stop.is_some() || netgit::is_transient(&out.stderr);
            if out.success() || attempt >= attempts || !transient {
                return Ok(out);
            }
            let delay = self.cfg.net_retry_delays[attempt - 1];
            (self.log)(&format!(
                "git {} failed transiently (attempt {attempt}/{attempts}); retrying in {}s",
                args.first().unwrap_or(&""),
                delay.as_secs()
            ));
            tokio::select! {
                _ = ctl.stopped() => return Err("stopped".into()),
                () = tokio::time::sleep(delay) => {}
            }
            attempt += 1;
        }
    }

    /// A fresh bare repo for one network call, borrowing `repo`'s objects.
    async fn net_repo(&self, ctl: &mut Ctl, tag: &str, repo: &Path) -> Result<NetDir, String> {
        let dir = netgit::net_dir(&self.cfg.workspace, tag).map_err(|e| e.to_string())?;
        let parent = dir.parent().unwrap_or(&self.cfg.workspace).to_path_buf();
        let _ = crate::privs::create_dir_all_mode(&parent, Some(0o700));
        let guard = NetDir(dir.clone());
        let d = dir.to_string_lossy().into_owned();
        self.local_ok(ctl, &parent, &["init", "-q", "--bare", "--template=", &d])
            .await?;
        let f = dir.join("objects/info/alternates");
        crate::privs::create_dir_all(&dir.join("objects/info"))
            .and_then(|()| std::fs::write(&f, format!("{}\n", repo.join(".git/objects").display())))
            .and_then(|()| crate::privs::own(&f))
            .map_err(|e| format!("cannot prepare the fetch repo: {e}"))?;
        Ok(guard)
    }

    async fn fetch(&self, ctl: &mut Ctl, ws: &StepWorkspace, dir: &Path) -> Result<(), String> {
        let env = self.net_env(ws)?;
        let net = self.net_repo(ctl, "fetch", dir).await?;
        let out = self
            .net(
                ctl,
                &env,
                &net.0,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--",
                    &ws.url,
                    "+refs/heads/*:refs/remotes/origin/*",
                ],
                self.cfg.fetch_attempt_cap,
            )
            .await?;
        if !out.success() {
            return Err(format!("git fetch failed: {}", out.stderr.trim()));
        }
        let n = net.0.to_string_lossy().into_owned();
        self.local_ok(
            ctl,
            dir,
            &[
                "fetch",
                "--prune",
                "--no-tags",
                "--",
                &n,
                "+refs/remotes/origin/*:refs/remotes/origin/*",
            ],
        )
        .await?;
        Ok(())
    }

    async fn clone(&self, ctl: &mut Ctl, ws: &StepWorkspace, dir: &Path) -> Result<(), String> {
        let env = self.net_env(ws)?;
        let tmp = netgit::net_dir(&self.cfg.workspace, "clone").map_err(|e| e.to_string())?;
        let parent = tmp.parent().unwrap_or(&self.cfg.workspace).to_path_buf();
        let _ = crate::privs::create_dir_all_mode(&parent, Some(0o700));
        let tmp = NetDir(tmp);
        let t = tmp.0.to_string_lossy().into_owned();
        let out = self
            .net(
                ctl,
                &env,
                &parent,
                &["clone", "--no-checkout", "--template=", "--", &ws.url, &t],
                self.cfg.fetch_attempt_cap,
            )
            .await?;
        if !out.success() {
            return Err(format!("git clone failed: {}", out.stderr.trim()));
        }
        // The clone's origin must not carry anything but the plain URL.
        self.local_ok(ctl, &tmp.0, &["remote", "set-url", "origin", &ws.url])
            .await?;
        if let Some(p) = dir.parent() {
            crate::privs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp.0, dir).map_err(|e| format!("cannot move the clone: {e}"))
    }

    /// Clone or fetch, check out `branch`, and return the push lease: the
    /// remote head of `branch` now (empty when it does not exist).
    pub async fn prepare(
        &self,
        ctl: &mut Ctl,
        ws: &StepWorkspace,
        dir: &Path,
        lease: Option<&str>,
    ) -> Result<String, String> {
        if dir.join(".git").exists() {
            self.fetch(ctl, ws, dir).await?;
        } else {
            let _ = std::fs::remove_dir_all(dir);
            self.clone(ctl, ws, dir).await?;
            // The clone is `--no-checkout`: its current branch (the default one) has an empty
            // index, and checking out that same branch below would change nothing. Populate it
            // now; a fresh clone has nothing local to lose. (An empty repository has no HEAD.)
            let _ = self
                .local(ctl, dir, &["reset", "-q", "--hard", "HEAD"])
                .await?;
        }
        // Step outputs (`.enzarb/out/`) are collected after each step and never belong in the
        // repository: drop stale ones (a branch that committed them then checks out cleanly)
        // and keep new ones out of commits.
        let _ = std::fs::remove_dir_all(dir.join(OUTPUT_DIR));
        // Copies tracked by HEAD (and the staged untracking of an earlier step) would count as
        // local changes and abort the checkout: restore them to HEAD first.
        let _ = self
            .local(ctl, dir, &["reset", "-q", "--", OUTPUT_DIR])
            .await?;
        let _ = self
            .local(ctl, dir, &["checkout", "-q", "HEAD", "--", OUTPUT_DIR])
            .await?;
        // The workspace's `.git` may be a file (separate git dir): ask git where its exclude is.
        let out = self
            .local(ctl, dir, &["rev-parse", "--git-path", "info/exclude"])
            .await?;
        if out.success() {
            let rel = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !rel.is_empty() {
                exclude(&dir.join(rel), &format!("/{OUTPUT_DIR}/"));
            }
        }
        let remote = self
            .rev(ctl, dir, &format!("refs/remotes/origin/{}", ws.branch))
            .await?;
        let local = self
            .rev(ctl, dir, &format!("refs/heads/{}", ws.branch))
            .await?;
        // Uncommitted changes on the default branch are never work: steps commit and push only
        // on their own task branch. They are left by a read step, or by a clone that was never
        // checked out (before the fix to `clone` above), and would block switching to the task
        // branch. Discard them before leaving the default branch.
        let current = self
            .local(ctl, dir, &["symbolic-ref", "--short", "-q", "HEAD"])
            .await?;
        let current = String::from_utf8_lossy(&current.stdout).trim().to_string();
        if current == ws.base && current != ws.branch {
            let _ = self
                .local(ctl, dir, &["reset", "-q", "--hard", "HEAD"])
                .await?;
        }
        match (&local, &remote) {
            (Some(_), r) => {
                self.local_ok(ctl, dir, &["checkout", "-q", &ws.branch])
                    .await?;
                // Take what others pushed when it is a fast-forward of ours.
                if r.is_some() {
                    let up = format!("origin/{}", ws.branch);
                    let _ = self
                        .local(ctl, dir, &["merge", "-q", "--ff-only", &up])
                        .await?;
                }
            }
            (None, Some(_)) => {
                let up = format!("origin/{}", ws.branch);
                self.local_ok(ctl, dir, &["checkout", "-q", "-B", &ws.branch, &up])
                    .await?;
            }
            (None, None) => {
                let base = format!("origin/{}", ws.base);
                if self.rev(ctl, dir, &base).await?.is_none() {
                    return Err(format!("base branch {:?} does not exist", ws.base));
                }
                self.local_ok(
                    ctl,
                    dir,
                    &["checkout", "-q", "--no-track", "-B", &ws.branch, &base],
                )
                .await?;
            }
        }
        // A branch that already tracks outputs (committed before they were excluded): untrack
        // them so the next commit removes them from the change.
        let _ = self
            .local(
                ctl,
                dir,
                &["rm", "-r", "-q", "--cached", "--ignore-unmatch", OUTPUT_DIR],
            )
            .await?;
        // The branch's tracked copies are stale for this step.
        let _ = std::fs::remove_dir_all(dir.join(OUTPUT_DIR));
        Ok(lease
            .map(str::to_string)
            .unwrap_or_else(|| remote.unwrap_or_default()))
    }

    /// Commit everything (when `push`) and push `branch` with the lease.
    pub async fn finish(
        &self,
        ctl: &mut Ctl,
        ws: &StepWorkspace,
        dir: &Path,
        lease: &str,
        run_id: &str,
    ) -> Result<StepGit, (StepGit, String)> {
        let mut fact = StepGit {
            repo: ws.repo.clone(),
            branch: ws.branch.clone(),
            head_sha: String::new(),
            pushed: false,
        };
        let r = async {
            if ws.push {
                let status = self.local_ok(ctl, dir, &["status", "--porcelain"]).await?;
                if !status.stdout.iter().all(u8::is_ascii_whitespace) {
                    self.local_ok(ctl, dir, &["add", "-A"]).await?;
                    let msg = format!("enzarb: changes from step run {run_id}");
                    self.local_ok(
                        ctl,
                        dir,
                        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", &msg],
                    )
                    .await?;
                }
            }
            let head = self.rev(ctl, dir, "HEAD").await?.unwrap_or_default();
            Ok::<String, String>(head)
        }
        .await;
        match r {
            Ok(h) => fact.head_sha = h,
            Err(e) => return Err((fact, e)),
        }
        if !ws.push {
            return Ok(fact);
        }
        let unchanged = if lease.is_empty() {
            // A new branch with nothing on top of its base is not created.
            self.rev(ctl, dir, &format!("refs/remotes/origin/{}", ws.base))
                .await
                .ok()
                .flatten()
                .as_deref()
                == Some(fact.head_sha.as_str())
        } else {
            lease == fact.head_sha
        };
        if unchanged || fact.head_sha.is_empty() {
            return Ok(fact);
        }
        let env = match self.net_env(ws) {
            Ok(e) => e,
            Err(e) => return Err((fact, e)),
        };
        let net = match self.net_repo(ctl, "push", dir).await {
            Ok(n) => n,
            Err(e) => return Err((fact, e)),
        };
        let lease_arg = format!("--force-with-lease=refs/heads/{}:{lease}", ws.branch);
        let refspec = format!("{}:refs/heads/{}", fact.head_sha, ws.branch);
        let out = self
            .net(
                ctl,
                &env,
                &net.0,
                &["push", "--no-verify", &lease_arg, "--", &ws.url, &refspec],
                self.cfg.push_attempt_cap,
            )
            .await;
        match out {
            Ok(o) if o.success() => {
                fact.pushed = true;
                // Keep the remote-tracking ref in step for the next lease.
                let r = format!("refs/remotes/origin/{}", ws.branch);
                let _ = self
                    .local(ctl, dir, &["update-ref", &r, &fact.head_sha])
                    .await;
                Ok(fact)
            }
            Ok(o) => {
                let e = format!("git push failed ({:?}): {}", o.code(), o.stderr.trim());
                Err((fact, e))
            }
            Err(e) => Err((fact, e)),
        }
    }
}

/// `<ws>/tasks/<task_id>/<owner>/<repo>`.
pub fn workspace_dir(root: &Path, task_id: &str, repo: &str) -> PathBuf {
    root.join("tasks").join(task_id).join(repo)
}

/// Where steps write their declared output files, relative to the workspace.
pub const OUTPUT_DIR: &str = ".enzarb/out";

/// Add `pattern` to the repository's local exclude file (`path`, from `git rev-parse
/// --git-path info/exclude`) once.
fn exclude(path: &Path, pattern: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let cur = std::fs::read_to_string(path).unwrap_or_default();
    if cur.lines().any(|l| l == pattern) {
        return;
    }
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let sep = if cur.is_empty() || cur.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let _ = writeln!(f, "{sep}{pattern}");
    }
}
