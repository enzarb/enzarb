//! The reducer (ADR-0008 kept, ADR-0022 §6): an inbox with dedupe keys, one event per
//! transaction, a row lock per task, effects queued and the projection written in the same
//! transaction. Routing: a linked subject goes to its task; a PR referencing issues is linked to
//! their task; anything else goes through the merged bindings.

use enzarb_connector::{Actor as CActor, WorkItemKey};
use enzarb_flow::document::{Subject, SubjectComment, TaskDocument, TaskInfo};
use enzarb_flow::events::{Event, Payload, SubjectKind};
use enzarb_flow::interp::{self, Effect, Status, TaskEvent, TaskState};
use enzarb_flow::resolve::{Entry, Resolved};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use tokio::sync::watch;

use crate::engine::{Engine, repo_key, subject_key};
use crate::error::Error;
use crate::index::ProjectInfo;

type Tx<'a> = Transaction<'a, Postgres>;

/// Inbox `type` prefix of engine-produced events (payload = a `TaskEvent`).
pub const ENGINE_PREFIX: &str = "engine:";

pub(crate) struct TaskRow {
    pub id: i64,
    pub project: String,
    pub repo: String,
    pub state: TaskState,
    pub resolved: Resolved,
}

/// Delay before retry number `attempts` (1-based) of a failing event: 2 s doubling to 5 min.
/// There is no cap on attempts.
pub fn retry_delay(attempts: i32) -> std::time::Duration {
    let exp = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(16);
    std::time::Duration::from_secs((2u64 << exp).min(300))
}

impl Engine {
    /// Runs until shutdown: wakes on `NOTIFY engine_events` and on a poll interval.
    pub async fn run_reducer(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), Error> {
        let mut listener = sqlx::postgres::PgListener::connect_with(&self.pool).await?;
        listener.listen("engine_events").await?;
        loop {
            while self.process_next().await? {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                _ = listener.recv() => {}
                _ = tokio::time::sleep(self.cfg.poll) => {}
            }
        }
    }

    /// Processes one inbox event. `false` when nothing is due: the inbox is empty, or only
    /// events waiting for their retry (or behind one) remain.
    ///
    /// A failing event stays unprocessed and is retried with backoff, forever (ADR-0024
    /// amendment). Later events of the same task (else the same subject) wait behind it; other
    /// tasks' events keep flowing.
    pub async fn process_next(&self) -> Result<bool, Error> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT e.id, e.project, e.subject_key, e.task_id, e.type, e.payload, e.attempts
             FROM engine.events e
             WHERE e.processed_at IS NULL AND (e.retry_at IS NULL OR e.retry_at <= now())
               AND NOT EXISTS (
                   SELECT 1 FROM engine.events p
                   WHERE p.processed_at IS NULL AND p.id < e.id
                     AND ((e.task_id IS NOT NULL AND p.task_id = e.task_id)
                          OR (e.task_id IS NULL AND e.subject_key IS NOT NULL
                              AND p.task_id IS NULL AND p.subject_key = e.subject_key
                              AND p.project IS NOT DISTINCT FROM e.project)))
             ORDER BY e.id LIMIT 1 FOR UPDATE OF e SKIP LOCKED",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let id: i64 = row.get("id");
        let ty: String = row.get("type");
        let payload: Value = row.get("payload");
        let task_id: Option<i64> = row.get("task_id");
        let attempts: i32 = row.get("attempts");
        // A savepoint, so a failure undoes the handler's writes but keeps the event's row lock.
        sqlx::query("SAVEPOINT handler").execute(&mut *tx).await?;
        let note = if let Some(kind) = ty.strip_prefix("ui.") {
            self.on_ui_event(&mut tx, id, task_id, kind, payload).await
        } else if ty.starts_with(ENGINE_PREFIX) {
            self.on_engine_event(&mut tx, id, task_id, &ty, payload)
                .await
        } else {
            let project: Option<String> = row.get("project");
            let skey: Option<String> = row.get("subject_key");
            self.on_catalog_event(&mut tx, id, project, skey, task_id, payload)
                .await
        };
        let (note, task) = match note {
            Ok(x) => x,
            Err(e) => {
                tracing::error!(
                    event = id,
                    attempts = attempts + 1,
                    "processing failed: {e}"
                );
                sqlx::query("ROLLBACK TO SAVEPOINT handler")
                    .execute(&mut *tx)
                    .await?;
                let attempts = attempts.saturating_add(1);
                let err = e.to_string();
                let delay = retry_delay(attempts);
                sqlx::query(
                    "UPDATE engine.events SET attempts = $2, last_error = $3,
                            retry_at = now() + make_interval(secs => $4) WHERE id = $1",
                )
                .bind(id)
                .bind(attempts)
                .bind(&err)
                .bind(delay.as_secs_f64())
                .execute(&mut *tx)
                .await?;
                if let Some(task_id) = task_id {
                    let text = format!("Event {ty} failing (attempt {attempts}): {err}");
                    sqlx::query(
                        "UPDATE engine.projection SET reason = 'event_failing', text = $2,
                                updated_at = now() WHERE task_id = $1",
                    )
                    .bind(task_id)
                    .bind(text)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                return Ok(true);
            }
        };
        if attempts > 0
            && let Some(t) = task.or(task_id)
        {
            // A retried event succeeded: drop the failure overlay if nothing rewrote it.
            sqlx::query(
                "UPDATE engine.projection SET reason = NULL, text = NULL
                 WHERE task_id = $1 AND reason = 'event_failing'",
            )
            .bind(t)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE engine.events SET processed_at = now(), note = $2, retry_at = NULL,
                    task_id = COALESCE(task_id, $3) WHERE id = $1",
        )
        .bind(id)
        .bind(note)
        .bind(task)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn on_engine_event(
        &self,
        tx: &mut Tx<'_>,
        event_id: i64,
        task_id: Option<i64>,
        ty: &str,
        payload: Value,
    ) -> Result<(Option<String>, Option<i64>), Error> {
        let Some(task_id) = task_id else {
            return Ok((Some("engine event without task".into()), None));
        };
        let ev: TaskEvent = serde_json::from_value(payload.clone())
            .map_err(|e| Error::State(format!("{ty}: {e}")))?;
        if matches!(ev, TaskEvent::Resume | TaskEvent::Cancel) {
            // UI resume and cancel (ADR-0022 §9 generic actions): writers only.
            let what = if matches!(ev, TaskEvent::Cancel) {
                "cancel"
            } else {
                "resume"
            };
            let login = payload
                .pointer("/actor/login")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mut task = self.lock_task(tx, task_id).await?;
            if login.is_empty() || !self.can_write(&task.repo, login).await {
                let who = if login.is_empty() { "anonymous" } else { login };
                return Ok((Some(format!("{who} may not {what}")), Some(task_id)));
            }
            // Jobs that waited on a human (a missing secret) run again.
            sqlx::query(
                "UPDATE engine.jobs SET status = 'queued', run_after = now(), updated_at = now()
                 WHERE task_id = $1 AND status = 'blocked'",
            )
            .bind(task_id)
            .execute(&mut **tx)
            .await?;
            self.apply(tx, &mut task, Some(event_id), &ev).await?;
            return Ok((None, Some(task_id)));
        }
        let mut task = self.lock_task(tx, task_id).await?;
        if let TaskEvent::ActionFinished { result, .. } = &ev {
            self.link_from_result(tx, &mut task, result).await?;
        }
        self.apply(tx, &mut task, Some(event_id), &ev).await?;
        Ok((None, Some(task_id)))
    }

    /// `engine_api.submit`: `{source: ui, actor: {login}, kind, data}`. Commands, comments,
    /// stop and steer become a `comment` catalog event on the task (the workflow's waits and
    /// steer/stop matchers decide); `resume` and `cancel` are engine-native (writers only).
    async fn on_ui_event(
        &self,
        tx: &mut Tx<'_>,
        event_id: i64,
        task_id: Option<i64>,
        kind: &str,
        payload: Value,
    ) -> Result<(Option<String>, Option<i64>), Error> {
        let Some(task_id) = task_id else {
            return Ok((Some("ui event without task".into()), None));
        };
        let login = payload
            .pointer("/actor/login")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if kind == "resume" || kind == "cancel" {
            // Engine-native: resume re-resolves (in `apply`) and continues; cancel stops any
            // running step and ends the task `cancelled`.
            return self
                .on_engine_event(
                    tx,
                    event_id,
                    Some(task_id),
                    &format!("{ENGINE_PREFIX}{kind}"),
                    json!({"kind": kind, "actor": {"login": login}}),
                )
                .await;
        }
        let data = payload.get("data").cloned().unwrap_or(Value::Null);
        let words = |s: &str| s.to_string();
        let (body, command, args) = match kind {
            "command" => {
                let cmd = data
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim_start_matches('/')
                    .to_ascii_lowercase();
                if cmd.is_empty()
                    || !cmd
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Ok((Some("invalid command".into()), Some(task_id)));
                }
                let args: Vec<String> = data
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(words)).collect())
                    .unwrap_or_default();
                let body = std::iter::once(format!("/{cmd}"))
                    .chain(args.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" ");
                (body, Some(cmd), args)
            }
            "stop" => (format!("/{kind}"), Some(kind.to_string()), Vec::new()),
            "comment" | "steer" => {
                let body = data
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let parsed =
                    enzarb_flow::events::Comment::parsed(&body, Some(&self.cfg.bot_handle));
                (body, parsed.command, parsed.args)
            }
            other => return Ok((Some(format!("unknown ui kind {other}")), Some(task_id))),
        };
        let repo: String = sqlx::query_scalar("SELECT repo FROM engine.tasks WHERE id = $1")
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await?;
        let ev = json!({
            "type": "comment", "body": body, "command": command, "args": args,
            "id": format!("ui:{event_id}"), "repo": repo,
            "actor": {"login": login, "can_write": false, "bot": false},
            "at": chrono::Utc::now().to_rfc3339(),
        });
        self.on_catalog_event(tx, event_id, None, None, Some(task_id), ev)
            .await
    }

    async fn can_write(&self, repo: &str, login: &str) -> bool {
        let actor = CActor {
            login: login.to_string(),
            id: 0,
            is_bot: false,
        };
        match self.permissions.permission(&repo_key(repo), &actor).await {
            Ok(p) => p.can_write(),
            Err(e) => {
                tracing::warn!(
                    repo,
                    login,
                    "permission lookup failed, treating as no write access: {e}"
                );
                false
            }
        }
    }

    async fn on_catalog_event(
        &self,
        tx: &mut Tx<'_>,
        event_id: i64,
        project: Option<String>,
        skey: Option<String>,
        task_id: Option<i64>,
        payload: Value,
    ) -> Result<(Option<String>, Option<i64>), Error> {
        let mut ev: Event = match serde_json::from_value(payload) {
            Ok(e) => e,
            Err(e) => return Ok((Some(format!("not a catalog event: {e}")), None)),
        };
        // `mentioned` uses this instance's handle, whatever the sender computed.
        if let Payload::Comment(c) = &mut ev.payload {
            c.mentioned = enzarb_flow::events::mentions(&c.body, &self.cfg.bot_handle);
        }
        // can_write comes from GitHub, never from the payload.
        if let (Some(repo), Some(actor)) = (ev.repo.clone(), ev.actor.as_mut()) {
            actor.can_write = !actor.login.is_empty() && self.can_write(&repo, &actor.login).await;
        }
        // The instance's own comments are bot comments whatever the sender said, so they are
        // never answered (no reply loop).
        if let Some(actor) = ev.actor.as_mut()
            && actor.login.eq_ignore_ascii_case(&self.cfg.bot_login)
        {
            actor.bot = true;
        }
        let repo = ev.repo.clone().map(|r| r.to_ascii_lowercase());
        let info = match (&project, &repo) {
            (Some(p), _) => self.index.get(p),
            (None, Some(r)) => self.index.lookup(r),
            _ => None,
        };
        let Some(info) = info else {
            return Ok((Some("no project for this event".into()), None));
        };
        let skey = skey.or_else(|| {
            let (r, s) = (repo.as_ref()?, ev.subject.as_ref()?);
            Some(subject_key(r, s.number))
        });

        // 1. Routed explicitly (UI submit) or by a linked subject.
        let mut target = task_id;
        if target.is_none()
            && let Some(k) = &skey
        {
            target = sqlx::query_scalar(
                "SELECT task_id FROM engine.subjects WHERE key = $1 AND NOT ended",
            )
            .bind(k)
            .fetch_optional(&mut **tx)
            .await?;
        }
        // 2. A pull request that references issues joins their task.
        if target.is_none()
            && let (Payload::PullRequest(pr), Some(r), Some(k)) = (&ev.payload, &repo, &skey)
        {
            let mut refs = pr.references.clone();
            if let Ok(more) = self
                .connector()
                .pr_references(&WorkItemKey::new(repo_key(r), pr.number))
                .await
            {
                refs.extend(more);
            }
            refs.sort();
            refs.dedup();
            for n in refs {
                let t: Option<i64> = sqlx::query_scalar(
                    "SELECT task_id FROM engine.subjects WHERE key = $1 AND NOT ended",
                )
                .bind(subject_key(r, n))
                .fetch_optional(&mut **tx)
                .await?;
                if let Some(t) = t {
                    self.link_subject(tx, t, "pull_request", k, &pr_url(r, pr.number))
                        .await?;
                    target = Some(t);
                    break;
                }
            }
        }
        // 3. A push to the head branch of a live task's open pull request (also foreign pushes).
        if target.is_none()
            && let (Payload::Push(p), Some(r)) = (&ev.payload, &repo)
        {
            target = sqlx::query_scalar(
                "SELECT id FROM engine.tasks WHERE repo = $1 AND ended_at IS NULL
                   AND state->'doc'->'subjects'->'pull_requests' @> jsonb_build_array(jsonb_build_object('head', $2::text, 'state', 'open'))
                 ORDER BY id LIMIT 1",
            )
            .bind(r)
            .bind(&p.branch)
            .fetch_optional(&mut **tx)
            .await?;
        }
        // 4. Release-like events go to every live task whose merged commit the ref contains.
        if target.is_none()
            && ev.subject.is_none()
            && let Some(r) = &repo
            && let Some((git_ref, tag)) = released_ref(&ev.payload)
        {
            let ids = self.containing_tasks(tx, r, &git_ref).await?;
            let mut first = None;
            for id in ids {
                let mut task = self.lock_task(tx, id).await?;
                if let Some(tag) = &tag {
                    let _ = task.state.doc.set_fact("contained_in_tag", json!(tag));
                }
                self.apply(
                    tx,
                    &mut task,
                    Some(event_id),
                    &TaskEvent::Event { event: ev.clone() },
                )
                .await?;
                first.get_or_insert(id);
            }
            if first.is_some() {
                return Ok((None, first));
            }
        }
        if let Some(t) = target {
            let mut task = self.lock_task(tx, t).await?;
            if let Err(e) = validate_custom(&task.resolved, &ev) {
                return Ok((Some(e), Some(t)));
            }
            let old_head = open_pr_head_sha(&task);
            self.refresh_doc(&mut task.state.doc, &ev).await;
            self.refresh_on_head_change(tx, &task, event_id, &ev, old_head)
                .await?;
            self.apply(
                tx,
                &mut task,
                Some(event_id),
                &TaskEvent::Event { event: ev },
            )
            .await?;
            return Ok((None, Some(t)));
        }

        // 3. Bindings.
        let Some(repo) = repo else {
            return Ok((Some("unrouted: no repo".into()), None));
        };
        if ev
            .actor
            .as_ref()
            .is_some_and(|a| a.login.eq_ignore_ascii_case(&self.cfg.bot_login))
        {
            return Ok((Some("unrouted: own event".into()), None));
        }
        let bindings = match self.library.bindings(&info, &repo).await {
            Ok(b) => b,
            Err(e) => {
                self.record_resolution(&info, &repo, None, &[], Some(&e))
                    .await;
                return Ok((Some(format!("bindings: {e}")), None));
            }
        };
        let routing = enzarb_flow::document::RoutingContext {
            relations: self
                .routing_relations(tx, &ev, &repo, skey.as_deref())
                .await?,
            event: ev.clone(),
        };
        let (b, params) = match enzarb_flow::resolve::select_binding_in(&bindings, &routing) {
            Ok(Some(s)) => (s.binding.clone(), s.params),
            Ok(None) => return Ok((Some("unrouted: no binding matched".into()), None)),
            Err(e) => return Ok((Some(format!("bindings: {e}")), None)),
        };
        let params = (!params.is_empty()).then_some(params);
        let resolved = match self
            .library
            .resolve(&info, b.workflow.clone(), params)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                self.record_resolution(&info, &repo, None, &bindings, Some(&e))
                    .await;
                return Ok((Some(format!("workflow {}: {e}", b.workflow.path)), None));
            }
        };
        self.record_resolution(&info, &repo, Some(&resolved), &bindings, None)
            .await;
        if let Err(e) = validate_custom(&resolved, &ev) {
            return Ok((Some(e), None));
        }
        let t = self
            .create_task(
                tx,
                &info,
                &repo,
                b.id.as_deref(),
                &b.workflow,
                resolved,
                &ev,
                skey,
            )
            .await?;
        Ok((None, Some(t)))
    }

    /// Tasks the event's object relates to, for a binding's `if`/`with`. `kind` is the related
    /// task's role towards the object, as in its relation row (T, kind, object): an epic's
    /// `parent` rows make it the `parent` of each sub-issue. Tasks of issues a pull request
    /// references come as `references`.
    async fn routing_relations(
        &self,
        tx: &mut Tx<'_>,
        ev: &Event,
        repo: &str,
        skey: Option<&str>,
    ) -> Result<Vec<enzarb_flow::document::RoutingRelation>, Error> {
        let mut rows: Vec<(String, i64)> = Vec::new();
        if let Some(k) = skey {
            let r: Vec<(String, i64)> =
                sqlx::query_as("SELECT kind, task_id FROM engine.relations WHERE target_key = $1")
                    .bind(k)
                    .fetch_all(&mut **tx)
                    .await?;
            rows.extend(r);
        }
        if let Payload::PullRequest(pr) = &ev.payload {
            for n in &pr.references {
                let t: Option<i64> = sqlx::query_scalar(
                    "SELECT task_id FROM engine.subjects WHERE key = $1 ORDER BY ended, linked_at DESC LIMIT 1",
                )
                .bind(subject_key(repo, *n))
                .fetch_optional(&mut **tx)
                .await?;
                if let Some(t) = t {
                    rows.push(("references".into(), t));
                }
            }
        }
        let mut out = Vec::new();
        for (kind, id) in rows {
            let r: Option<(String, Value)> =
                sqlx::query_as("SELECT workflow_ref, state FROM engine.tasks WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut **tx)
                    .await?;
            let Some((workflow, state)) = r else { continue };
            let Ok(st) = serde_json::from_value::<TaskState>(state) else {
                continue;
            };
            let status = match &st.status {
                Status::Ended { status } => status.clone(),
                s => s.label().to_string(),
            };
            let phase = st
                .frames
                .iter()
                .map(|f| f.id.clone())
                .collect::<Vec<_>>()
                .join("/");
            out.push(enzarb_flow::document::RoutingRelation {
                kind,
                task: id.to_string(),
                workflow,
                status,
                phase,
                node: st.frames.last().map(|f| f.node.clone()).unwrap_or_default(),
                vars: st.doc.vars.clone(),
                subject: None,
            });
        }
        Ok(out)
    }

    /// Live tasks of `repo` with a merged pull request whose merge commit `git_ref` contains
    /// (connector compare: the merge commit is not ahead of the ref).
    async fn containing_tasks(
        &self,
        tx: &mut Tx<'_>,
        repo: &str,
        git_ref: &str,
    ) -> Result<Vec<i64>, Error> {
        let rows: Vec<(i64, Value)> = sqlx::query_as(
            "SELECT id, state->'doc'->'subjects'->'pull_requests' FROM engine.tasks
             WHERE repo = $1 AND ended_at IS NULL ORDER BY id",
        )
        .bind(repo)
        .fetch_all(&mut **tx)
        .await?;
        let mut out = Vec::new();
        for (id, prs) in rows {
            let prs: Vec<Subject> = serde_json::from_value(prs).unwrap_or_default();
            for p in prs.iter().filter(|p| p.state == "merged") {
                let Some(sha) = &p.merge_sha else { continue };
                if let Ok(Some(0)) = self
                    .connector()
                    .ahead_by(&repo_key(repo), git_ref, sha)
                    .await
                {
                    out.push(id);
                    break;
                }
            }
        }
        Ok(out)
    }

    /// On a verified change of the open pull request's head (any pusher), the head-bound facts
    /// (`ci`, `mergeable`, `reviews`) are refreshed.
    async fn refresh_on_head_change(
        &self,
        tx: &mut Tx<'_>,
        task: &TaskRow,
        event_id: i64,
        ev: &Event,
        old_head: Option<String>,
    ) -> Result<(), Error> {
        let claimed = match &ev.payload {
            Payload::PullRequest(p) if !p.head_sha.is_empty() => p.head_sha.clone(),
            Payload::Push(p) => p.after.clone(),
            _ => return Ok(()),
        };
        let Some(n) = task
            .state
            .doc
            .subjects
            .pull_requests
            .iter()
            .rev()
            .find(|p| p.state == "open")
            .map(|p| (p.repo.clone(), p.number))
        else {
            return Ok(());
        };
        let known = task.state.doc.facts.ci.as_ref().map(|c| c.sha.clone());
        if old_head.as_deref() == Some(claimed.as_str()) && known.as_deref() == Some(&claimed) {
            return Ok(());
        }
        // Verify on GitHub; never trust the payload alone.
        let item = WorkItemKey::new(repo_key(&n.0), n.1);
        let verified = match self.connector().get_merge_readiness(&item, n.1).await {
            Ok(r) => r.head_sha,
            Err(e) => {
                tracing::debug!(task = task.id, "head verification: {e}");
                return Ok(());
            }
        };
        if verified != claimed || known.as_deref() == Some(verified.as_str()) {
            return Ok(());
        }
        for fact in ["ci", "mergeable", "reviews"] {
            self.queue_effect(
                tx,
                task,
                Some(event_id),
                &Effect::RefreshFact { fact: fact.into() },
            )
            .await?;
        }
        Ok(())
    }

    /// `instance` of task documents, from config.
    fn instance(&self) -> enzarb_flow::document::Instance {
        enzarb_flow::document::Instance {
            bot: enzarb_flow::document::Bot {
                login: self.cfg.bot_login.clone(),
                handle: self.cfg.bot_handle.clone(),
            },
            url: self.cfg.public_url.clone(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_task(
        &self,
        tx: &mut Tx<'_>,
        info: &ProjectInfo,
        repo: &str,
        binding: Option<&str>,
        entry: &Entry,
        resolved: Resolved,
        ev: &Event,
        skey: Option<String>,
    ) -> Result<i64, Error> {
        let wref = format!("{}@{}:{}", entry.repo, entry.rref, entry.path);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO engine.tasks (project, repo, binding, workflow_ref, state, resolved, origins, started_by)
             VALUES ($1, $2, $3, $4, '{}', '{}', $5, $6) RETURNING id",
        )
        .bind(&info.name)
        .bind(repo)
        .bind(binding)
        .bind(&wref)
        .bind(serde_json::to_value(&resolved.origins).unwrap_or_default())
        .bind(ev.actor.as_ref().map(|a| a.login.clone()))
        .fetch_one(&mut **tx)
        .await?;
        let mut doc = TaskDocument {
            task: TaskInfo {
                id: id.to_string(),
                project: info.name.clone(),
                repo: repo.to_string(),
                workflow: wref,
                default_branch: info.default_branch(repo).unwrap_or("main").to_string(),
            },
            ..Default::default()
        };
        doc.history = self.history(tx, &info.name).await?;
        self.refresh_doc(&mut doc, ev).await;
        if let (Some(k), Some(s)) = (&skey, &ev.subject) {
            let kind = match s.kind {
                SubjectKind::Issue => "issue",
                SubjectKind::PullRequest => "pull_request",
            };
            let url = match s.kind {
                SubjectKind::Issue => issue_url(repo, s.number),
                SubjectKind::PullRequest => pr_url(repo, s.number),
            };
            self.link_subject(tx, id, kind, k, &url).await?;
        }
        let mut state = TaskState::new(doc);
        // Known from the start (possibly `[]`), so a workflow can tell "none" from "unknown".
        if state.doc.subjects.issue.is_some() {
            match crate::actions::fact(self, id, repo, &state, "dependencies").await {
                Ok(v) => {
                    let _ = state.doc.set_fact("dependencies", v);
                }
                Err(e) => tracing::debug!(task = id, "dependencies at start: {e}"),
            }
        }
        let mut task = TaskRow {
            id,
            project: info.name.clone(),
            repo: repo.to_string(),
            state,
            resolved,
        };
        self.apply(tx, &mut task, None, &TaskEvent::Start { event: ev.clone() })
            .await?;
        Ok(id)
    }

    async fn history(
        &self,
        tx: &mut Tx<'_>,
        project: &str,
    ) -> Result<Vec<enzarb_flow::document::HistoryEntry>, Error> {
        let rows = sqlx::query(
            "SELECT t.id, t.workflow_ref, p.status, p.end_status, p.title, t.ended_at
             FROM engine.tasks t JOIN engine.projection p ON p.task_id = t.id
             WHERE t.project = $1 ORDER BY t.updated_at DESC LIMIT 20",
        )
        .bind(project)
        .fetch_all(&mut **tx)
        .await?;
        Ok(rows
            .iter()
            .map(|r| enzarb_flow::document::HistoryEntry {
                task: r.get::<i64, _>("id").to_string(),
                workflow: r.get("workflow_ref"),
                status: r
                    .get::<Option<String>, _>("end_status")
                    .unwrap_or_else(|| r.get("status")),
                title: r.get("title"),
                ended_at: r
                    .get::<Option<chrono::DateTime<chrono::Utc>>, _>("ended_at")
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// Writes what was last resolved for a Project/repo (board and the read-only viewer).
    async fn record_resolution(
        &self,
        info: &ProjectInfo,
        repo: &str,
        resolved: Option<&Resolved>,
        bindings: &[enzarb_flow::resolve::RBinding],
        error: Option<&str>,
    ) {
        let wf = resolved.map(viewer_workflow);
        let board = resolved.map(|r| serde_json::to_value(&r.board).unwrap_or_default());
        let res = sqlx::query(
            "INSERT INTO engine.resolutions (project, repo, board, view, updated_at)
             VALUES ($1, $2, COALESCE($3, '{\"columns\": []}'),
                     jsonb_build_object('bindings', $4::jsonb,
                                        'workflows', CASE WHEN $5::jsonb IS NULL THEN '[]'::jsonb ELSE jsonb_build_array($5::jsonb) END,
                                        'lint', $6::jsonb), now())
             ON CONFLICT (project, repo) DO UPDATE SET
                board = COALESCE($3, engine.resolutions.board),
                view = jsonb_build_object(
                    'bindings', $4::jsonb,
                    'workflows', COALESCE((
                        SELECT jsonb_agg(w) FROM jsonb_array_elements(COALESCE(engine.resolutions.view->'workflows', '[]')) w
                        WHERE $5::jsonb IS NULL OR w->>'ref' <> $5::jsonb->>'ref'), '[]')
                        || CASE WHEN $5::jsonb IS NULL THEN '[]'::jsonb ELSE jsonb_build_array($5::jsonb) END,
                    'lint', $6::jsonb),
                updated_at = now()",
        )
        .bind(&info.name)
        .bind(repo)
        .bind(board)
        .bind(serde_json::to_value(bindings).unwrap_or_default())
        .bind(wf)
        .bind(json!(error.map(|e| vec![e]).unwrap_or_default()))
        .execute(&self.pool)
        .await;
        if let Err(e) = res {
            tracing::warn!("recording resolution: {e}");
        }
    }

    pub(crate) async fn lock_task(&self, tx: &mut Tx<'_>, id: i64) -> Result<TaskRow, Error> {
        let r = sqlx::query(
            "SELECT id, project, repo, state, resolved FROM engine.tasks WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
        Ok(TaskRow {
            id,
            project: r.get("project"),
            repo: r.get("repo"),
            state: serde_json::from_value(r.get("state"))
                .map_err(|e| Error::State(format!("task {id} state: {e}")))?,
            resolved: serde_json::from_value(r.get("resolved"))
                .map_err(|e| Error::State(format!("task {id} definition: {e}")))?,
        })
    }

    pub(crate) async fn link_subject(
        &self,
        tx: &mut Tx<'_>,
        task: i64,
        kind: &str,
        key: &str,
        url: &str,
    ) -> Result<bool, Error> {
        let n = sqlx::query(
            "INSERT INTO engine.subjects (task_id, kind, key, url) VALUES ($1, $2, $3, $4)
             ON CONFLICT DO NOTHING",
        )
        .bind(task)
        .bind(kind)
        .bind(key)
        .bind(url)
        .execute(&mut **tx)
        .await?;
        Ok(n.rows_affected() == 1)
    }

    /// An action result may name a subject to link (`{"subject": {"kind", "repo", "number"}}`,
    /// e.g. a pull request the action opened).
    async fn link_from_result(
        &self,
        tx: &mut Tx<'_>,
        task: &mut TaskRow,
        result: &Value,
    ) -> Result<(), Error> {
        let Some(s) = result.get("subject") else {
            return Ok(());
        };
        let (Some(kind), Some(repo), Some(n)) = (
            s.get("kind").and_then(Value::as_str),
            s.get("repo").and_then(Value::as_str),
            s.get("number").and_then(Value::as_u64),
        ) else {
            return Ok(());
        };
        let url = s
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        self.link_subject(tx, task.id, kind, &subject_key(repo, n), &url)
            .await?;
        if kind == "pull_request" {
            let item = WorkItemKey::new(repo_key(repo), n);
            let subj = match self.connector().fetch_thread(&item).await {
                Ok(t) => thread_subject(repo, &self.cfg.bot_handle, &t, "open"),
                Err(_) => Subject {
                    repo: repo.into(),
                    number: n,
                    url,
                    state: "open".into(),
                    ..Default::default()
                },
            };
            // Created for this task: what was said before is not feedback.
            let subj = Subject {
                linked_at: Some(chrono::Utc::now().to_rfc3339()),
                ..subj
            };
            let prs = &mut task.state.doc.subjects.pull_requests;
            prs.retain(|p| !(p.repo == subj.repo && p.number == subj.number));
            prs.push(subj);
        }
        Ok(())
    }

    /// Keeps the task document's subjects current from GitHub (verified data, not agent output).
    async fn refresh_doc(&self, doc: &mut TaskDocument, ev: &Event) {
        let (Some(repo), Some(s)) = (ev.repo.as_ref(), ev.subject.as_ref()) else {
            return;
        };
        let repo = repo.to_ascii_lowercase();
        let item = WorkItemKey::new(repo_key(&repo), s.number);
        let state = match &ev.payload {
            Payload::Issue(i) if i.action == "closed" => "closed",
            Payload::PullRequest(p) if p.merged => "merged",
            Payload::PullRequest(p) if p.action == "closed" => "closed",
            _ => "open",
        };
        let fetched = self.connector().fetch_thread(&item).await.ok();
        let mut subj = match &fetched {
            Some(t) => thread_subject(&repo, &self.cfg.bot_handle, t, state),
            None => Subject {
                repo: repo.clone(),
                number: s.number,
                state: state.into(),
                ..Default::default()
            },
        };
        match &ev.payload {
            Payload::Issue(i) => {
                if subj.title.is_empty() {
                    subj.title = i.title.clone();
                    subj.body = i.body.clone();
                }
                subj.labels = i.labels.clone();
                if subj.url.is_empty() {
                    subj.url = issue_url(&repo, s.number);
                }
            }
            Payload::PullRequest(p) => {
                if subj.title.is_empty() {
                    subj.title = p.title.clone();
                    subj.body = p.body.clone();
                }
                subj.head = Some(p.head.clone()).filter(|h| !h.is_empty()).or(subj.head);
                subj.merge_sha =
                    Some(p.merge_sha.clone())
                        .filter(|m| !m.is_empty())
                        .or_else(|| {
                            doc.subjects
                                .pull_requests
                                .iter()
                                .find(|x| x.repo == repo && x.number == s.number)
                                .and_then(|x| x.merge_sha.clone())
                        });
                subj.base = Some(p.base.clone()).filter(|b| !b.is_empty()).or(subj.base);
                if subj.url.is_empty() {
                    subj.url = pr_url(&repo, s.number);
                }
            }
            _ => {}
        }
        // Set once, when the engine linked the object; a refresh keeps it.
        subj.linked_at = match s.kind {
            SubjectKind::Issue => doc.subjects.issue.as_ref(),
            SubjectKind::PullRequest => doc
                .subjects
                .pull_requests
                .iter()
                .find(|x| x.repo == repo && x.number == s.number),
        }
        .filter(|x| x.repo == repo && x.number == s.number)
        .and_then(|x| x.linked_at.clone());
        match s.kind {
            SubjectKind::Issue => {
                if doc
                    .subjects
                    .issue
                    .as_ref()
                    .is_none_or(|i| i.number == s.number && i.repo == repo)
                {
                    doc.subjects.issue = Some(subj);
                }
            }
            SubjectKind::PullRequest => {
                let prs = &mut doc.subjects.pull_requests;
                prs.retain(|p| !(p.repo == repo && p.number == s.number));
                prs.push(subj);
            }
        }
    }

    /// Re-resolves at a wait (auto-upgrade, ADR-0022 §4), steps the interpreter and persists
    /// state, transition, effects and projection.
    pub(crate) async fn apply(
        &self,
        tx: &mut Tx<'_>,
        task: &mut TaskRow,
        event_id: Option<i64>,
        ev: &TaskEvent,
    ) -> Result<(), Error> {
        task.state.doc.instance = self.instance();
        // Auto-upgrade (ADR-0022 §4) at each wait and before each step: with nothing in flight,
        // or when this event finishes what was in flight (the next node, often a step, then
        // comes from the new definition). A running step keeps the definition it started with.
        let finishing = matches!(
            ev,
            TaskEvent::StepFinished(_) | TaskEvent::ActionFinished { .. }
        );
        if !matches!(task.state.status, Status::Ended { .. } | Status::New)
            && (task.state.pending.is_none() || finishing)
        {
            self.reresolve(task).await;
        }
        let from = summary(&task.state);
        let (next, effects) = interp::step(&task.resolved, &task.state, ev);
        task.state = next;
        self.persist(tx, task, event_id, from, &effects).await
    }

    async fn reresolve(&self, task: &mut TaskRow) {
        let Some(info) = self.index.get(&task.project) else {
            return;
        };
        let e = &task.resolved.entry;
        let entry = Entry {
            repo: e.repo.clone(),
            rref: e.rref.clone(),
            path: e.path.clone(),
        };
        let params = (!task.resolved.params.is_empty()).then(|| task.resolved.params.clone());
        match self.library.resolve(&info, entry, params).await {
            Ok(new) if new != task.resolved => {
                // The finishing step or action no longer holds the definition.
                let pending = task.state.pending.take();
                let (mut s, outcome) = interp::reresolve(&new, &task.state);
                s.pending = pending;
                task.state = s;
                if outcome == interp::Reresolution::Upgraded {
                    task.resolved = new;
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                task = task.id,
                "re-resolution failed, keeping definition: {e}"
            ),
        }
    }

    async fn persist(
        &self,
        tx: &mut Tx<'_>,
        task: &TaskRow,
        event_id: Option<i64>,
        from: Value,
        effects: &[Effect],
    ) -> Result<(), Error> {
        let ended = matches!(task.state.status, Status::Ended { .. });
        sqlx::query(
            "UPDATE engine.tasks SET state = $2, resolved = $3, origins = $4, updated_at = now(),
                    ended_at = CASE WHEN $5 THEN COALESCE(ended_at, now()) END
             WHERE id = $1",
        )
        .bind(task.id)
        .bind(serde_json::to_value(&task.state).map_err(|e| Error::State(e.to_string()))?)
        .bind(serde_json::to_value(&task.resolved).map_err(|e| Error::State(e.to_string()))?)
        .bind(serde_json::to_value(&task.resolved.origins).unwrap_or_default())
        .bind(ended)
        .execute(&mut **tx)
        .await?;
        sqlx::query("UPDATE engine.subjects SET ended = $2 WHERE task_id = $1")
            .bind(task.id)
            .bind(ended)
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT INTO engine.transitions (task_id, event_id, from_state, to_state, effects)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(task.id)
        .bind(event_id)
        .bind(from)
        .bind(summary(&task.state))
        .bind(serde_json::to_value(effects).unwrap_or_default())
        .execute(&mut **tx)
        .await?;
        let mut projected = false;
        for fx in effects {
            if matches!(fx, Effect::UpdateProjection(_)) {
                projected = true;
            }
            self.queue_effect(tx, task, event_id, fx).await?;
        }
        if !projected {
            // Keep the projection's subjects, title and cost current.
            sqlx::query("UPDATE engine.projection SET updated_at = now() WHERE task_id = $1")
                .bind(task.id)
                .execute(&mut **tx)
                .await?;
        }
        if ended {
            self.notify_related(tx, task).await?;
        }
        Ok(())
    }

    /// When a task ends: `child_ended`/`all_children_ended` to its parent and
    /// `dependency_changed` to tasks blocked by it.
    async fn notify_related(&self, tx: &mut Tx<'_>, task: &TaskRow) -> Result<(), Error> {
        let status = match &task.state.status {
            Status::Ended { status } => status.clone(),
            _ => return Ok(()),
        };
        let keys: Vec<String> =
            sqlx::query_scalar("SELECT key FROM engine.subjects WHERE task_id = $1")
                .bind(task.id)
                .fetch_all(&mut **tx)
                .await?;
        let mut targets = keys.clone();
        targets.push(format!("task:{}", task.id));
        // Tasks blocked by this one.
        let blocked: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT r.task_id FROM engine.relations r JOIN engine.tasks t ON t.id = r.task_id
             WHERE r.kind = 'blocked_by' AND r.target_key = ANY($1) AND t.ended_at IS NULL",
        )
        .bind(&targets)
        .fetch_all(&mut **tx)
        .await?;
        for b in blocked {
            let ev = json!({"type": "dependency_changed", "task": task.id.to_string(), "status": status});
            self.insert_event(
                tx,
                &format!("dep:{}:{}", task.id, b),
                &task.project,
                b,
                "dependency_changed",
                ev,
            )
            .await?;
        }
        // Parents. A relation row (T, kind, X) reads "T is <kind> of X": the epic's
        // (E, parent, child) and a child's own (C, child, E) both make E the parent of C.
        let mut parents: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT task_id FROM engine.relations WHERE kind = 'parent' AND target_key = ANY($1)",
        )
        .bind(&targets)
        .fetch_all(&mut **tx)
        .await?;
        let up: Vec<String> = sqlx::query_scalar(
            "SELECT target_key FROM engine.relations WHERE task_id = $1 AND kind = 'child'",
        )
        .bind(task.id)
        .fetch_all(&mut **tx)
        .await?;
        for k in up {
            if let Some(id) = self.task_of_key(tx, &k).await? {
                parents.push(id);
            }
        }
        parents.sort();
        parents.dedup();
        parents.retain(|p| *p != task.id);
        for parent in parents {
            let ev = json!({"type": "child_ended", "task": task.id.to_string(), "status": status});
            self.insert_event(
                tx,
                &format!("child:{}:{}", task.id, parent),
                &task.project,
                parent,
                "child_ended",
                ev,
            )
            .await?;
            // Every child of the parent: (parent, parent, X) targets and (C, child, parent) rows.
            let mut children: Vec<Option<i64>> = Vec::new();
            let down: Vec<String> = sqlx::query_scalar(
                "SELECT target_key FROM engine.relations WHERE task_id = $1 AND kind = 'parent'",
            )
            .bind(parent)
            .fetch_all(&mut **tx)
            .await?;
            for k in down {
                children.push(self.task_of_key(tx, &k).await?);
            }
            let mut pkeys: Vec<String> =
                sqlx::query_scalar("SELECT key FROM engine.subjects WHERE task_id = $1")
                    .bind(parent)
                    .fetch_all(&mut **tx)
                    .await?;
            pkeys.push(format!("task:{parent}"));
            let more: Vec<i64> = sqlx::query_scalar(
                "SELECT DISTINCT task_id FROM engine.relations WHERE kind = 'child' AND target_key = ANY($1)",
            )
            .bind(&pkeys)
            .fetch_all(&mut **tx)
            .await?;
            children.extend(more.into_iter().map(Some));
            let mut statuses = serde_json::Map::new();
            let mut all = true;
            for c in children {
                let Some(c) = c else {
                    all = false; // a child issue without a task has not run yet
                    continue;
                };
                if c == task.id {
                    statuses.insert(c.to_string(), Value::String(status.clone()));
                    continue;
                }
                let row: Option<(bool, Value)> = sqlx::query_as(
                    "SELECT ended_at IS NOT NULL, state->'status' FROM engine.tasks WHERE id = $1",
                )
                .bind(c)
                .fetch_optional(&mut **tx)
                .await?;
                match row {
                    Some((true, st)) => {
                        let s = st
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        statuses.insert(c.to_string(), Value::String(s));
                    }
                    _ => all = false,
                }
            }
            if all && !statuses.is_empty() {
                let ev = json!({"type": "all_children_ended", "statuses": statuses});
                self.insert_event(
                    tx,
                    &format!("children:{parent}"),
                    &task.project,
                    parent,
                    "all_children_ended",
                    ev,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// The task of a relation target (`task:<id>`, or the latest task of a subject key).
    async fn task_of_key(&self, tx: &mut Tx<'_>, key: &str) -> Result<Option<i64>, Error> {
        Ok(match key.strip_prefix("task:") {
            Some(id) => id.parse().ok(),
            None => sqlx::query_scalar(
                "SELECT task_id FROM engine.subjects WHERE key = $1 ORDER BY ended, linked_at DESC LIMIT 1",
            )
            .bind(key)
            .fetch_optional(&mut **tx)
            .await?,
        })
    }

    pub(crate) async fn insert_event(
        &self,
        tx: &mut Tx<'_>,
        dedupe: &str,
        project: &str,
        task_id: i64,
        ty: &str,
        payload: Value,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO engine.events (dedupe_key, project, task_id, type, payload)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (dedupe_key) DO NOTHING",
        )
        .bind(dedupe)
        .bind(project)
        .bind(task_id)
        .bind(ty)
        .bind(payload)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

/// `{status, position}` of a state, for the transition log.
fn summary(s: &TaskState) -> Value {
    json!({"status": s.status.label(), "node": s.position(), "pending": s.pending})
}

/// Custom events must match the schema their workflow declares.
fn validate_custom(res: &Resolved, ev: &Event) -> Result<(), String> {
    let Payload::Custom(c) = &ev.payload else {
        return Ok(());
    };
    let Some(decl) = res.events.get(&c.name) else {
        return Err(format!("custom event `{}` is not declared", c.name));
    };
    crate::schema::validate(&decl.schema, &c.payload)
        .map_err(|e| format!("custom event `{}`: {e}", c.name))
}

pub(crate) fn issue_url(repo: &str, n: u64) -> String {
    format!("https://github.com/{repo}/issues/{n}")
}

pub(crate) fn pr_url(repo: &str, n: u64) -> String {
    format!("https://github.com/{repo}/pull/{n}")
}

/// The ref a release-like event names, and the tag (for `contained_in_tag`) when it is one.
fn released_ref(p: &Payload) -> Option<(String, Option<String>)> {
    match p {
        Payload::Release(r) if !r.tag.is_empty() => Some((r.tag.clone(), Some(r.tag.clone()))),
        Payload::Tag(t) if t.action != "deleted" && !t.name.is_empty() => {
            Some((t.name.clone(), Some(t.name.clone())))
        }
        Payload::Deployment(d) if !d.sha.is_empty() => Some((d.sha.clone(), None)),
        Payload::Package(p) if !p.version.is_empty() => {
            // Packages are commonly built from tag `v<version>`; `<version>` is tried by GitHub
            // compare only through that tag name.
            let tag = if p.version.starts_with('v') {
                p.version.clone()
            } else {
                format!("v{}", p.version)
            };
            Some((tag, None))
        }
        _ => None,
    }
}

fn open_pr_head_sha(task: &TaskRow) -> Option<String> {
    task.state.doc.facts.git.as_ref().map(|g| g.head.clone())
}

pub(crate) fn thread_subject(
    repo: &str,
    bot_handle: &str,
    t: &enzarb_connector::Thread,
    state: &str,
) -> Subject {
    Subject {
        repo: repo.to_string(),
        number: t.item.number,
        title: t.item.title.clone(),
        body: t.item.body.clone(),
        url: t.item.url.clone(),
        state: state.to_string(),
        labels: Vec::new(),
        author: t.item.author.clone(),
        head: t.item.head_ref.clone(),
        base: t.item.base_ref.clone(),
        merge_sha: None,
        linked_at: None,
        comments: t
            .comments
            .iter()
            .map(|c| SubjectComment {
                author: c.author.login.clone(),
                body: c.comment.body.clone(),
                at: c.created_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                command: enzarb_flow::events::parse_command(&c.comment.body, Some(bot_handle))
                    .map(|(command, _)| command),
                bot: c.author.is_bot,
            })
            .collect(),
    }
}

/// The viewer's shape of a resolved workflow: `{ref, origin, phases: [{id, use, origin, params,
/// with, exits, routes, graph: {nodes, edges}}]}`.
fn viewer_workflow(r: &Resolved) -> Value {
    use enzarb_flow::resolve::RKind;
    let phases: Vec<Value> = r
        .phases
        .iter()
        .map(|p| {
            let nodes: Vec<Value> = p
                .nodes
                .iter()
                .map(|(id, n)| {
                    let (kind, detail) = match &n.kind {
                        RKind::Step(s) => ("step", json!({"use": s.name})),
                        RKind::Wait(w) => ("wait", json!({"reason": w.reason, "column": w.column})),
                        RKind::Action(a) => ("action", serde_json::to_value(a).unwrap_or_default()),
                        RKind::End(s) => ("end", json!({"status": s})),
                        RKind::Phase(ph) => ("phase", json!({"use": ph.origin.path})),
                        RKind::Decide => ("decide", json!({"set": n.set})),
                    };
                    json!({"id": id, "kind": kind, "detail": detail, "start": *id == p.start})
                })
                .collect();
            let edges: Vec<Value> = p
                .nodes
                .iter()
                .flat_map(|(id, n)| {
                    n.next
                        .iter()
                        .map(move |e| json!({"from": id, "to": e.to, "if": e.cond}))
                })
                .collect();
            json!({
                "id": p.id, "use": p.origin.path, "origin": p.origin, "params": p.params,
                "with": p.params, "exits": p.exits, "routes": p.routes,
                "description": p.description, "graph": {"nodes": nodes, "edges": edges},
            })
        })
        .collect();
    json!({
        "ref": format!("{}@{}:{}", r.entry.repo, r.entry.rref, r.entry.path),
        "origin": r.entry,
        "description": r.description,
        "phases": phases,
        "origins": r.origins,
    })
}

#[cfg(test)]
mod retry_tests {
    use super::retry_delay;
    use std::time::Duration;

    #[test]
    fn backoff_doubles_to_a_plateau_without_overflow() {
        let secs = |n| retry_delay(n).as_secs();
        assert_eq!([secs(1), secs(2), secs(3), secs(4)], [2, 4, 8, 16]);
        assert_eq!(secs(8), 256);
        assert_eq!(secs(9), 300);
        assert_eq!(retry_delay(i32::MAX), Duration::from_secs(300));
        assert_eq!(retry_delay(0), Duration::from_secs(2));
        assert_eq!(retry_delay(-5), Duration::from_secs(2));
    }
}
