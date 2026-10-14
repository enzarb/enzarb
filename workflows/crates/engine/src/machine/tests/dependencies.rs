//! Dependencies gate the merge and re-evaluate when they change.

use super::*;
use crate::machine::change_request::{CheckStatus, readiness};

fn ready_data() -> CrData {
    CrData {
        last_review: "approve".into(),
        ci: CiState::Success,
        ci_sha: "sha1".into(),
        mergeable: Some(true),
        ..cr_data()
    }
}

fn cr_with(phase: CrPhase, data: CrData) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState { phase, data })
}

fn waiting(n: u64) -> OpenDependency {
    OpenDependency {
        key: format!("github:enzarb/enzarb-ui#{n}"),
        label: format!("#{n}"),
        url: format!("https://github.com/enzarb/enzarb-ui/issues/{n}"),
        problem: String::new(),
    }
}

fn changed(id: &str) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::DependenciesChanged,
    }
}

fn with_deps(deps: Vec<OpenDependency>) -> Ctx {
    Ctx {
        open_dependencies: deps,
        ..ctx_for(43, Permission::None)
    }
}

#[test]
fn open_dependency_keeps_an_approved_change_request_from_ready_to_merge() {
    let s = cr_with(CrPhase::ReadyToMerge, ready_data());
    let t = transition(&with_deps(vec![waiting(12)]), &s, &changed("e1"));
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.open_dependencies, [waiting(12)]);
    let r = readiness(&d);
    assert_eq!(r.dependencies.status, CheckStatus::Pending);
    assert_eq!(r.dependencies.detail, "waiting on #12");
    // Nothing for the connector to answer: the gate never asks.
    assert!(
        !kinds(&t).contains(&JobKind::CheckMergeReadiness),
        "{:?}",
        kinds(&t)
    );
}

#[test]
fn completed_dependency_continues_the_dependent_without_a_human() {
    let s = cr_with(
        CrPhase::AwaitingChanges,
        CrData {
            open_dependencies: vec![waiting(12)],
            ..ready_data()
        },
    );
    let t = transition(&with_deps(Vec::new()), &s, &changed("e2"));
    let d = assert_cr(&t, CrPhase::ReadyToMerge);
    assert!(d.open_dependencies.is_empty());
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
}

#[test]
fn unchanged_dependencies_are_a_duplicate() {
    let s = cr_with(
        CrPhase::AwaitingChanges,
        CrData {
            open_dependencies: vec![waiting(12)],
            ..ready_data()
        },
    );
    let t = transition(&with_deps(vec![waiting(12)]), &s, &changed("e3"));
    rejected(&t, &s);
}

#[test]
fn any_event_sees_fresh_dependencies_and_the_merge_is_refused() {
    let s = cr_with(CrPhase::ReadyToMerge, ready_data());
    let ev = ui("m", "maintainer", UiActionKind::Merge);
    let t = transition(&with_deps(vec![waiting(12)]), &s, &ev);
    assert!(!t.accepted, "{}", t.note);
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
    // Control: without the dependency the same merge starts.
    let t = transition(&ctx_for(43, Permission::Write), &s, &ev);
    assert_cr(&t, CrPhase::Merging);
}

#[test]
fn closed_without_completing_and_cycles_fail_the_check() {
    let closed = OpenDependency {
        problem: "dependency #12 closed without completing".into(),
        ..waiting(12)
    };
    let cycle = OpenDependency {
        key: String::new(),
        label: "dependency cycle".into(),
        url: String::new(),
        problem: "dependency cycle: #43 → #7 → #43".into(),
    };
    for (dep, text) in [
        (closed, "dependency #12 closed without completing"),
        (cycle, "dependency cycle: #43 → #7 → #43"),
    ] {
        let d = CrData {
            open_dependencies: vec![dep],
            ..ready_data()
        };
        let r = readiness(&d);
        assert_eq!(r.dependencies.status, CheckStatus::Failed);
        assert_eq!(r.dependencies.detail, text);
        let s = cr_with(CrPhase::ReadyToMerge, ready_data());
        let t = transition(&with_deps(d.open_dependencies.clone()), &s, &changed("c"));
        assert_cr(&t, CrPhase::AwaitingChanges);
    }
}

#[test]
fn several_dependencies_are_listed() {
    let d = CrData {
        open_dependencies: vec![waiting(1), waiting(2)],
        ..ready_data()
    };
    assert_eq!(readiness(&d).dependencies.detail, "waiting on #1, #2");
}

#[test]
fn finished_change_requests_ignore_dependency_changes() {
    for phase in [CrPhase::Merged, CrPhase::Closed, CrPhase::Cancelled] {
        let s = cr_with(phase, ready_data());
        let t = transition(&with_deps(vec![waiting(12)]), &s, &changed("f"));
        assert!(!t.accepted);
    }
}

#[test]
fn issues_reject_the_event() {
    let s = WorkflowState::Issue(IssueState {
        phase: IssuePhase::Planning,
        data: IssueData::default(),
    });
    let t = run(Permission::None, &s, &changed("i"));
    rejected(&t, &s);
}

#[test]
fn dependencies_event_parses_from_a_row() {
    let ev = EngineEvent::from_row("1", "DependenciesChanged", &json!({})).unwrap();
    assert_eq!(ev.payload, EventPayload::DependenciesChanged);
}
