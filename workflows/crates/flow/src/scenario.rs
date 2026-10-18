//! YAML scenario tests (ADR-0022 §13): a start event, further events,
//! stubbed step and action results, and expectations on the path, the
//! final status and the projection.

use crate::document::TaskDocument;
use crate::events::Event;
use crate::format::WaitingOn;
use crate::interp::{Effect, Projection, StepFinished, TaskEvent, TaskState, step};
use crate::resolve::Resolved;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// A scenario file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Workflow file path in the repository under test.
    pub workflow: String,
    /// Workflow params, as a binding's `with` would give them.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    /// Initial task document (subjects, task info, `vars`, `facts`,
    /// `instance`, …).
    #[serde(default)]
    pub document: TaskDocument,
    /// The event that starts the task.
    pub start: Event,
    /// Step results by `phase/…/node` (or bare node id), used in order; the
    /// last one repeats.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stubs: BTreeMap<String, Vec<Stub>>,
    /// Action results by action name (default `{ok: true}`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, Vec<ActionStub>>,
    /// Inputs after the start event, each followed by an optional expectation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Item>,
    /// Expectations after the last input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expect>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stub {
    #[serde(default = "success")]
    pub exit: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub outputs: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<crate::document::GitFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<crate::interp::StepEnvironment>,
}

fn success() -> String {
    "success".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionStub {
    #[serde(default = "yes")]
    pub ok: bool,
    #[serde(default)]
    pub result: Value,
}

fn yes() -> bool {
    true
}

/// One input. Exactly one of the input fields is set (an `expect`-only item checks the current state).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Item {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Event>,
    /// A refreshed fact: `{fact: ci, value: {…}}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<FactIn>,
    /// Fires the current wait's timer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timer: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stall: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resume: bool,
    /// The pending platform wait's condition holds: the step resumes on the same run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub platform_cleared: bool,
    /// Finishes the pending step with this result (instead of a stub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_finished: Option<Stub>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expect>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactIn {
    pub fact: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// `running`, `waiting`, `paused`, or `ended:<status>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Current `phase/…/node`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Every node entered so far, exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ExpectProjection>,
    /// Effect kinds emitted while handling this input, in order
    /// (`run_step`, `steer`, `stop_step`, `call_action`, …; projection updates are left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<String>>,
    /// Outputs that must be present with these values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Map<String, Value>>,
    /// Task variables that must have these values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vars: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpectProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<WaitingOn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_you: Option<bool>,
    /// Button labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
    /// Names of the commands that apply, in order (`retry`, `cancel`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    /// Artifact titles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<String>>,
}

/// A failed expectation: value path in the scenario file plus message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub path: String,
    pub message: String,
}

struct Runner<'a> {
    res: &'a Resolved,
    sc: &'a Scenario,
    state: TaskState,
    projection: Option<Projection>,
    used: BTreeMap<String, usize>,
    effects: Vec<String>,
    failures: Vec<Failure>,
}

/// Runs a scenario against a resolved workflow; returns the failures.
pub fn run(res: &Resolved, sc: &Scenario) -> Vec<Failure> {
    let mut r = Runner {
        res,
        sc,
        state: TaskState::new(sc.document.clone()),
        projection: None,
        used: BTreeMap::new(),
        effects: Vec::new(),
        failures: Vec::new(),
    };
    r.feed(TaskEvent::Start {
        event: sc.start.clone(),
    });
    for (i, item) in sc.events.iter().enumerate() {
        r.effects.clear();
        let base = format!("events[{i}]");
        let input = if let Some(e) = &item.event {
            Some(TaskEvent::Event { event: e.clone() })
        } else if let Some(f) = &item.fact {
            Some(TaskEvent::FactUpdated {
                fact: f.fact.clone(),
                value: f.value.clone(),
            })
        } else if item.timer {
            match r.state.timer.clone() {
                Some(timer_id) => Some(TaskEvent::Timer { timer_id }),
                None => {
                    r.fail(&base, "no timer is set");
                    None
                }
            }
        } else if item.stall {
            Some(TaskEvent::StallDetected { reason: None })
        } else if item.resume {
            Some(TaskEvent::Resume)
        } else if item.platform_cleared {
            match (&r.state.pending, &r.state.platform_wait) {
                (Some(crate::interp::Pending::Step { run_id, .. }), Some(w)) => {
                    Some(TaskEvent::PlatformCleared {
                        run_id: run_id.clone(),
                        wait: w.waits,
                    })
                }
                _ => {
                    r.fail(&base, "no platform wait");
                    None
                }
            }
        } else if let Some(s) = &item.step_finished {
            match &r.state.pending {
                Some(crate::interp::Pending::Step { run_id, .. }) => {
                    Some(finished(run_id.clone(), s))
                }
                _ => {
                    r.fail(&base, "no step is running");
                    None
                }
            }
        } else {
            None
        };
        if let Some(ev) = input {
            r.feed(ev);
        }
        if let Some(x) = &item.expect {
            r.check(&format!("{base}.expect"), x);
        }
    }
    if let Some(x) = &sc.expect {
        r.check("expect", x);
    }
    r.failures
}

fn finished(run_id: String, s: &Stub) -> TaskEvent {
    TaskEvent::StepFinished(StepFinished {
        run_id,
        exit: s.exit.clone(),
        outputs: s.outputs.clone(),
        result: s.result.clone(),
        git: s.git.clone(),
        environment: s.environment.clone(),
    })
}

impl Runner<'_> {
    fn fail(&mut self, path: &str, msg: impl Into<String>) {
        self.failures.push(Failure {
            path: path.into(),
            message: msg.into(),
        });
    }

    /// Applies an event, then answers stubbed steps and actions until quiet.
    fn feed(&mut self, first: TaskEvent) {
        let mut queue = vec![first];
        let mut guard = 0;
        while let Some(ev) = queue.pop() {
            guard += 1;
            if guard > 10_000 {
                self.fail("", "scenario did not settle after 10000 events");
                return;
            }
            let (s, fx) = step(self.res, &self.state, &ev);
            self.state = s;
            for e in fx {
                match e {
                    Effect::UpdateProjection(p) => self.projection = Some(*p),
                    Effect::RunStep(rs) => {
                        self.effects.push("run_step".into());
                        let bare = rs.node.rsplit('/').next().unwrap_or_default().to_string();
                        let key = if self.sc.stubs.contains_key(&rs.node) {
                            Some(rs.node.clone())
                        } else {
                            self.sc.stubs.contains_key(&bare).then_some(bare)
                        };
                        if let Some(k) = key {
                            let list = &self.sc.stubs[&k];
                            let n = self.used.entry(k).or_insert(0);
                            let stub = &list[(*n).min(list.len() - 1)];
                            *n += 1;
                            queue.insert(0, finished(rs.run_id.clone(), stub));
                        }
                    }
                    Effect::CallAction { call_id, name, .. } => {
                        self.effects.push("call_action".into());
                        let k = format!("action:{name}");
                        let n = self.used.entry(k).or_insert(0);
                        let (ok, result) = match self.sc.actions.get(&name) {
                            Some(list) if !list.is_empty() => {
                                let a = &list[(*n).min(list.len() - 1)];
                                (a.ok, a.result.clone())
                            }
                            _ => (true, Value::Null),
                        };
                        *n += 1;
                        queue.insert(
                            0,
                            TaskEvent::ActionFinished {
                                call_id,
                                ok,
                                result,
                            },
                        );
                    }
                    other => {
                        let v = serde_json::to_value(&other).unwrap_or_default();
                        self.effects
                            .push(v["effect"].as_str().unwrap_or_default().to_string());
                    }
                }
            }
        }
    }

    fn status(&self) -> String {
        match &self.state.status {
            crate::interp::Status::Ended { status } => format!("ended:{status}"),
            s => s.label().to_string(),
        }
    }

    fn check(&mut self, base: &str, x: &Expect) {
        if let Some(want) = &x.status {
            let got = self.status();
            if &got != want {
                let why = match &self.state.status {
                    crate::interp::Status::Paused { reason, .. } => format!(" ({reason})"),
                    _ => String::new(),
                };
                self.fail(
                    &format!("{base}.status"),
                    format!("status is `{got}`{why}, expected `{want}`"),
                );
            }
        }
        if let Some(want) = &x.at {
            let got = self.state.position().unwrap_or_default();
            if &got != want {
                self.fail(
                    &format!("{base}.at"),
                    format!("at `{got}`, expected `{want}`"),
                );
            }
        }
        if let Some(want) = &x.path
            && *want != self.state.path
        {
            let got = self.state.path.join(", ");
            self.fail(&format!("{base}.path"), format!("path was [{got}]"));
        }
        if let Some(want) = &x.effects
            && *want != self.effects
        {
            let got = self.effects.join(", ");
            self.fail(&format!("{base}.effects"), format!("effects were [{got}]"));
        }
        if let Some(want) = &x.outputs {
            for (k, v) in want {
                if self.state.doc.outputs.get(k) != Some(v) {
                    let got = self
                        .state
                        .doc
                        .outputs
                        .get(k)
                        .map(Value::to_string)
                        .unwrap_or_else(|| "missing".into());
                    self.fail(
                        &format!("{base}.outputs.{k}"),
                        format!("output `{k}` is {got}"),
                    );
                }
            }
        }
        if let Some(want) = &x.vars {
            for (k, v) in want {
                if self.state.doc.vars.get(k) != Some(v) {
                    let got = self
                        .state
                        .doc
                        .vars
                        .get(k)
                        .map(Value::to_string)
                        .unwrap_or_else(|| "missing".into());
                    self.fail(&format!("{base}.vars.{k}"), format!("var `{k}` is {got}"));
                }
            }
        }
        if let Some(want) = &x.projection {
            let Some(p) = self.projection.clone() else {
                self.fail(&format!("{base}.projection"), "no projection");
                return;
            };
            let mut cmp = |field: &str, got: String, want: Option<String>| {
                if let Some(w) = want
                    && got != w
                {
                    self.fail(
                        &format!("{base}.projection.{field}"),
                        format!("{field} is `{got}`, expected `{w}`"),
                    );
                }
            };
            cmp(
                "column",
                p.column.clone().unwrap_or_default(),
                want.column.clone(),
            );
            cmp(
                "reason",
                p.reason.clone().unwrap_or_default(),
                want.reason.clone(),
            );
            cmp(
                "text",
                p.text.clone().unwrap_or_default(),
                want.text.clone(),
            );
            cmp(
                "needs_you",
                p.needs_you.to_string(),
                want.needs_you.map(|b| b.to_string()),
            );
            let on = |w: Option<WaitingOn>| {
                w.map(|w| {
                    serde_json::to_value(w)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                })
            };
            cmp(
                "waiting_on",
                on(p.waiting_on).unwrap_or_default(),
                on(want.waiting_on),
            );
            cmp(
                "actions",
                p.actions
                    .iter()
                    .map(|a| a.label.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                want.actions.as_ref().map(|a| a.join(", ")),
            );
            cmp(
                "commands",
                p.commands
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                want.commands.as_ref().map(|a| a.join(", ")),
            );
            cmp(
                "artifacts",
                p.artifacts
                    .iter()
                    .map(|a| a.title.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                want.artifacts.as_ref().map(|a| a.join(", ")),
            );
        }
    }
}
