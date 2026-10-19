//! The interpreter (ADR-0022 §6): a pure, deterministic
//! `step(resolved, state, event) -> (state, effects)`. No IO, no clock:
//! time and every result arrive as events.

use crate::commands::CommandRoute;
use crate::document::{Artifact, GitFacts, Relation, Subject, TaskDocument};
use crate::events::{Event, Payload, SubjectKind};
use crate::format::{
    Access, Action, CANCELLED, CommandInfo, Concurrency, HandlerDo, Matcher, MatcherSpec, NumField,
    Output, ResultSpec, Secret, Sender, SubjectLink, TextField, Tools, UiAction, Wait, WaitingOn,
    When, parse_duration,
};
use crate::resolve::{EXIT_LIMIT, Exec, RKind, RNode, RPhase, RRuntime, RStep, Resolved, Route};
use crate::template;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Upper bound on engine-side transitions per event (a loop with no wait or
/// step pauses the task instead of spinning).
const MAX_HOPS: usize = 1000;

// ---------------------------------------------------------------- state

/// The engine phase of a task.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Status {
    #[default]
    New,
    Running,
    Waiting,
    Ended {
        status: String,
    },
    Paused {
        reason: String,
        previous: Box<Status>,
        /// Node re-entered on resume (after an error), else the task continues as it was.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_at: Option<String>,
    },
}

/// Longest agent-reported environment detail kept (characters).
const AGENT_ENVIRONMENT_DETAIL_MAX: usize = 500;

/// The agent's advisory `result.environment.detail`: a non-empty string, truncated. Untrusted
/// text, only ever displayed.
fn agent_environment_detail(result: &Option<Value>) -> Option<String> {
    let d = result
        .as_ref()?
        .get("environment")?
        .get("detail")?
        .as_str()?
        .trim();
    (!d.is_empty()).then(|| d.chars().take(AGENT_ENVIRONMENT_DETAIL_MAX).collect())
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::New => "new",
            Status::Running => "running",
            Status::Waiting => "waiting",
            Status::Ended { .. } => "ended",
            Status::Paused { .. } => "paused",
        }
    }
}

/// One active phase: frame 0 is the top-level phase, deeper frames are nested phases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Frame {
    /// Phase id (frame 0) or the id of the nested phase node.
    pub id: String,
    /// Current node in this phase.
    pub node: String,
    #[serde(default)]
    pub visits: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pending {
    Step { run_id: String, attempt: u32 },
    Action { call_id: String },
}

/// A step that ended with an environment failure the task waits out (ADR-0026). The status
/// stays `running` and `pending` keeps the run id, so older readers of the state still decode it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlatformWait {
    pub kind: String,
    pub detail: String,
    /// The wake condition, resolved from the step when the wait began.
    pub until: When,
    /// Wake-ups so far: the idempotency token of the next `platform_cleared`.
    #[serde(default)]
    pub waits: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Everything the interpreter keeps per task (persisted by the engine).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct TaskState {
    pub status: Status,
    #[serde(default)]
    pub frames: Vec<Frame>,
    #[serde(default)]
    pub pending: Option<Pending>,
    #[serde(default)]
    pub timer: Option<String>,
    /// Entries per top-level phase id.
    #[serde(default)]
    pub entries: BTreeMap<String, u32>,
    pub doc: TaskDocument,
    /// The last finished step or action: `{node, exit, outputs, result, git, environment, missing_outputs}`.
    #[serde(default)]
    pub last: Value,
    #[serde(default)]
    pub last_agent_run: Option<String>,
    /// Counter for deterministic run/call/timer ids.
    #[serde(default)]
    pub seq: u64,
    /// `phase/…/node` of every node entered, in order.
    #[serde(default)]
    pub path: Vec<String>,
    /// Why the task is pinned to an older definition, if it is.
    #[serde(default)]
    pub pinned: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    /// Set while the task waits for the platform; resumes the pending step's run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_wait: Option<PlatformWait>,
    /// Platform waits that ended with a wake-up (the next wait's token).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub platform_waits: u32,
}

impl TaskState {
    pub fn new(doc: TaskDocument) -> Self {
        Self {
            doc,
            ..Default::default()
        }
    }

    /// `phase/…/node` of the current position.
    pub fn position(&self) -> Option<String> {
        let f = self.frames.last()?;
        let mut parts: Vec<&str> = self.frames.iter().map(|f| f.id.as_str()).collect();
        parts.push(&f.node);
        Some(parts.join("/"))
    }

    fn phase_path(&self) -> Vec<String> {
        self.frames.iter().map(|f| f.id.clone()).collect()
    }
}

// ---------------------------------------------------------------- events

/// Input to the interpreter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskEvent {
    /// Creates the task at the first phase.
    Start {
        event: Event,
    },
    /// A catalog or custom event routed to this task.
    Event {
        event: Event,
    },
    StepFinished(StepFinished),
    ActionFinished {
        call_id: String,
        ok: bool,
        #[serde(default)]
        result: Value,
    },
    Timer {
        timer_id: String,
    },
    /// A fact the engine refreshed from git or GitHub.
    FactUpdated {
        fact: String,
        value: Value,
    },
    /// No activity for a long time: pause and notify.
    StallDetected {
        #[serde(default)]
        reason: Option<String>,
    },
    /// A human resumed a paused task.
    Resume,
    /// A writer cancelled the task (engine-native, not a workflow handler): any running step
    /// is stopped and the task ends `cancelled`.
    Cancel,
    /// A platform wait's condition holds (engine-internal, from the ticker). Ignored unless the
    /// task is in the wait of this run and `wait` is its current wake-up count.
    PlatformCleared {
        run_id: String,
        wait: u32,
    },
    /// The engine could not start the pending step (a missing secret, a token the provider
    /// refuses): nothing ran. The step is forgotten and the task pauses at its node, so a
    /// resume (on a possibly re-resolved definition) starts it afresh.
    StepNotStarted {
        run_id: String,
        reason: String,
    },
}

/// The result of a step, reported by the executor.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct StepFinished {
    pub run_id: String,
    /// `success`, `failure`, `interrupted`, …
    pub exit: String,
    /// Declared output files that were present, by name (JSON outputs parsed).
    #[serde(default)]
    pub outputs: Map<String, Value>,
    /// The validated `submit_result` payload.
    #[serde(default)]
    pub result: Option<Value>,
    /// Git facts the executor verified against the remote.
    #[serde(default)]
    pub git: Option<GitFacts>,
    /// Set when the step failed because of its environment, not the code.
    #[serde(default)]
    pub environment: Option<StepEnvironment>,
}

/// What the sandbox observed about its environment (`last.environment`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StepEnvironment {
    /// `disk_full`, `oom`, `toolchain_missing`, `network`, `load` or `unknown`.
    pub kind: String,
    /// Free text saying what was observed.
    pub detail: String,
}

// ---------------------------------------------------------------- effects

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "effect", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Effect {
    RunStep(Box<RunStep>),
    /// Deliver an event into the running agent session as a user turn.
    Steer {
        run_id: String,
        event: Event,
    },
    /// Stop a running step; it finishes with exit `interrupted`.
    StopStep {
        run_id: String,
    },
    CallAction {
        call_id: String,
        name: String,
        args: Map<String, Value>,
    },
    RefreshFact {
        fact: String,
    },
    SetTimer {
        timer_id: String,
        seconds: u64,
    },
    CreateRelations {
        kind: String,
        targets: Vec<Value>,
        /// Set when the targets are objects the successful action just created (`relate.from`):
        /// the engine then gives a `parent` target that has no task one. Targets named before the
        /// call (`relate.targets`) are never adopted, so a failing action adopts nothing.
        #[serde(default)]
        created: bool,
    },
    /// Link a GitHub object an action returned as a subject of the task.
    LinkSubject {
        kind: SubjectKind,
        repo: String,
        number: u64,
    },
    UpdateProjection(Box<Projection>),
    /// Answer a comment on `number` (the task's issue or pull request when unset), as the
    /// bot: a command the wait does not accept.
    Reply {
        number: Option<u64>,
        body: String,
    },
}

/// A step ready to run: templates rendered, permissions and secrets by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunStep {
    pub run_id: String,
    pub attempt: u32,
    /// Platform waits that ended for this run: non-zero when the run continues after a wait
    /// (same `run_id`), so the job key differs per wake-up.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub resume: u32,
    /// `phase/…/node`.
    pub node: String,
    /// The step's `use` reference (or `inline`).
    pub step: String,
    pub exec: RunExec,
    pub workspaces: Vec<RunWorkspace>,
    pub github: BTreeMap<String, Access>,
    /// The step's secrets, normalized (the runtime's own are in its file).
    pub secrets: Vec<Secret>,
    pub priority: i32,
    pub concurrency: Option<Concurrency>,
    pub outputs: BTreeMap<String, Output>,
    pub result: Option<ResultSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunExec {
    Agent {
        #[schemars(with = "Value")]
        runtime: Box<RRuntime>,
        model: Option<String>,
        prompt: String,
    },
    Command {
        run: String,
        tools: Tools,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunWorkspace {
    pub repo: String,
    pub mode: Access,
    /// Rendered `branch` (write workspaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Rendered `base`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// What the UI, notifications and API read about a task (ADR-0022 §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Projection {
    /// Engine phase: `running`, `waiting`, `paused` or `ended`.
    pub status: String,
    pub end_status: Option<String>,
    /// `phase/…` and node id.
    pub phase: String,
    pub node: String,
    pub column: Option<String>,
    pub waiting_on: Option<WaitingOn>,
    pub reason: Option<String>,
    pub text: Option<String>,
    pub actions: Vec<UiAction>,
    /// The commands that apply now (`from: writer` ones only count for writers).
    #[serde(default)]
    pub commands: Vec<CommandInfo>,
    pub artifacts: Vec<Artifact>,
    pub needs_you: bool,
}

// ---------------------------------------------------------------- step

/// Applies one event. Ended tasks ignore everything.
pub fn step(res: &Resolved, state: &TaskState, ev: &TaskEvent) -> (TaskState, Vec<Effect>) {
    let mut m = Machine {
        res,
        s: state.clone(),
        fx: Vec::new(),
    };
    m.apply(ev);
    (m.s, m.fx)
}

/// The commands the task accepts now: the active wait's commands (declared, inherited and
/// matched by its own rules), its buttons, and the engine-native `/cancel`. Empty once the task
/// ended. The reducer's reply and the projection both read this.
pub fn accepted_commands(res: &Resolved, state: &TaskState) -> Vec<CommandInfo> {
    let cancel = || CommandInfo {
        name: "cancel".into(),
        label: "Cancel".into(),
        description: Some("Stop the task".into()),
        from: Sender::Writer,
    };
    let node = res
        .phase(&state.phase_path())
        .zip(state.frames.last())
        .and_then(|(p, f)| p.nodes.get(&f.node));
    match (&state.status, node.map(|n| &n.kind)) {
        (Status::Waiting, Some(RKind::Wait(w))) => {
            let mut out: Vec<CommandInfo> = w
                .commands
                .iter()
                .map(|c| {
                    let name = c.name.as_str().to_string();
                    CommandInfo {
                        label: c
                            .label
                            .as_ref()
                            .map(|l| l.as_str().to_string())
                            .filter(|l| !l.is_empty() && !template::is_template(l))
                            .unwrap_or_else(|| format!("/{name}")),
                        description: c.description.as_ref().map(|d| d.as_str().to_string()),
                        from: c.from.unwrap_or(Sender::Writer),
                        name,
                    }
                })
                .collect();
            if !out.iter().any(|c| c.name == "cancel") {
                out.push(cancel());
            }
            out
        }
        (Status::Running, _) => vec![cancel()],
        _ => Vec::new(),
    }
}

struct Machine<'a> {
    res: &'a Resolved,
    s: TaskState,
    fx: Vec<Effect>,
}

enum Flow {
    Enter(String),
    Exit(String),
    Stop,
}

impl<'a> Machine<'a> {
    fn apply(&mut self, ev: &TaskEvent) {
        if matches!(self.s.status, Status::Ended { .. }) {
            return;
        }
        // A wait's projection re-renders when a fact or var it reads changes
        // without the node being re-entered.
        let before = (self.s.status == Status::Waiting)
            .then(|| (self.s.position(), self.build_projection()));
        self.dispatch(ev);
        if let Some((pos, old)) = before
            && self.s.status == Status::Waiting
            && self.s.position() == pos
            && !self
                .fx
                .iter()
                .any(|e| matches!(e, Effect::UpdateProjection(_)))
        {
            let now = self.build_projection();
            if now != old {
                self.fx.push(Effect::UpdateProjection(Box::new(now)));
            }
        }
    }

    fn dispatch(&mut self, ev: &TaskEvent) {
        match ev {
            TaskEvent::Start { event } => {
                if self.s.status != Status::New {
                    return;
                }
                self.s.doc.event = Some(event.clone());
                match self.res.phases.first() {
                    Some(p) => {
                        let id = p.id.clone();
                        self.enter_phase(&id, 0);
                    }
                    None => self.end("done"),
                }
            }
            TaskEvent::StallDetected { reason } => {
                if !matches!(self.s.status, Status::Paused { .. } | Status::New) {
                    self.pause(reason.clone().unwrap_or_else(|| "stalled".into()), None);
                }
            }
            TaskEvent::Resume => {
                if let Status::Paused {
                    previous,
                    resume_at,
                    ..
                } = self.s.status.clone()
                {
                    self.s.status = *previous;
                    match resume_at {
                        Some(n) => self.run(Flow::Enter(n)),
                        None => self.project(),
                    }
                }
            }
            TaskEvent::Cancel => {
                if self.s.status == Status::New {
                    return;
                }
                if let Some(Pending::Step { run_id, .. }) = self.s.pending.clone() {
                    self.fx.push(Effect::StopStep { run_id });
                }
                self.end(CANCELLED);
            }
            TaskEvent::StepNotStarted { run_id, reason } => {
                if !matches!(&self.s.pending, Some(Pending::Step { run_id: r, .. }) if r == run_id)
                {
                    return;
                }
                self.unpause_for_result();
                self.s.pending = None;
                let here = self.s.frames.last().map(|f| f.node.clone());
                self.pause(reason.clone(), here);
            }
            TaskEvent::PlatformCleared { run_id, wait } => self.on_platform_cleared(run_id, *wait),
            TaskEvent::Event { event } => self.on_event(event),
            TaskEvent::StepFinished(f) => self.on_step_finished(f),
            TaskEvent::ActionFinished {
                call_id,
                ok,
                result,
            } => {
                if self.s.pending
                    != Some(Pending::Action {
                        call_id: call_id.clone(),
                    })
                {
                    return;
                }
                self.unpause_for_result();
                self.s.pending = None;
                let node = self.s.position().unwrap_or_default();
                self.s.last = json!({"node": node, "exit": if *ok { "success" } else { "failure" }, "ok": ok, "result": result, "environment": null});
                if *ok
                    && let Some(RKind::Action(a)) = self.node().map(|n| &n.kind)
                    && let Err(e) = self.action_results(a, result)
                {
                    let here = self.s.frames.last().map(|f| f.node.clone());
                    self.pause(e, here);
                    return;
                }
                self.after_finish();
            }
            TaskEvent::Timer { timer_id } => {
                if self.s.timer.as_deref() == Some(timer_id) && self.s.status == Status::Waiting {
                    self.s.timer = None;
                    let ev = json!({"type": "timer", "timer_id": timer_id});
                    self.wait_event("timer", &ev, true, None);
                }
            }
            TaskEvent::FactUpdated { fact, value } => {
                if let Err(e) = self.s.doc.set_fact(fact, value.clone()) {
                    self.s.notes.push(e);
                    return;
                }
                if self.s.status == Status::Waiting {
                    let ev = json!({"type": "fact", "fact": fact, "value": value});
                    self.wait_event("fact", &ev, true, None);
                }
            }
        }
    }

    // ------------------------------------------------------------ lookup

    fn phase(&self) -> Option<&'a RPhase> {
        self.res.phase(&self.s.phase_path())
    }

    fn node(&self) -> Option<&'a RNode> {
        let f = self.s.frames.last()?;
        self.phase()?.nodes.get(&f.node)
    }

    fn ctx(&self, params: &Map<String, Value>, extra: &[(&str, Value)]) -> Value {
        let mut v = serde_json::to_value(&self.s.doc).unwrap_or_else(|_| json!({}));
        crate::document::add_feedback(&mut v);
        let o = v.as_object_mut().expect("document is an object");
        o.insert("params".into(), Value::Object(params.clone()));
        o.insert("last".into(), self.s.last.clone());
        o.insert(
            "exit".into(),
            self.s.last.get("exit").cloned().unwrap_or(Value::Null),
        );
        for (k, x) in extra {
            o.insert((*k).into(), x.clone());
        }
        v
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.s.seq += 1;
        format!("{prefix}-{}", self.s.seq)
    }

    // ------------------------------------------------------------ events

    fn matches(
        &self,
        m: &Matcher,
        ty: &str,
        ev: &Value,
        can_write: bool,
        default_from: Sender,
        params: &Map<String, Value>,
    ) -> bool {
        let (event, from, cond) = match m {
            Matcher::Type(t) => (t.as_str(), None, None),
            Matcher::Full(MatcherSpec { event, from, cond }) => {
                (event.as_str(), *from, cond.as_deref())
            }
        };
        if event != ty {
            return false;
        }
        if from.unwrap_or(default_from) == Sender::Writer && !can_write {
            return false;
        }
        match cond {
            None => true,
            Some(c) => {
                template::truthy(c, &self.ctx(params, &[("event", ev.clone())])).unwrap_or(false)
            }
        }
    }

    fn on_event(&mut self, event: &Event) {
        let ty = event.type_name().to_string();
        let ev = serde_json::to_value(event).unwrap_or_default();
        let can_write = event.can_write();
        if self.s.status != Status::New && self.handle(&ty, &ev, can_write, event) {
            return;
        }
        // `/cancel` from a writer is engine-native: it ends the task wherever it is.
        if can_write
            && self.s.status != Status::New
            && let Payload::Comment(c) = &event.payload
            && c.command.as_deref() == Some("cancel")
        {
            self.dispatch(&TaskEvent::Cancel);
            return;
        }
        if let Some(Pending::Step { run_id, .. }) = self.s.pending.clone() {
            let Some(RKind::Step(st)) = self.node().map(|n| &n.kind) else {
                return;
            };
            let params = &st.params;
            if self.s.platform_wait.is_some() {
                // Nothing runs: a stop ends the step as interrupted, a steer has no session.
                if st
                    .stop
                    .iter()
                    .any(|m| self.matches(m, &ty, &ev, can_write, Sender::Anyone, params))
                {
                    self.s.pending = None;
                    self.s.platform_wait = None;
                    let pos = self.s.position().unwrap_or_default();
                    self.s.last = json!({"node": pos, "exit": "interrupted", "outputs": {}, "result": null, "git": null, "environment": null, "missing_outputs": []});
                    self.after_finish();
                }
            } else if st
                .stop
                .iter()
                .any(|m| self.matches(m, &ty, &ev, can_write, Sender::Anyone, params))
            {
                self.fx.push(Effect::StopStep { run_id });
            } else if st
                .steer
                .iter()
                .any(|m| self.matches(m, &ty, &ev, can_write, Sender::Anyone, params))
            {
                self.fx.push(Effect::Steer {
                    run_id,
                    event: event.clone(),
                });
            }
            return;
        }
        if self.s.status == Status::Waiting && !self.wait_event(&ty, &ev, can_write, Some(event)) {
            self.reply_unaccepted(event, can_write);
        }
    }

    /// Answers a person's command the wait did not take: what is accepted instead.
    fn reply_unaccepted(&mut self, event: &Event, can_write: bool) {
        let Payload::Comment(c) = &event.payload else {
            return;
        };
        let Some(cmd) = c.command.as_deref() else {
            return;
        };
        if event.actor.as_ref().is_none_or(|a| a.bot) {
            return;
        }
        let all = accepted_commands(self.res, &self.s);
        let usable: Vec<&CommandInfo> = all
            .iter()
            .filter(|i| i.from == Sender::Anyone || can_write)
            .collect();
        let list = if usable.is_empty() {
            "none".to_string()
        } else {
            usable
                .iter()
                .map(|i| format!("`/{}`", i.name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = if all.iter().any(|i| i.name == cmd) {
            format!(
                "`/{cmd}` needs write access to the repository. Accepted from you here: {list}."
            )
        } else {
            format!("`/{cmd}` does not apply right now. Accepted here: {list}.")
        };
        let number = event.subject.as_ref().map(|s| s.number).filter(|n| *n > 0);
        self.fx.push(Effect::Reply { number, body });
    }

    /// Routes a declared command of the active wait. False when the wait declares no route
    /// for it or the sender may not use it.
    fn command_route(&mut self, event: &Event, can_write: bool, ev: &Value) -> bool {
        let Payload::Comment(c) = &event.payload else {
            return false;
        };
        let Some(cmd) = c.command.as_deref() else {
            return false;
        };
        let (Some(node), Some(phase)) = (self.node(), self.phase()) else {
            return false;
        };
        let RKind::Wait(w) = &node.kind else {
            return false;
        };
        let Some(decl) = w.commands.iter().find(|d| d.name.as_str() == cmd) else {
            return false;
        };
        let (Some(to), from) = (&decl.to, decl.from.unwrap_or(Sender::Writer)) else {
            return false;
        };
        if from == Sender::Writer && !can_write {
            return false;
        }
        let Ok(route) = CommandRoute::parse(to, |n| phase.nodes.contains_key(n)) else {
            return false;
        };
        let here = self
            .s
            .frames
            .last()
            .map(|f| f.node.clone())
            .unwrap_or_default();
        let flow = match route {
            CommandRoute::Node(n) => Flow::Enter(n),
            CommandRoute::Exit(e) => Flow::Exit(e),
            CommandRoute::Start => Flow::Enter(phase.start.clone()),
            CommandRoute::Own => Flow::Enter(here),
            CommandRoute::Goto(p) => {
                self.apply_leave_sets(&[("event", ev.clone())]);
                self.s.doc.event = Some(event.clone());
                self.s.pending = None;
                self.s.timer = None;
                self.s.status = Status::Running;
                self.enter_phase(&p, 0);
                return true;
            }
        };
        self.s.doc.event = Some(event.clone());
        self.s.timer = None;
        self.s.status = Status::Running;
        let flow = match flow {
            Flow::Enter(n) => self.leave(n, &[("event", ev.clone())]),
            other => {
                // `leave` applies `set_on_exit`; do the same before leaving by an exit.
                self.apply_leave_sets(&[("event", ev.clone())]);
                other
            }
        };
        self.run(flow);
        true
    }

    /// The `set_on_exit` of the current node.
    fn apply_leave_sets(&mut self, extra: &[(&str, Value)]) {
        let (Some(node), Some(phase)) = (self.node(), self.phase()) else {
            return;
        };
        if let Err(e) = self.apply_set(&node.set_on_exit, &phase.params, extra) {
            self.s.notes.push(format!("set_on_exit: {e}"));
        }
    }

    /// Tries the workflow's task-wide handlers; true when one consumed the event.
    fn handle(&mut self, ty: &str, ev: &Value, can_write: bool, event: &Event) -> bool {
        let res = self.res;
        let Some(h) = res.handlers.iter().find(|h| {
            let from = h.from.unwrap_or_default();
            h.events
                .iter()
                .any(|m| self.matches(m, ty, ev, can_write, from, &res.params))
                && h.cond.as_ref().is_none_or(|c| {
                    template::truthy(c, &self.ctx(&res.params, &[("event", ev.clone())]))
                        .unwrap_or(false)
                })
        }) else {
            return false;
        };
        let running = match &self.s.pending {
            Some(Pending::Step { run_id, .. }) => Some(run_id.clone()),
            _ => None,
        };
        if matches!(h.do_, Some(HandlerDo::StopStep)) && running.is_none() {
            return false;
        }
        if let Err(e) = self.apply_set(&h.set, &res.params, &[("event", ev.clone())]) {
            self.s.notes.push(format!("handler set: {e}"));
        }
        let Some(action) = &h.do_ else {
            return false;
        };
        if let Some(run_id) = running {
            self.fx.push(Effect::StopStep { run_id });
        }
        if matches!(action, HandlerDo::StopStep) {
            return true;
        }
        self.s.doc.event = Some(event.clone());
        self.s.pending = None;
        self.s.timer = None;
        match action {
            HandlerDo::Cancel => self.end(CANCELLED),
            HandlerDo::End(s) => self.end(s),
            HandlerDo::Goto(p) => {
                self.s.status = Status::Running;
                self.enter_phase(p, 0);
            }
            HandlerDo::StopStep => {}
        }
        true
    }

    /// Assigns task variables in order; each sees the ones before it.
    fn apply_set(
        &mut self,
        set: &Map<String, Value>,
        params: &Map<String, Value>,
        extra: &[(&str, Value)],
    ) -> Result<(), String> {
        for (k, v) in set {
            let val = match v {
                Value::String(e) => template::eval(e, &self.ctx(params, extra))
                    .map_err(|e| format!("set `{k}`: {}", e.message))?,
                other => other.clone(),
            };
            self.s.doc.vars.insert(k.clone(), val);
        }
        Ok(())
    }

    /// Leaves the current node toward `to`: `set_on_exit`, then enter.
    fn leave(&mut self, to: String, extra: &[(&str, Value)]) -> Flow {
        let (Some(node), Some(phase)) = (self.node(), self.phase()) else {
            return Flow::Enter(to);
        };
        if let Err(e) = self.apply_set(&node.set_on_exit, &phase.params, extra) {
            let here = self.s.frames.last().map(|f| f.node.clone());
            self.pause(e, here);
            return Flow::Stop;
        }
        Flow::Enter(to)
    }

    /// `as`, `subject` and `relate.from` of a successful action.
    fn action_results(&mut self, a: &Action, result: &Value) -> Result<(), String> {
        let params = self.phase().map(|p| p.params.clone()).unwrap_or_default();
        if let Some(name) = &a.as_ {
            self.s.doc.outputs.insert(name.clone(), result.clone());
        }
        let ctx = self.ctx(&params, &[("result", result.clone())]);
        let eval = |e: &str| template::eval(e, &ctx).map_err(|e| e.message);
        let link = match &a.subject {
            None | Some(SubjectLink::Flag(false)) => None,
            Some(SubjectLink::Flag(true)) => {
                let kind: SubjectKind = serde_json::from_value(result["kind"].clone())
                    .map_err(|_| format!("subject: result has no valid `kind`: {result}"))?;
                Some((kind, result["number"].clone(), result["repo"].clone()))
            }
            Some(SubjectLink::Spec(s)) => Some((
                s.kind,
                eval(s.number.as_deref().unwrap_or("result.number"))?,
                eval(s.repo.as_deref().unwrap_or("result.repo"))?,
            )),
        };
        if let Some((kind, number, repo)) = link {
            let number = number
                .as_u64()
                .or_else(|| number.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| format!("subject: `number` is not a number: {number}"))?;
            let repo = match repo.as_str() {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => self.s.doc.task.repo.clone(),
            };
            let subject = Subject {
                repo: repo.clone(),
                number,
                title: result["title"].as_str().unwrap_or_default().into(),
                url: result["url"].as_str().unwrap_or_default().into(),
                state: "open".into(),
                ..Default::default()
            };
            let subs = &mut self.s.doc.subjects;
            match kind {
                SubjectKind::PullRequest => {
                    if !subs
                        .pull_requests
                        .iter()
                        .any(|p| p.repo == repo && p.number == number)
                    {
                        subs.pull_requests.push(subject);
                    }
                }
                SubjectKind::Issue => {
                    if subs.issue.is_none() {
                        subs.issue = Some(subject);
                    }
                }
            }
            self.fx.push(Effect::LinkSubject { kind, repo, number });
        }
        if let Some(rel) = &a.relate
            && let Some(from) = &rel.from
        {
            let targets = match eval(from).map_err(|e| format!("relate.from: {e}"))? {
                Value::Null => Vec::new(),
                Value::Array(a) => a,
                v => vec![v],
            };
            if !targets.is_empty() {
                for t in &targets {
                    self.s.doc.relations.push(Relation {
                        kind: rel.kind.clone(),
                        target: t.clone(),
                    });
                }
                self.fx.push(Effect::CreateRelations {
                    kind: rel.kind.clone(),
                    targets,
                    created: true,
                });
            }
        }
        Ok(())
    }

    /// Offers an event to the current wait. True when it moved the task: through the wait's
    /// own `next:` rules, else through a command the wait declares.
    fn wait_event(&mut self, ty: &str, ev: &Value, can_write: bool, event: Option<&Event>) -> bool {
        let (Some(node), Some(phase)) = (self.node(), self.phase()) else {
            return false;
        };
        let RKind::Wait(w) = &node.kind else {
            return false;
        };
        let from = w.from.unwrap_or_default();
        let admitted = w
            .events
            .iter()
            .any(|m| self.matches(m, ty, ev, can_write, from, &phase.params));
        if admitted {
            let ctx = self.ctx(&phase.params, &[("event", ev.clone())]);
            match follow(&node.next, &ctx) {
                Ok(Some(to)) => {
                    if let Some(e) = event {
                        self.s.doc.event = Some(e.clone());
                    }
                    self.s.timer = None;
                    self.s.status = Status::Running;
                    let f = self.leave(to, &[("event", ev.clone())]);
                    self.run(f);
                    return true;
                }
                Ok(None) => {} // not for the rules: a declared command may take it
                Err(e) => {
                    self.pause(format!("condition error: {e}"), None);
                    return true;
                }
            }
        }
        if let Some(e) = event
            && self.command_route(e, can_write, ev)
        {
            return true;
        }
        if !admitted
            && let (Some(e), Some(run)) = (event, self.s.last_agent_run.clone())
            && w.steer
                .iter()
                .any(|m| self.matches(m, ty, ev, can_write, from, &phase.params))
        {
            self.fx.push(Effect::Steer {
                run_id: run,
                event: e.clone(),
            });
            return true;
        }
        false
    }

    fn unpause_for_result(&mut self) {
        if let Status::Paused {
            previous,
            resume_at: None,
            ..
        } = &self.s.status
        {
            self.s.status = (**previous).clone();
        }
    }

    fn on_platform_cleared(&mut self, run_id: &str, wait: u32) {
        let Some(w) = &self.s.platform_wait else {
            return;
        };
        let Some(Pending::Step {
            run_id: pending,
            attempt,
        }) = self.s.pending.clone()
        else {
            return;
        };
        if w.waits != wait || pending != run_id || matches!(self.s.status, Status::Paused { .. }) {
            return;
        }
        let Some(RKind::Step(st)) = self.node().map(|n| &n.kind) else {
            return;
        };
        let st: &RStep = st;
        self.s.platform_wait = None;
        self.s.platform_waits += 1;
        self.s.notes.push(format!(
            "{}: platform wait cleared; resuming run {pending}",
            self.s.position().unwrap_or_default()
        ));
        self.send_step(st, attempt, Some(pending));
    }

    fn on_step_finished(&mut self, f: &StepFinished) {
        let Some(Pending::Step { run_id, attempt }) = self.s.pending.clone() else {
            return;
        };
        if run_id != f.run_id {
            return;
        }
        self.unpause_for_result();
        let Some(RKind::Step(st)) = self.node().map(|n| &n.kind) else {
            return;
        };
        let st: &RStep = st;
        self.s.pending = None;
        let node_id = self
            .s
            .frames
            .last()
            .map(|f| f.node.clone())
            .unwrap_or_default();
        let pos = self.s.position().unwrap_or_default();
        // An environment failure the step waits out: no edge, no retry, no attempt used. The
        // pending run stays so the wake-up continues it.
        if let Some(env) = &f.environment
            && let Some(rule) = st.platform_wait.iter().find(|r| r.kind == env.kind)
        {
            self.s.pending = Some(Pending::Step { run_id, attempt });
            self.s.last = json!({
                "node": pos, "exit": f.exit, "outputs": f.outputs, "result": f.result,
                "git": f.git, "environment": f.environment, "missing_outputs": [],
            });
            // The agent's own report is advisory: it only adds detail, never routes.
            let detail = match agent_environment_detail(&f.result) {
                Some(agent) => format!("{}; agent: {agent}", env.detail),
                None => env.detail.clone(),
            };
            self.s
                .notes
                .push(format!("{pos}: platform wait on `{}` ({detail})", env.kind));
            self.s.platform_wait = Some(PlatformWait {
                kind: env.kind.clone(),
                detail,
                until: rule.until.clone(),
                waits: self.s.platform_waits,
            });
            self.s.status = Status::Running;
            self.project();
            return;
        }
        // Retry before recording anything.
        if let Some(r) = &st.retry {
            let retried = if r.on.is_empty() {
                f.exit == "failure"
            } else {
                r.on.contains(&f.exit)
            };
            if retried && attempt <= r.max {
                self.s.notes.push(format!(
                    "{pos}: attempt {attempt} exited `{}`; retrying",
                    f.exit
                ));
                self.start_step(st, attempt + 1);
                return;
            }
        }
        let mut missing = Vec::new();
        for (name, decl) in &st.outputs {
            match f.outputs.get(name) {
                Some(v) => {
                    self.s.doc.outputs.insert(name.clone(), v.clone());
                    if let Some(title) = &decl.artifact {
                        let format = decl
                            .format
                            .clone()
                            .unwrap_or_else(|| guess_format(&decl.file));
                        let content = match v {
                            Value::String(s) => s.clone(),
                            other => serde_json::to_string_pretty(other).unwrap_or_default(),
                        };
                        self.s.doc.artifacts.retain(|a| a.title != *title);
                        self.s.doc.artifacts.push(Artifact {
                            title: title.clone(),
                            format,
                            content,
                            node: pos.clone(),
                        });
                    }
                }
                None if !decl.optional => missing.push(name.clone()),
                None => {}
            }
        }
        if let (Some(spec), Some(r)) = (&st.result, &f.result) {
            let name = spec.as_.clone().unwrap_or_else(|| node_id.clone());
            self.s.doc.outputs.insert(name, r.clone());
        }
        if let Some(g) = &f.git {
            self.s.doc.facts.git = Some(g.clone());
        }
        self.s.last = json!({
            "node": pos, "exit": f.exit, "outputs": f.outputs, "result": f.result,
            "git": f.git, "environment": f.environment, "missing_outputs": missing,
        });
        self.after_finish();
    }

    /// Follows the current node's edges with `last` set.
    fn after_finish(&mut self) {
        let (Some(node), Some(phase)) = (self.node(), self.phase()) else {
            return;
        };
        let ctx = self.ctx(&phase.params, &[]);
        self.s.status = Status::Running;
        match follow(&node.next, &ctx) {
            Ok(Some(to)) => {
                let f = self.leave(to, &[]);
                self.run(f)
            }
            Ok(None) => {
                let here = self.s.frames.last().map(|f| f.node.clone());
                self.pause("no edge matched".into(), here);
            }
            Err(e) => {
                let here = self.s.frames.last().map(|f| f.node.clone());
                self.pause(format!("condition error: {e}"), here);
            }
        }
    }

    // ------------------------------------------------------------ movement

    fn run(&mut self, mut flow: Flow) {
        for _ in 0..MAX_HOPS {
            flow = match flow {
                Flow::Stop => return,
                Flow::Enter(n) => self.enter(n),
                Flow::Exit(e) => self.exit_phase(e),
            };
        }
        self.pause(
            format!("more than {MAX_HOPS} engine transitions without a wait or step"),
            None,
        );
    }

    fn enter_phase(&mut self, id: &str, depth: usize) {
        if depth > self.res.phases.len() + 1 {
            self.end(EXIT_LIMIT);
            return;
        }
        let Some(p) = self.res.phases.iter().find(|p| p.id == id) else {
            self.pause(format!("unknown phase `{id}`"), None);
            return;
        };
        let n = self.s.entries.entry(id.to_string()).or_insert(0);
        *n += 1;
        if *n > p.max_entries {
            self.s.notes.push(format!(
                "phase `{id}` entered more than {} times; exit `{}`",
                p.max_entries, p.on_limit
            ));
            match p
                .routes
                .get(&p.on_limit)
                .cloned()
                .unwrap_or(Route::End(p.on_limit.clone()))
            {
                Route::End(s) => self.end(&s),
                Route::Phase(next) if next == id => self.end(&p.on_limit),
                Route::Phase(next) => self.enter_phase(&next, depth + 1),
            }
            return;
        }
        self.s.frames = vec![Frame {
            id: id.into(),
            node: p.start.clone(),
            visits: BTreeMap::new(),
        }];
        let start = p.start.clone();
        self.run(Flow::Enter(start));
    }

    fn enter(&mut self, id: String) -> Flow {
        let Some(phase) = self.phase() else {
            return Flow::Stop;
        };
        let Some(node) = phase.nodes.get(&id) else {
            self.pause(format!("unknown node `{id}`"), None);
            return Flow::Stop;
        };
        let frame = self.s.frames.last_mut().expect("in a phase");
        frame.node = id.clone();
        let v = frame.visits.entry(id.clone()).or_insert(0);
        *v += 1;
        if *v > node.max_visits {
            let pos = self.s.position().unwrap_or_default();
            self.s.notes.push(format!(
                "{pos}: visited more than {} times",
                node.max_visits
            ));
            return match &node.on_limit {
                Some(n) if *n != id => Flow::Enter(n.clone()),
                _ => Flow::Exit(EXIT_LIMIT.into()),
            };
        }
        self.s.path.push(self.s.position().unwrap_or_default());
        if let Err(e) = self.apply_set(&node.set, &phase.params, &[]) {
            self.pause(e, Some(id));
            return Flow::Stop;
        }
        match &node.kind {
            RKind::End(status) => Flow::Exit(status.clone()),
            RKind::Decide => {
                let ctx = self.ctx(&phase.params, &[]);
                match follow(&node.next, &ctx) {
                    Ok(Some(to)) => self.leave(to, &[]),
                    Ok(None) => {
                        self.pause("no edge matched".into(), Some(id));
                        Flow::Stop
                    }
                    Err(e) => {
                        self.pause(format!("condition error: {e}"), Some(id));
                        Flow::Stop
                    }
                }
            }
            RKind::Phase(inner) => {
                self.s.frames.push(Frame {
                    id: id.clone(),
                    node: inner.start.clone(),
                    visits: BTreeMap::new(),
                });
                Flow::Enter(inner.start.clone())
            }
            RKind::Step(st) => {
                self.start_step(st, 1);
                Flow::Stop
            }
            RKind::Action(a) => {
                let ctx = self.ctx(&phase.params, &[]);
                let args = match template::render_value(&Value::Object(a.args.clone()), &ctx) {
                    Ok(Value::Object(o)) => o,
                    Ok(_) => Map::new(),
                    Err(e) => {
                        self.pause(format!("action `{}`: {}", a.name, e.message), Some(id));
                        return Flow::Stop;
                    }
                };
                if let Some(rel) = &a.relate
                    && let Some(targets) = &rel.targets
                {
                    match template::eval(targets, &ctx) {
                        Ok(Value::Array(targets)) => {
                            for t in &targets {
                                self.s.doc.relations.push(Relation {
                                    kind: rel.kind.clone(),
                                    target: t.clone(),
                                });
                            }
                            self.fx.push(Effect::CreateRelations {
                                kind: rel.kind.clone(),
                                targets,
                                created: false,
                            });
                        }
                        Ok(Value::Null) => {}
                        Ok(other) => {
                            self.pause(
                                format!("relate.targets must be a list, got {other}"),
                                Some(id),
                            );
                            return Flow::Stop;
                        }
                        Err(e) => {
                            self.pause(format!("relate.targets: {}", e.message), Some(id));
                            return Flow::Stop;
                        }
                    }
                }
                let call_id = self.next_id("call");
                self.s.pending = Some(Pending::Action {
                    call_id: call_id.clone(),
                });
                self.s.status = Status::Running;
                self.fx.push(Effect::CallAction {
                    call_id,
                    name: a.name.clone(),
                    args,
                });
                self.project();
                Flow::Stop
            }
            RKind::Wait(w) => {
                for f in &w.refresh {
                    self.fx.push(Effect::RefreshFact { fact: f.clone() });
                }
                if let Some(t) = &w.timer {
                    let seconds = match &t.seconds {
                        NumField::Int(n) => Ok(*n),
                        NumField::Value(s) => template::render(s, &self.ctx(&phase.params, &[]))
                            .map_err(|e| e.message)
                            .and_then(|r| {
                                parse_duration(&r).ok_or_else(|| {
                                    format!("`{r}` is not a number of seconds or a duration")
                                })
                            }),
                        NumField::Param { param } => Err(format!("unresolved param `{param}`")),
                    };
                    let seconds = match seconds {
                        Ok(s) => s,
                        Err(e) => {
                            self.pause(format!("timer.seconds: {e}"), Some(id));
                            return Flow::Stop;
                        }
                    };
                    let timer_id = self.next_id("timer");
                    self.s.timer = Some(timer_id.clone());
                    self.fx.push(Effect::SetTimer { timer_id, seconds });
                }
                self.s.status = Status::Waiting;
                self.project();
                Flow::Stop
            }
        }
    }

    fn exit_phase(&mut self, exit: String) -> Flow {
        let done = self.s.frames.pop();
        if self.s.frames.is_empty() {
            let Some(done) = done else { return Flow::Stop };
            let p = self.res.phases.iter().find(|p| p.id == done.id);
            let route = p
                .and_then(|p| p.routes.get(&exit).cloned())
                .unwrap_or(Route::End(exit.clone()));
            match route {
                Route::End(s) => {
                    // Keep the last position for the projection and history.
                    self.s.frames.push(done);
                    self.end(&s);
                }
                Route::Phase(id) => self.enter_phase(&id, 0),
            }
            return Flow::Stop;
        }
        // A nested phase finished: follow the phase node's edges with `exit`.
        let (Some(node), Some(phase)) = (self.node(), self.phase()) else {
            return Flow::Stop;
        };
        let ctx = self.ctx(&phase.params, &[("exit", json!(exit))]);
        self.s.last = json!({"node": self.s.position(), "exit": exit, "environment": null});
        match follow(&node.next, &ctx) {
            Ok(Some(to)) => self.leave(to, &[("exit", json!(exit))]),
            Ok(None) => {
                self.pause(format!("no edge matched exit `{exit}`"), None);
                Flow::Stop
            }
            Err(e) => {
                self.pause(format!("condition error: {e}"), None);
                Flow::Stop
            }
        }
    }

    fn start_step(&mut self, st: &RStep, attempt: u32) {
        self.send_step(st, attempt, None);
    }

    /// Renders and emits the step. `again` is the run to continue (a platform wait ended): same
    /// run id, so the sandbox resumes the session instead of starting a new run.
    fn send_step(&mut self, st: &RStep, attempt: u32, again: Option<String>) {
        let here = self.s.frames.last().map(|f| f.node.clone());
        let mut extra: Vec<(String, Value)> = Vec::new();
        for (name, input) in &st.inputs {
            let v = match input.from.strip_prefix("params.") {
                Some(p) => st.params.get(p).cloned().filter(|v| !v.is_null()),
                None => self.s.doc.lookup(&input.from),
            }
            .or_else(|| input.default.clone());
            match v {
                Some(v) if !type_ok(input.ty, &v) => {
                    self.pause(
                        format!("input `{name}` ({}) has the wrong type: {v}", input.from),
                        here,
                    );
                    return;
                }
                Some(v) => extra.push((name.clone(), v)),
                None if input.required => {
                    self.pause(
                        format!("missing required input `{name}` ({})", input.from),
                        here,
                    );
                    return;
                }
                None => {} // left undefined: renders empty, tests false
            }
        }
        let refs: Vec<(&str, Value)> = extra.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let ctx = self.ctx(&st.params, &refs);
        let render = |s: &str| template::render(s, &ctx).map_err(|e| e.message);
        let exec = match &st.exec {
            Exec::Agent {
                runtime,
                model,
                prompt,
            } => render(prompt).and_then(|p| {
                let model = match model {
                    Some(m) if template::is_template(m) => Some(render(m)?.trim().to_string()),
                    m => m.clone(),
                };
                Ok(RunExec::Agent {
                    runtime: runtime.clone(),
                    model: model.filter(|m| !m.is_empty()),
                    prompt: p,
                })
            }),
            Exec::Command { run, tools } => render(run).map(|r| RunExec::Command {
                run: r,
                tools: tools.clone(),
            }),
        };
        let workspaces: Result<Vec<_>, _> = st
            .workspaces
            .iter()
            .map(|w| {
                let opt = |v: &Option<String>| -> Result<Option<String>, String> {
                    v.as_deref()
                        .map(|s| render(s).map(|r| r.trim().to_string()))
                        .transpose()
                };
                Ok(RunWorkspace {
                    repo: render(&w.repo)?,
                    mode: w.mode,
                    branch: opt(&w.branch)?,
                    base: opt(&w.base)?,
                })
            })
            .collect();
        let concurrency = match &st.concurrency {
            Some(c) => render(&c.group).map(|group| Some(Concurrency { group, max: c.max })),
            None => Ok(None),
        };
        let (exec, workspaces, concurrency) = match (exec, workspaces, concurrency) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                self.pause(format!("template error in step `{}`: {e}", st.name), here);
                return;
            }
        };
        // Run ids are global (step_runs, session events, the Project's sandbox): qualify them
        // with the task so two tasks never share one.
        let resume = if again.is_some() {
            self.s.platform_waits
        } else {
            0
        };
        let run_id = match (again, self.s.doc.task.id.as_str()) {
            (Some(r), _) => r,
            (None, "") => self.next_id("run"),
            (None, t) => self.next_id(&format!("task-{t}-run")),
        };
        if matches!(exec, RunExec::Agent { .. }) {
            self.s.last_agent_run = Some(run_id.clone());
        }
        self.s.pending = Some(Pending::Step {
            run_id: run_id.clone(),
            attempt,
        });
        self.s.status = Status::Running;
        self.fx.push(Effect::RunStep(Box::new(RunStep {
            run_id,
            attempt,
            resume,
            node: self.s.position().unwrap_or_default(),
            step: st.name.clone(),
            exec,
            workspaces,
            github: st.github.clone(),
            secrets: st.secrets.clone(),
            priority: st.priority.unwrap_or(0),
            concurrency,
            outputs: st.outputs.clone(),
            result: st.result.clone(),
        })));
        self.project();
    }

    fn end(&mut self, status: &str) {
        self.s.status = Status::Ended {
            status: status.into(),
        };
        self.s.pending = None;
        self.s.platform_wait = None;
        self.s.timer = None;
        self.project();
    }

    fn pause(&mut self, reason: String, resume_at: Option<String>) {
        let previous = match &self.s.status {
            Status::Paused { previous, .. } => previous.clone(),
            s => Box::new(s.clone()),
        };
        self.s.notes.push(format!("paused: {reason}"));
        self.s.status = Status::Paused {
            reason,
            previous,
            resume_at,
        };
        self.project();
    }

    // ------------------------------------------------------------ projection

    fn project(&mut self) {
        let p = self.build_projection();
        self.fx.push(Effect::UpdateProjection(Box::new(p)));
    }

    fn build_projection(&self) -> Projection {
        let board = &self.res.board.defaults;
        let phase = self
            .s
            .frames
            .iter()
            .map(|f| f.id.clone())
            .collect::<Vec<_>>()
            .join("/");
        let node = self
            .s
            .frames
            .last()
            .map(|f| f.node.clone())
            .unwrap_or_default();
        let wait: Option<&Wait> = match self.node().map(|n| &n.kind) {
            Some(RKind::Wait(w)) => Some(w),
            _ => None,
        };
        let mut p = Projection {
            status: self.s.status.label().into(),
            end_status: None,
            phase,
            node,
            column: None,
            waiting_on: None,
            reason: None,
            text: None,
            actions: Vec::new(),
            commands: accepted_commands(self.res, &self.s),
            artifacts: self.s.doc.artifacts.clone(),
            needs_you: false,
        };
        match &self.s.status {
            Status::Ended { status } => {
                p.end_status = Some(status.clone());
                p.column = board.ended.clone();
            }
            Status::Paused { reason, .. } => {
                p.reason = Some("paused".into());
                p.text = Some(reason.clone());
                p.waiting_on = Some(WaitingOn::Human);
                p.needs_you = true;
                p.column = board.paused.clone();
            }
            Status::Waiting => {
                if let Some(w) = wait {
                    let writer = w.from == Some(Sender::Writer)
                        || w.events.iter().any(|m| {
                            matches!(
                                m,
                                Matcher::Full(MatcherSpec {
                                    from: Some(Sender::Writer),
                                    ..
                                })
                            )
                        });
                    let on = w.waiting_on.unwrap_or(if !w.actions.is_empty() || writer {
                        WaitingOn::Human
                    } else {
                        WaitingOn::External
                    });
                    p.waiting_on = Some(on);
                    p.needs_you = on == WaitingOn::Human;
                    let params = self.phase().map(|p| p.params.clone()).unwrap_or_default();
                    let ctx = self.ctx(&params, &[]);
                    let field = |t: &TextField| -> Result<String, String> {
                        let s = t.as_str();
                        if template::is_template(s) {
                            template::render(s, &ctx).map_err(|e| e.message)
                        } else {
                            Ok(s.to_string())
                        }
                    };
                    let short = |t: &TextField| field(t).ok().map(|s| s.trim().to_string());
                    p.reason = w.reason.as_ref().and_then(short).filter(|s| !s.is_empty());
                    p.text = w.text.as_ref().map(|t| {
                        template::render(t.as_str(), &ctx).unwrap_or_else(|e| {
                            format!("{} (template error: {})", t.as_str(), e.message)
                        })
                    });
                    p.actions = w
                        .actions
                        .iter()
                        .filter_map(|a| {
                            Some(UiAction {
                                label: short(&a.label)?,
                                command: short(&a.command)?,
                            })
                        })
                        .filter(|a| !a.command.is_empty())
                        .collect();
                    let col = w.column.as_ref().and_then(short).filter(|s| !s.is_empty());
                    p.column = col.or_else(|| match on {
                        WaitingOn::Human => board.waiting_human.clone(),
                        WaitingOn::Agent => board.waiting_agent.clone(),
                        WaitingOn::External => board.waiting_external.clone(),
                    });
                }
            }
            Status::Running if self.s.platform_wait.is_some() => {
                let w = self.s.platform_wait.as_ref().expect("checked");
                p.waiting_on = Some(WaitingOn::External);
                p.reason = Some("platform_wait".into());
                p.text = Some(format!(
                    "Waiting for the sandbox: {} ({})",
                    w.kind, w.detail
                ));
                p.column = board.waiting_external.clone();
            }
            Status::Running | Status::New => {
                p.waiting_on = Some(WaitingOn::Agent);
                p.column = board.running.clone();
            }
        }
        p
    }
}

fn follow(edges: &[crate::resolve::REdge], ctx: &Value) -> Result<Option<String>, String> {
    for e in edges {
        match &e.cond {
            None => return Ok(Some(e.to.clone())),
            Some(c) => {
                if template::truthy(c, ctx).map_err(|e| e.message)? {
                    return Ok(Some(e.to.clone()));
                }
            }
        }
    }
    Ok(None)
}

fn type_ok(t: crate::format::InputType, v: &Value) -> bool {
    use crate::format::InputType::*;
    match t {
        Any => true,
        String => v.is_string(),
        Integer => v.is_i64() || v.is_u64(),
        Number => v.is_number(),
        Boolean => v.is_boolean(),
        Object => v.is_object(),
        List => v.is_array(),
    }
}

fn guess_format(file: &str) -> String {
    match file.rsplit('.').next() {
        Some("md") => "markdown",
        Some("json") => "json",
        _ => "text",
    }
    .into()
}

// ---------------------------------------------------------------- upgrade

/// Outcome of offering a task a newly resolved definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Reresolution {
    /// The task continues on the new definition from the same node.
    Upgraded,
    /// A step is running; it finishes on the definition it started with.
    Deferred,
    /// The position does not exist in the new definition.
    Pinned { reason: String },
}

/// The auto-upgrade hook (ADR-0022 §4): called at each wait and before each
/// step with the re-resolved definition. On `Upgraded` the engine stores the
/// new definition; on `Pinned` it keeps the old one.
pub fn reresolve(new: &Resolved, state: &TaskState) -> (TaskState, Reresolution) {
    let mut s = state.clone();
    if matches!(
        s.pending,
        Some(Pending::Step { .. }) | Some(Pending::Action { .. })
    ) {
        return (s, Reresolution::Deferred);
    }
    if matches!(s.status, Status::Ended { .. } | Status::New) {
        s.pinned = None;
        return (s, Reresolution::Upgraded);
    }
    let pos = s.position().unwrap_or_default();
    let ok = (1..=s.frames.len()).all(|depth| {
        let path: Vec<String> = s.frames[..depth].iter().map(|f| f.id.clone()).collect();
        new.phase(&path)
            .is_some_and(|p| p.nodes.contains_key(&s.frames[depth - 1].node))
    }) && match (
        s.status == Status::Waiting,
        new.phase(&s.phase_path()).and_then(|p| {
            p.nodes
                .get(&s.frames.last().map(|f| f.node.clone()).unwrap_or_default())
        }),
    ) {
        (true, Some(n)) => matches!(n.kind, RKind::Wait(_)),
        (_, Some(_)) => true,
        _ => false,
    };
    if ok {
        if s.pinned.take().is_some() {
            s.notes
                .push(format!("unpinned: `{pos}` exists in {}", new.entry.sha));
        }
        (s, Reresolution::Upgraded)
    } else {
        let reason = format!(
            "`{pos}` does not exist in {}@{}",
            new.entry.path, new.entry.sha
        );
        if s.pinned.as_deref() != Some(reason.as_str()) {
            s.notes.push(format!("pinned: {reason}"));
        }
        s.pinned = Some(reason.clone());
        (s, Reresolution::Pinned { reason })
    }
}
