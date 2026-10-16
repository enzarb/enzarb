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

    /// Processes one inbox event. `false` when the inbox is empty.
    pub async fn process_next(&self) -> Result<bool, Error> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, project, subject_key, task_id, type, payload FROM engine.events
             WHERE processed_at IS NULL ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED",
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
                // Never wedge the inbox: record and move on (the task, if any, is untouched).
                tracing::error!(event = id, "processing failed: {e}");
                tx.rollback().await?;
                tx = self.pool.begin().await?;
                (Some(format!("error: {e}")), None)
            }
        };
        sqlx::query(
            "UPDATE engine.events SET processed_at = now(), note = $2,
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
        if matches!(ev, TaskEvent::Resume) {
            // A UI resume: writers only.
            let login = payload
                .pointer("/actor/login")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mut task = self.lock_task(tx, task_id).await?;
            if !login.is_empty() && !self.can_write(&task.repo, login).await {
                return Ok((Some(format!("{login} may not resume")), Some(task_id)));
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
    /// stop, cancel and steer become a `comment` catalog event on the task (the workflow's waits
    /// and steer/stop matchers decide); `resume` resumes a paused task (writers only).
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
        if kind == "resume" {
            return self
                .on_engine_event(
                    tx,
                    event_id,
                    Some(task_id),
                    "engine:resume",
                    json!({"kind": "resume", "actor": {"login": login}}),
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
            "stop" | "cancel" => (format!("/{kind}"), Some(kind.to_string()), Vec::new()),
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
        self.permissions
            .permission(&repo_key(repo), &actor)
            .await
            .map(|p| p.can_write())
            .unwrap_or(false)
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
        // can_write comes from GitHub, never from the payload.
        if let (Some(repo), Some(actor)) = (ev.repo.clone(), ev.actor.as_mut()) {
            actor.can_write = !actor.login.is_empty() && self.can_write(&repo, &actor.login).await;
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
        if let Some(t) = target {
            let mut task = self.lock_task(tx, t).await?;
            if let Err(e) = validate_custom(&task.resolved, &ev) {
                return Ok((Some(e), Some(t)));
            }
            self.refresh_doc(&mut task.state.doc, &ev).await;
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
        let b = match enzarb_flow::resolve::select_binding(&bindings, &ev) {
            Ok(Some(b)) => b.clone(),
            Ok(None) => return Ok((Some("unrouted: no binding matched".into()), None)),
            Err(e) => return Ok((Some(format!("bindings: {e}")), None)),
        };
        let resolved = match self.library.resolve(&info, b.workflow.clone()).await {
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
        let mut task = TaskRow {
            id,
            project: info.name.clone(),
            repo: repo.to_string(),
            state: TaskState::new(doc),
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
                Ok(t) => thread_subject(repo, &t, "open"),
                Err(_) => Subject {
                    repo: repo.into(),
                    number: n,
                    url,
                    state: "open".into(),
                    ..Default::default()
                },
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
            Some(t) => thread_subject(&repo, t, state),
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
                subj.base = Some(p.base.clone()).filter(|b| !b.is_empty()).or(subj.base);
                if subj.url.is_empty() {
                    subj.url = pr_url(&repo, s.number);
                }
            }
            _ => {}
        }
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
        if task.state.status == Status::Waiting && task.state.pending.is_none() {
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
        match self.library.resolve(&info, entry).await {
            Ok(new) if new != task.resolved => {
                let (s, outcome) = interp::reresolve(&new, &task.state);
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
        // The parent.
        let parents: Vec<String> = sqlx::query_scalar(
            "SELECT target_key FROM engine.relations WHERE task_id = $1 AND kind = 'parent'",
        )
        .bind(task.id)
        .fetch_all(&mut **tx)
        .await?;
        for pkey in parents {
            let parent: Option<i64> = match pkey.strip_prefix("task:") {
                Some(id) => id.parse().ok(),
                None => sqlx::query_scalar(
                    "SELECT task_id FROM engine.subjects WHERE key = $1 ORDER BY ended, linked_at DESC LIMIT 1",
                )
                .bind(&pkey)
                .fetch_optional(&mut **tx)
                .await?,
            };
            let Some(parent) = parent else { continue };
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
            let rows = sqlx::query(
                "SELECT t.id, t.ended_at IS NOT NULL AS ended, t.state->'status' AS st
                 FROM engine.relations r JOIN engine.tasks t ON t.id = r.task_id
                 WHERE r.kind = 'parent' AND r.target_key = $1",
            )
            .bind(&pkey)
            .fetch_all(&mut **tx)
            .await?;
            if rows
                .iter()
                .all(|r| r.get::<bool, _>("ended") || r.get::<i64, _>("id") == task.id)
            {
                let statuses: serde_json::Map<String, Value> = rows
                    .iter()
                    .map(|r| {
                        let id: i64 = r.get("id");
                        let st: Value = r.get("st");
                        let s = if id == task.id {
                            status.clone()
                        } else {
                            st.get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string()
                        };
                        (id.to_string(), Value::String(s))
                    })
                    .collect();
                let ev = json!({"type": "all_children_ended", "statuses": statuses});
                self.insert_event(
                    tx,
                    &format!("children:{parent}:{pkey}"),
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

pub(crate) fn thread_subject(repo: &str, t: &enzarb_connector::Thread, state: &str) -> Subject {
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
        comments: t
            .comments
            .iter()
            .map(|c| SubjectComment {
                author: c.author.login.clone(),
                body: c.comment.body.clone(),
                at: c.created_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
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
