//! GitHub webhook payload -> normalized [`Event`]s (contracts section 5).
//!
//! Pure: no I/O, so the receiver can call it without holding App credentials.

use chrono::{DateTime, Utc};
use enzarb_connector::command::{has_approve_command, mentions_handle};
use enzarb_connector::epic::parse_epic_marker;
use enzarb_connector::{
    Actor, CheckSummary, CiInfo, CiState, CommentKind, CommentRef, Event, EventData, EventType,
    ItemKind, ItemRef, RepoKey, Review, check_state,
};
use serde::{Deserialize, Serialize};

/// Event `source` attribute and `RepoKey` connector name.
pub const SOURCE: &str = "github";

/// Label that grants approval when added to an issue or PR.
pub const APPROVED_LABEL: &str = "enzarb:approved";

#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    #[error("malformed {event} payload: {source}")]
    Malformed {
        event: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{event} payload is missing `{field}`")]
    Missing { event: String, field: &'static str },
}

/// Result of normalizing one webhook delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Normalized {
    /// Zero or more work events, in emission order. Empty means the delivery
    /// is valid but irrelevant (ping, unmapped action, bot-authored).
    Events { events: Vec<Event> },
    /// App installed / removed / repositories changed. Not a workflow event;
    /// the receiver updates `connectors` state instead.
    Installation { change: Box<InstallationChange> },
    /// A commit `status` delivery. It names no pull request, so turning it
    /// into events needs an API lookup (`GitHubConnector::resolve_commit_status`).
    CommitStatus { status: Box<CommitStatusChange> },
}

impl Normalized {
    /// The work events, or an empty vec for an installation change or a
    /// commit status that is not yet resolved to pull requests.
    pub fn into_events(self) -> Vec<Event> {
        match self {
            Normalized::Events { events } => events,
            Normalized::Installation { .. } | Normalized::CommitStatus { .. } => Vec::new(),
        }
    }
}

/// A `status` delivery that did not come from the bot itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitStatusChange {
    /// Delivery id.
    pub delivery: String,
    pub repo: RepoKey,
    pub sha: String,
    pub state: CiState,
    pub context: Option<String>,
    pub url: Option<String>,
    pub actor: Actor,
    pub time: Option<DateTime<Utc>>,
}

/// One `CiStatusChanged` event per pull request. The first keeps the delivery
/// id; the rest get `<delivery>#pr<number>` so each dedupe key is distinct.
pub(crate) fn ci_events(
    delivery: &str,
    repo: &RepoKey,
    actor: &Actor,
    time: Option<DateTime<Utc>>,
    ci: &CiInfo,
    items: Vec<ItemRef>,
) -> Vec<Event> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let id = if i == 0 {
                delivery.to_string()
            } else {
                format!("{delivery}#pr{}", item.number)
            };
            Event::new(
                SOURCE,
                id,
                EventType::CiStatusChanged,
                time.unwrap_or_else(Utc::now),
                EventData {
                    repo: repo.clone(),
                    actor: actor.clone(),
                    item,
                    comment: None,
                    merged: None,
                    review: None,
                    ci: Some(ci.clone()),
                    label: None,
                },
            )
        })
        .collect()
}

pub(crate) fn state_of_status(state: &str) -> CiState {
    match state {
        "success" => CiState::Success,
        "failure" | "error" => CiState::Failure,
        _ => CiState::Pending,
    }
}

/// The App slug for a bot login: `enzarb[bot]` -> `enzarb`.
fn bot_slug(bot_login: &str) -> &str {
    bot_login.strip_suffix("[bot]").unwrap_or(bot_login)
}

/// An `installation` or `installation_repositories` delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationChange {
    /// Delivery id.
    pub id: String,
    /// `installation` or `installation_repositories`.
    pub event: String,
    /// `created`, `deleted`, `suspend`, `unsuspend`, `new_permissions_accepted`,
    /// `added`, `removed`.
    pub action: String,
    pub installation_id: u64,
    pub account_login: String,
    pub account_id: u64,
    /// `User` or `Organization`.
    pub account_type: String,
    /// `all` or `selected`.
    pub repository_selection: String,
    /// Repos now available (`installation.created`) or added.
    pub repositories_added: Vec<RepoKey>,
    /// Repos removed (`installation.deleted`: all of them).
    pub repositories_removed: Vec<RepoKey>,
    pub sender: Actor,
}

// ---- payload shapes (only the fields we read) ----

#[derive(Deserialize)]
pub(crate) struct User {
    pub(crate) login: String,
    pub(crate) id: u64,
    #[serde(default, rename = "type")]
    pub(crate) kind: Option<String>,
}

impl User {
    pub(crate) fn actor(&self) -> Actor {
        Actor {
            login: self.login.clone(),
            id: self.id,
            is_bot: self.kind.as_deref() == Some("Bot") || self.login.ends_with("[bot]"),
        }
    }
}

#[derive(Deserialize)]
struct Repository {
    name: String,
    owner: User,
}

impl Repository {
    fn key(&self) -> RepoKey {
        RepoKey::new(SOURCE, &self.owner.login, &self.name)
    }
}

#[derive(Deserialize)]
struct RepoStub {
    full_name: String,
}

impl RepoStub {
    fn key(&self) -> Option<RepoKey> {
        let (o, r) = self.full_name.split_once('/')?;
        Some(RepoKey::new(SOURCE, o, r))
    }
}

#[derive(Deserialize)]
pub(crate) struct Issue {
    pub(crate) number: u64,
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) body: Option<String>,
    pub(crate) user: User,
    /// Present when the "issue" is a pull request (issue_comment events).
    #[serde(default)]
    pub(crate) pull_request: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) closed_at: Option<DateTime<Utc>>,
}

impl Issue {
    pub(crate) fn item(&self) -> ItemRef {
        ItemRef {
            kind: if self.pull_request.is_some() {
                ItemKind::ChangeRequest
            } else {
                ItemKind::Issue
            },
            number: self.number,
            url: self.html_url.clone(),
            title: self.title.clone(),
            body: self.body.clone().unwrap_or_default(),
            author: self.user.login.clone(),
            head_ref: None,
            base_ref: None,
            head_sha: None,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Branch {
    #[serde(rename = "ref")]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) sha: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PullRequest {
    pub(crate) number: u64,
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) body: Option<String>,
    pub(crate) user: User,
    pub(crate) head: Branch,
    pub(crate) base: Branch,
    #[serde(default)]
    pub(crate) state: Option<String>,
    #[serde(default)]
    pub(crate) merged: Option<bool>,
    #[serde(default)]
    pub(crate) created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) closed_at: Option<DateTime<Utc>>,
}

impl PullRequest {
    pub(crate) fn item(&self) -> ItemRef {
        ItemRef {
            kind: ItemKind::ChangeRequest,
            number: self.number,
            url: self.html_url.clone(),
            title: self.title.clone(),
            body: self.body.clone().unwrap_or_default(),
            author: self.user.login.clone(),
            head_ref: Some(self.head.name.clone()),
            base_ref: Some(self.base.name.clone()),
            head_sha: self.head.sha.clone(),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Comment {
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) created_at: Option<DateTime<Utc>>,
}

impl Comment {
    pub(crate) fn comment_ref(&self) -> CommentRef {
        self.comment_ref_kind(CommentKind::Conversation)
    }

    pub(crate) fn comment_ref_kind(&self, kind: CommentKind) -> CommentRef {
        CommentRef {
            id: self.id,
            kind,
            body: self.body.clone(),
            url: self.html_url.clone(),
        }
    }
}

#[derive(Deserialize)]
struct ReviewPayload {
    id: u64,
    #[serde(default)]
    state: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    submitted_at: Option<DateTime<Utc>>,
}

/// The pull request stub inside `check_suite` / `check_run` payloads.
#[derive(Deserialize)]
struct PrStub {
    number: u64,
    head: Branch,
    base: Branch,
}

impl PrStub {
    /// Title, body and author are not in the stub and stay empty.
    fn item(&self, repo: &RepoKey) -> ItemRef {
        ItemRef {
            kind: ItemKind::ChangeRequest,
            number: self.number,
            url: format!(
                "https://github.com/{}/pull/{}",
                repo.full_name(),
                self.number
            ),
            title: String::new(),
            body: String::new(),
            author: String::new(),
            head_ref: Some(self.head.name.clone()),
            base_ref: Some(self.base.name.clone()),
            head_sha: self.head.sha.clone(),
        }
    }
}

#[derive(Deserialize)]
struct AppStub {
    #[serde(default)]
    slug: String,
}

#[derive(Deserialize)]
struct CheckSuitePayload {
    head_sha: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    app: Option<AppStub>,
    #[serde(default)]
    pull_requests: Vec<PrStub>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct CheckRunPayload {
    head_sha: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    app: Option<AppStub>,
    #[serde(default)]
    pull_requests: Vec<PrStub>,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct Label {
    name: String,
}

#[derive(Deserialize)]
struct Account {
    login: String,
    id: u64,
    #[serde(default, rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct InstallationPayload {
    id: u64,
    /// Absent on the minimal `installation` stub that rides on other events.
    #[serde(default)]
    account: Option<Account>,
    #[serde(default)]
    repository_selection: String,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    action: String,
    #[serde(default)]
    sender: Option<User>,
    #[serde(default)]
    repository: Option<Repository>,
    #[serde(default)]
    issue: Option<Issue>,
    #[serde(default)]
    pull_request: Option<PullRequest>,
    #[serde(default)]
    comment: Option<Comment>,
    #[serde(default)]
    review: Option<ReviewPayload>,
    #[serde(default)]
    label: Option<Label>,
    #[serde(default)]
    check_suite: Option<CheckSuitePayload>,
    #[serde(default)]
    check_run: Option<CheckRunPayload>,
    // `status` events carry these at the top level.
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    installation: Option<InstallationPayload>,
    #[serde(default)]
    repositories: Vec<RepoStub>,
    #[serde(default)]
    repositories_added: Vec<RepoStub>,
    #[serde(default)]
    repositories_removed: Vec<RepoStub>,
}

fn missing(event: &str, field: &'static str) -> NormalizeError {
    NormalizeError::Missing {
        event: event.to_string(),
        field,
    }
}

/// Normalizes one webhook delivery.
///
/// * `event_name`: `X-GitHub-Event`.
/// * `delivery_id`: `X-GitHub-Delivery`; becomes the primary event's `id`.
///   When one delivery yields several events (a comment that is also a
///   mention and an approval) the extra events get `<delivery>#mentioned` /
///   `<delivery>#approved` so each has a distinct dedupe key.
/// * `bot_handle`: the `@handle` that counts as a mention (no `@`).
/// * `bot_login`: the App's own login (`enzarb[bot]`); its events are dropped.
pub fn normalize(
    event_name: &str,
    delivery_id: &str,
    body: &[u8],
    bot_handle: &str,
    bot_login: &str,
) -> Result<Normalized, NormalizeError> {
    let p: Payload = serde_json::from_slice(body).map_err(|source| NormalizeError::Malformed {
        event: event_name.to_string(),
        source,
    })?;

    if matches!(event_name, "installation" | "installation_repositories") {
        return installation(event_name, delivery_id, p).map(|change| Normalized::Installation {
            change: Box::new(change),
        });
    }

    let ignored = Normalized::Events { events: vec![] };
    if !matches!(
        event_name,
        "issues"
            | "issue_comment"
            | "pull_request"
            | "pull_request_review"
            | "pull_request_review_comment"
            | "check_suite"
            | "check_run"
            | "status"
    ) {
        return Ok(ignored);
    }
    let sender = p
        .sender
        .as_ref()
        .ok_or_else(|| missing(event_name, "sender"))?;
    // Loop protection only matters for comments and reviews. For CI events the
    // sender is whoever pushed the commit, which is the bot for its own PRs;
    // `from_bot(app)` below excludes check suites created by our own app.
    let is_ci_event = matches!(event_name, "check_suite" | "check_run" | "status");
    // The one bot event that passes: a sub-issue enzarb created for an epic opens its own
    // workflow. The engine checks the marker against the stored parent and the sender.
    let epic_child = event_name == "issues"
        && p.action == "opened"
        && p.issue
            .as_ref()
            .and_then(|i| i.body.as_deref())
            .is_some_and(|b| parse_epic_marker(b).is_some());
    if !is_ci_event && !epic_child && sender.login.eq_ignore_ascii_case(bot_login) {
        return Ok(ignored);
    }
    let actor = sender.actor();
    let repo = p
        .repository
        .as_ref()
        .ok_or_else(|| missing(event_name, "repository"))?
        .key();

    if event_name == "status" {
        let sha = p.sha.clone().ok_or_else(|| missing(event_name, "sha"))?;
        let state = p
            .state
            .as_deref()
            .ok_or_else(|| missing(event_name, "state"))?;
        return Ok(Normalized::CommitStatus {
            status: Box::new(CommitStatusChange {
                delivery: delivery_id.to_string(),
                repo,
                sha,
                state: state_of_status(state),
                context: p.context.clone(),
                url: p.target_url.clone().filter(|u| !u.is_empty()),
                actor,
                time: p.updated_at,
            }),
        });
    }

    let slug = bot_slug(bot_login);
    let from_bot = |app: &Option<AppStub>| {
        app.as_ref()
            .is_some_and(|a| a.slug.eq_ignore_ascii_case(slug))
    };

    let mut b = Builder {
        delivery: delivery_id,
        repo,
        actor,
        out: Vec::new(),
    };

    match (event_name, p.action.as_str()) {
        ("check_suite", "completed") => {
            let cs = p
                .check_suite
                .as_ref()
                .ok_or_else(|| missing(event_name, "check_suite"))?;
            if !from_bot(&cs.app) {
                let ci = CiInfo {
                    head_sha: cs.head_sha.clone(),
                    state: check_state(&CheckSummary {
                        status: "completed".into(),
                        conclusion: cs.conclusion.clone(),
                    }),
                    context: None,
                    url: None,
                };
                let items = cs.pull_requests.iter().map(|pr| pr.item(&b.repo)).collect();
                b.out = ci_events(delivery_id, &b.repo, &b.actor, cs.updated_at, &ci, items);
            }
        }
        ("check_run", "completed") => {
            let cr = p
                .check_run
                .as_ref()
                .ok_or_else(|| missing(event_name, "check_run"))?;
            if !from_bot(&cr.app) {
                let ci = CiInfo {
                    head_sha: cr.head_sha.clone(),
                    state: check_state(&CheckSummary {
                        status: "completed".into(),
                        conclusion: cr.conclusion.clone(),
                    }),
                    context: cr.name.clone(),
                    url: cr.html_url.clone(),
                };
                let items = cr.pull_requests.iter().map(|pr| pr.item(&b.repo)).collect();
                b.out = ci_events(delivery_id, &b.repo, &b.actor, cr.completed_at, &ci, items);
            }
        }
        ("issues", "opened") => {
            let i = p
                .issue
                .as_ref()
                .ok_or_else(|| missing(event_name, "issue"))?;
            let item = i.item();
            let t = i.created_at.or(i.updated_at);
            b.push(EventType::WorkItemOpened, t, item.clone(), None, None, None);
            if mentions_handle(&item.body, bot_handle) {
                b.push_extra(EventType::Mentioned, "mentioned", t, item, None);
            }
        }
        ("issues", "closed") => {
            let i = p
                .issue
                .as_ref()
                .ok_or_else(|| missing(event_name, "issue"))?;
            let t = i.closed_at.or(i.updated_at);
            b.push(
                EventType::WorkItemClosed,
                t,
                i.item(),
                None,
                Some(false),
                None,
            );
        }
        ("issues", "labeled") => {
            let i = p
                .issue
                .as_ref()
                .ok_or_else(|| missing(event_name, "issue"))?;
            if is_approval_label(&p) {
                b.push(
                    EventType::ApprovalGranted,
                    i.updated_at,
                    i.item(),
                    None,
                    None,
                    None,
                );
            } else if let Some(l) = &p.label {
                // Any other label (lowercased): `epic`, and the labels graph waits on
                // `labeled` branch on (ADR-0020). Workflows ignore the rest.
                b.push_labeled(i.updated_at, i.item(), &l.name.to_ascii_lowercase());
            }
        }
        ("issue_comment", "created") => {
            let i = p
                .issue
                .as_ref()
                .ok_or_else(|| missing(event_name, "issue"))?;
            let c = p
                .comment
                .as_ref()
                .ok_or_else(|| missing(event_name, "comment"))?;
            b.comment(c, i.item(), bot_handle, CommentKind::Conversation);
        }
        ("pull_request_review_comment", "created") => {
            let pr = p
                .pull_request
                .as_ref()
                .ok_or_else(|| missing(event_name, "pull_request"))?;
            let c = p
                .comment
                .as_ref()
                .ok_or_else(|| missing(event_name, "comment"))?;
            b.comment(c, pr.item(), bot_handle, CommentKind::Review);
        }
        ("pull_request", "opened" | "reopened") => {
            let pr = p
                .pull_request
                .as_ref()
                .ok_or_else(|| missing(event_name, "pull_request"))?;
            let item = pr.item();
            let t = if p.action == "opened" {
                pr.created_at.or(pr.updated_at)
            } else {
                pr.updated_at
            };
            b.push(
                EventType::ChangeRequestOpened,
                t,
                item.clone(),
                None,
                None,
                None,
            );
            if mentions_handle(&item.body, bot_handle) {
                b.push_extra(EventType::Mentioned, "mentioned", t, item, None);
            }
        }
        ("pull_request", "synchronize") => {
            let pr = p
                .pull_request
                .as_ref()
                .ok_or_else(|| missing(event_name, "pull_request"))?;
            b.push(
                EventType::ChangeRequestUpdated,
                pr.updated_at,
                pr.item(),
                None,
                None,
                None,
            );
        }
        ("pull_request", "closed") => {
            let pr = p
                .pull_request
                .as_ref()
                .ok_or_else(|| missing(event_name, "pull_request"))?;
            let t = pr.closed_at.or(pr.updated_at);
            let merged = pr.merged.unwrap_or(false);
            b.push(
                EventType::WorkItemClosed,
                t,
                pr.item(),
                None,
                Some(merged),
                None,
            );
        }
        ("pull_request", "labeled") => {
            let pr = p
                .pull_request
                .as_ref()
                .ok_or_else(|| missing(event_name, "pull_request"))?;
            if is_approval_label(&p) {
                b.push(
                    EventType::ApprovalGranted,
                    pr.updated_at,
                    pr.item(),
                    None,
                    None,
                    None,
                );
            }
        }
        ("pull_request_review", "submitted") => {
            let pr = p
                .pull_request
                .as_ref()
                .ok_or_else(|| missing(event_name, "pull_request"))?;
            let r = p
                .review
                .as_ref()
                .ok_or_else(|| missing(event_name, "review"))?;
            let review = Review {
                id: r.id,
                state: r.state.clone(),
                body: r.body.clone().unwrap_or_default(),
            };
            b.push(
                EventType::ReviewSubmitted,
                r.submitted_at.or(pr.updated_at),
                pr.item(),
                None,
                None,
                Some(review),
            );
        }
        _ => {}
    }

    Ok(Normalized::Events { events: b.out })
}

fn is_approval_label(p: &Payload) -> bool {
    p.label
        .as_ref()
        .is_some_and(|l| l.name.eq_ignore_ascii_case(APPROVED_LABEL))
}

struct Builder<'a> {
    delivery: &'a str,
    repo: RepoKey,
    actor: Actor,
    out: Vec<Event>,
}

impl Builder<'_> {
    fn data(&self, item: ItemRef, comment: Option<CommentRef>) -> EventData {
        EventData {
            repo: self.repo.clone(),
            actor: self.actor.clone(),
            item,
            comment,
            merged: None,
            review: None,
            ci: None,
            label: None,
        }
    }

    fn push(
        &mut self,
        ty: EventType,
        time: Option<DateTime<Utc>>,
        item: ItemRef,
        comment: Option<CommentRef>,
        merged: Option<bool>,
        review: Option<Review>,
    ) {
        let mut data = self.data(item, comment);
        data.merged = merged;
        data.review = review;
        self.out.push(Event::new(
            SOURCE,
            self.delivery,
            ty,
            time.unwrap_or_else(Utc::now),
            data,
        ));
    }

    fn push_labeled(&mut self, time: Option<DateTime<Utc>>, item: ItemRef, label: &str) {
        let mut data = self.data(item, None);
        data.label = Some(label.to_string());
        self.out.push(Event::new(
            SOURCE,
            self.delivery,
            EventType::WorkItemLabeled,
            time.unwrap_or_else(Utc::now),
            data,
        ));
    }

    /// An additional event derived from the same delivery.
    fn push_extra(
        &mut self,
        ty: EventType,
        suffix: &str,
        time: Option<DateTime<Utc>>,
        item: ItemRef,
        comment: Option<CommentRef>,
    ) {
        let data = self.data(item, comment);
        self.out.push(Event::new(
            SOURCE,
            format!("{}#{suffix}", self.delivery),
            ty,
            time.unwrap_or_else(Utc::now),
            data,
        ));
    }

    /// `CommentAdded`, plus `Mentioned` and `ApprovalGranted` as the body warrants.
    fn comment(&mut self, c: &Comment, item: ItemRef, bot_handle: &str, kind: CommentKind) {
        let cref = c.comment_ref_kind(kind);
        self.push(
            EventType::CommentAdded,
            c.created_at,
            item.clone(),
            Some(cref.clone()),
            None,
            None,
        );
        if mentions_handle(&c.body, bot_handle) {
            self.push_extra(
                EventType::Mentioned,
                "mentioned",
                c.created_at,
                item.clone(),
                Some(cref.clone()),
            );
        }
        if has_approve_command(&c.body, bot_handle) {
            self.push_extra(
                EventType::ApprovalGranted,
                "approved",
                c.created_at,
                item,
                Some(cref),
            );
        }
    }
}

fn installation(
    event_name: &str,
    delivery_id: &str,
    p: Payload,
) -> Result<InstallationChange, NormalizeError> {
    let inst = p
        .installation
        .ok_or_else(|| missing(event_name, "installation"))?;
    let account = inst
        .account
        .ok_or_else(|| missing(event_name, "installation.account"))?;
    let sender = p.sender.ok_or_else(|| missing(event_name, "sender"))?;
    let keys = |v: &[RepoStub]| v.iter().filter_map(RepoStub::key).collect::<Vec<_>>();
    let (added, removed) = match (event_name, p.action.as_str()) {
        ("installation", "created") => (keys(&p.repositories), vec![]),
        ("installation", "deleted") => (vec![], keys(&p.repositories)),
        _ => (keys(&p.repositories_added), keys(&p.repositories_removed)),
    };
    Ok(InstallationChange {
        id: delivery_id.to_string(),
        event: event_name.to_string(),
        action: p.action,
        installation_id: inst.id,
        account_login: account.login,
        account_id: account.id,
        account_type: account.kind,
        repository_selection: inst.repository_selection,
        repositories_added: added,
        repositories_removed: removed,
        sender: sender.actor(),
    })
}
