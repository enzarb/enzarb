//! Work item dependencies (docs/adr/0017-work-item-dependencies.md).
//!
//! * [`record_text`] stores the `Depends on #N` references of a connector event's body or
//!   comment (the `text` and `comment` sources of `work_item_dependencies`).
//! * [`sync_github`] stores GitHub's native "blocked by" relationships (the `github` source).
//! * [`open_dependencies`] is what a work item still waits on, read for every event by the
//!   reducer and by the ticker's sweep ([`crate::Ticker`]).
//!
//! A dependency counts only inside one Project; references to other Projects are ignored.
//! An item is done when its work item is `Done` (issue) or `Merged` (change request).

use std::sync::Arc;

use enzarb_connector::dependencies::text_dependencies;
use enzarb_connector::{Connector, EventType, RepoKey, WorkItemKey};
use sqlx::{PgConnection, PgPool};

use crate::error::Error;
use crate::executor::ConnectorRegistry;
use crate::index::ProjectIndex;
use crate::machine::graph::{BlockingDependency, DependencyProgress, dependency_progress};
use crate::machine::stack::{ParentRow, select_stack_parent};
use crate::machine::{EngineEvent, EventPayload, OpenDependency, StackParent};
use crate::reducer::normalize_key;

/// How a dependency reads from an item in `home`: `#12`, or `owner/repo#12` elsewhere.
fn label(home: &RepoKey, dep: &WorkItemKey) -> String {
    if dep.repo == *home {
        format!("#{}", dep.number)
    } else {
        format!("{}#{}", dep.repo.full_name(), dep.number)
    }
}

fn parse_key(s: &str) -> Option<WorkItemKey> {
    s.parse::<WorkItemKey>().ok().map(|k| normalize_key(&k))
}

/// A title as one line of plain text for a prompt.
fn one_line(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['<', '>'], "")
}

/// One `dependency_status` row: `(depends_on_key, status, state, title, url)`.
pub type DependencyRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Render `dependency_status` rows `(depends_on_key, status, state, title, url)` as the
/// `{dependencies}` block of a prompt, one line per dependency.
pub fn format_dependencies(home: &RepoKey, rows: &[DependencyRow]) -> String {
    let mut lines = Vec::new();
    for (dep_key, status, state, title, url) in rows {
        let Some(dep) = parse_key(dep_key) else {
            continue;
        };
        let mut line = format!("- {}", label(home, &dep));
        if let Some(t) = title.as_deref().map(one_line).filter(|t| !t.is_empty()) {
            line.push_str(&format!(" ({t})"));
        }
        let state = state.as_deref().unwrap_or("not started");
        line.push_str(&format!(": {status} ({state})"));
        if let Some(u) = url.as_deref().filter(|u| !u.is_empty()) {
            line.push_str(&format!(" {u}"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// The `{dependencies}` block for `key`: everything it waits on, done or not. Empty when it
/// has no dependencies (the prompt renders that as `(none)`).
pub async fn dependency_context(pool: &PgPool, key: &WorkItemKey) -> Result<String, Error> {
    let rows: Vec<DependencyRow> = sqlx::query_as(
        "SELECT DISTINCT ON (depends_on_key) depends_on_key, status, state, title, url
             FROM workflows.dependency_status
             WHERE work_item_key = $1
             ORDER BY depends_on_key",
    )
    .bind(key.to_string())
    .fetch_all(pool)
    .await?;
    Ok(format_dependencies(&key.repo, &rows))
}

/// The dependencies of `key` that are not *completed* (see
/// [`crate::machine::graph::dependency_progress`]): what blocks a graph task from starting, or
/// from running an agent that pushes. Stable order (by key).
pub async fn blocking_dependencies(
    conn: &mut PgConnection,
    key: &WorkItemKey,
) -> Result<Vec<BlockingDependency>, Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
    )> = sqlx::query_as(
        r"SELECT DISTINCT ON (e.depends_on_key) e.depends_on_key, dw.workflow, dw.state,
                 dw.state_data->>'title', dw.state_data->>'url', cw.state, cw.state_data->>'url',
                 (dw.state_data->>'change_request') IS NOT NULL
          FROM workflows.effective_dependencies e
          LEFT JOIN workflows.work_items dw ON dw.key = e.depends_on_key
          LEFT JOIN workflows.work_items cw ON cw.workflow = 'change_request'
               AND ((dw.workflow = 'change_request' AND cw.key = dw.key)
                 OR ((dw.workflow = 'issue' OR dw.workflow LIKE 'graph:%')
                     AND dw.state_data->>'change_request' IS NOT NULL
                     AND cw.key = regexp_replace(dw.key, '#[0-9]+$', '') || '#'
                                  || (dw.state_data->>'change_request')))
          WHERE e.work_item_key = $1
          ORDER BY e.depends_on_key",
    )
    .bind(key.to_string())
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for (dep_key, workflow, state, title, url, cr_state, cr_url, has_cr) in rows {
        // A change request that is not tracked (yet) is not merged.
        let cr_state = cr_state.or_else(|| has_cr.then(|| "Open".to_string()));
        let progress =
            dependency_progress(workflow.as_deref(), state.as_deref(), cr_state.as_deref());
        if progress == DependencyProgress::Completed {
            continue;
        }
        let Some(dep) = parse_key(&dep_key) else {
            continue;
        };
        out.push(BlockingDependency {
            label: label(&key.repo, &dep),
            key: dep_key,
            title: title.unwrap_or_default(),
            state: state.unwrap_or_default(),
            url: url.unwrap_or_default(),
            pr_url: cr_url.unwrap_or_default(),
            failed: progress == DependencyProgress::Failed,
        });
    }
    Ok(out)
}

/// What `key` still waits on, in a stable order: dependencies that are not done, then its
/// dependency cycle if it is on one.
pub async fn open_dependencies(
    conn: &mut PgConnection,
    key: &WorkItemKey,
) -> Result<Vec<OpenDependency>, Error> {
    let key_s = key.to_string();
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT ON (depends_on_key) depends_on_key, status, url
         FROM workflows.dependency_status
         WHERE work_item_key = $1 AND status <> 'done'
         ORDER BY depends_on_key",
    )
    .bind(&key_s)
    .fetch_all(&mut *conn)
    .await?;
    let mut out: Vec<OpenDependency> = Vec::new();
    for (dep_key, status, url) in rows {
        let Some(dep) = parse_key(&dep_key) else {
            continue;
        };
        let label = label(&key.repo, &dep);
        let problem = if status == "closed" {
            format!("dependency {label} closed without completing")
        } else {
            String::new()
        };
        out.push(OpenDependency {
            key: dep_key,
            label,
            url: url.unwrap_or_default(),
            problem,
        });
    }
    let cycle: Option<Vec<String>> = sqlx::query_scalar("SELECT workflows.dependency_cycle($1)")
        .bind(&key_s)
        .fetch_one(&mut *conn)
        .await?;
    if let Some(path) = cycle {
        let labels: Vec<String> = path
            .iter()
            .filter_map(|k| parse_key(k))
            .map(|k| label(&key.repo, &k))
            .collect();
        out.push(OpenDependency {
            key: String::new(),
            label: "dependency cycle".into(),
            url: String::new(),
            problem: format!("dependency cycle: {}", labels.join(" → ")),
        });
    }
    Ok(out)
}

/// The change request `key` is stacked on, or could be stacked on (docs/workflows.md,
/// "Stacked change requests").
///
/// * `stacked_on` is the parent recorded in the item's state: its current state is read, with
///   `merged` / `closed` set accordingly.
/// * Otherwise the parent is chosen from the item's dependencies by [`select_stack_parent`].
pub async fn stack_parent(
    conn: &mut PgConnection,
    key: &WorkItemKey,
    stacked_on: Option<&str>,
) -> Result<Option<StackParent>, Error> {
    if let Some(parent) = stacked_on {
        let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT state, state_data->>'head_ref', state_data->>'head_sha'
             FROM workflows.work_items WHERE key = $1",
        )
        .bind(parent)
        .fetch_optional(&mut *conn)
        .await?;
        return Ok(row.map(|(state, head_ref, head_sha)| StackParent {
            key: parent.to_string(),
            head_ref: head_ref.unwrap_or_default(),
            head_sha: head_sha.unwrap_or_default(),
            merged: state == "Merged",
            closed: state == "Closed" || state == "Cancelled",
        }));
    }
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        r"SELECT dw.state, cw.key, cw.state,
                 cw.state_data->>'head_ref', cw.state_data->>'head_sha'
          FROM (SELECT DISTINCT depends_on_key FROM workflows.effective_dependencies
                WHERE work_item_key = $1) e
          LEFT JOIN workflows.work_items dw ON dw.key = e.depends_on_key
          LEFT JOIN workflows.work_items cw ON cw.workflow = 'change_request'
               AND ((dw.workflow = 'change_request' AND cw.key = dw.key)
                 OR ((dw.workflow = 'issue' OR dw.workflow LIKE 'graph:%')
                     AND dw.state_data->>'change_request' IS NOT NULL
                     AND cw.key = regexp_replace(dw.key, '#[0-9]+$', '') || '#'
                                  || (dw.state_data->>'change_request')))",
    )
    .bind(key.to_string())
    .fetch_all(&mut *conn)
    .await?;
    let rows: Vec<ParentRow> = rows
        .into_iter()
        .map(
            |(dep_state, cr_key, cr_state, head_ref, head_sha)| ParentRow {
                dep_state,
                cr_key,
                cr_state,
                head_ref,
                head_sha,
            },
        )
        .collect();
    Ok(select_stack_parent(&rows))
}

/// Tell the stacked children of the change request `parent` that it reached a final state
/// (`DependenciesChanged`, so a merge retargets them at once rather than at the next sweep).
/// The deduplication key makes repeating this harmless. Returns the children notified.
pub async fn notify_stacked_children(
    conn: &mut PgConnection,
    parent: &WorkItemKey,
    state: &str,
) -> Result<usize, Error> {
    let children: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM workflows.work_items
         WHERE workflow = 'change_request' AND state_data->>'stacked_on' = $1
           AND state NOT IN ('Merged', 'Closed', 'Cancelled')",
    )
    .bind(parent.to_string())
    .fetch_all(&mut *conn)
    .await?;
    for child in &children {
        let _: Option<i64> =
            sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
                .bind("engine")
                .bind(format!("stack-parent-final:{parent}:{state}:{child}"))
                .bind(child)
                .bind("DependenciesChanged")
                .bind(serde_json::json!({}))
                .fetch_one(&mut *conn)
                .await?;
    }
    Ok(children.len())
}

/// Store the dependencies written in the body or comment of a connector event on `key`.
/// `same_project` says whether a referenced item belongs to the item's Project; others are
/// ignored. Returns whether the stored set changed.
pub async fn record_text(
    conn: &mut PgConnection,
    key: &WorkItemKey,
    ev: &EngineEvent,
    commenter_can_write: bool,
    same_project: impl Fn(&WorkItemKey) -> bool,
) -> Result<bool, Error> {
    let EventPayload::Connector(e) = &ev.payload else {
        return Ok(false);
    };
    let (text, source, replace) = match e.event_type {
        EventType::WorkItemOpened
        | EventType::ChangeRequestOpened
        | EventType::ChangeRequestUpdated => (e.data.item.body.as_str(), "text", true),
        // A comment can add a dependency but cannot retract what another one said. Only
        // commenters with write access count: anyone else could close the merge gate.
        EventType::CommentAdded | EventType::Mentioned => match &e.data.comment {
            Some(c) if commenter_can_write => (c.body.as_str(), "comment", false),
            _ => return Ok(false),
        },
        _ => return Ok(false),
    };
    let mut keys: Vec<String> = Vec::new();
    for dep in text_dependencies(text, &e.data.repo) {
        let dep = normalize_key(&dep);
        if dep == *key {
            continue;
        }
        if !same_project(&dep) {
            tracing::info!(item = %key, dependency = %dep, "ignoring a dependency outside the project");
            continue;
        }
        keys.push(dep.to_string());
    }
    if keys.is_empty() && !replace {
        return Ok(false);
    }
    set(conn, key, source, &keys, replace).await
}

async fn set(
    conn: &mut PgConnection,
    key: &WorkItemKey,
    source: &str,
    deps: &[String],
    replace: bool,
) -> Result<bool, Error> {
    Ok(
        sqlx::query_scalar("SELECT workflows.set_dependencies($1, $2, $3, $4)")
            .bind(key.to_string())
            .bind(source)
            .bind(deps)
            .bind(replace)
            .fetch_one(conn)
            .await?,
    )
}

/// Store the structure of an epic (docs/adr/0018-epics.md): the epic waits for every child,
/// and a child waits for the children it is `Blocked by`. `children` pairs each child with its
/// blockers. This is the `epic` source, so GitHub syncs and text edits never remove it, and
/// repeating the call changes nothing. Returns whether the stored set changed.
pub async fn record_epic(
    pool: &PgPool,
    epic: &WorkItemKey,
    children: &[(WorkItemKey, Vec<WorkItemKey>)],
) -> Result<bool, Error> {
    let mut conn = pool.acquire().await?;
    let mut changed = false;
    let all: Vec<String> = children
        .iter()
        .map(|(c, _)| normalize_key(c).to_string())
        .collect();
    changed |= set(&mut conn, &normalize_key(epic), "epic", &all, false).await?;
    for (child, blockers) in children {
        let blockers: Vec<String> = blockers
            .iter()
            .map(|b| normalize_key(b).to_string())
            .collect();
        if !blockers.is_empty() {
            changed |= set(&mut conn, &normalize_key(child), "epic", &blockers, false).await?;
        }
    }
    Ok(changed)
}

/// Replace the `github` dependencies of every active work item with what the connector
/// reports, one batch of at most `batch` items with an id above `after`. A connector without
/// the capability (or a token without access) is skipped. Returns how many items changed
/// and the cursor for the next pass (0 once the end is reached, so passes wrap around and
/// every item is visited in turn).
pub async fn sync_github(
    pool: &PgPool,
    connectors: &ConnectorRegistry,
    index: &Arc<dyn ProjectIndex>,
    after: i64,
    batch: i64,
) -> Result<(usize, i64), Error> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, key, project_id FROM work_items
         WHERE workflow IN ('issue', 'change_request')
           AND state NOT IN ('Done', 'Duplicate', 'Cancelled', 'Merged', 'Closed')
           AND id > $1
         ORDER BY id LIMIT $2",
    )
    .bind(after)
    .bind(batch)
    .fetch_all(pool)
    .await?;
    let next = match rows.last() {
        Some((id, ..)) if rows.len() as i64 == batch => *id,
        _ => 0,
    };
    let mut changed = 0;
    for (_, key_s, project) in rows {
        let (Some(key), Some(project)) = (parse_key(&key_s), project) else {
            continue;
        };
        let Some(conn): Option<&Arc<dyn Connector>> = connectors.get(&key.repo.connector) else {
            continue;
        };
        let deps = match conn.list_dependencies(&key).await {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, item = %key, "listing dependencies failed");
                continue;
            }
        };
        let keys: Vec<String> = deps
            .iter()
            .map(normalize_key)
            .filter(|d| {
                index
                    .lookup(&d.repo.to_string())
                    .is_some_and(|p| p.name == project)
            })
            .map(|d| d.to_string())
            .collect();
        let mut c = pool.acquire().await?;
        if set(&mut c, &key, "github", &keys, true).await? {
            changed += 1;
        }
    }
    Ok((changed, next))
}
