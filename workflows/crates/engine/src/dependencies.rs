//! Work item dependencies (docs/adr/0016-work-item-dependencies.md).
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
use crate::machine::{EngineEvent, EventPayload, OpenDependency};
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
