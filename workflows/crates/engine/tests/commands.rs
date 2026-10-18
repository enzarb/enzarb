//! The command model against the real standard library (snapshot in `tests/stdlib`): every
//! wait of every workflow accepts `/retry` and `/cancel`, and the library lints clean.

use std::path::PathBuf;

use enzarb_flow::events::{Actor, Comment, Event, Payload};
use enzarb_flow::interp::{Effect, Frame, Status, TaskEvent, TaskState, step};
use enzarb_flow::lint::FsLoader;
use enzarb_flow::resolve::{Entry, Options, RKind, RPhase, Resolved, resolve};

const LIB: &str = "enzarb/workflows";

fn load(workflow: &str) -> Resolved {
    let mut loader = FsLoader::default();
    loader.roots.insert(
        LIB.into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/stdlib"),
    );
    let entry = Entry {
        repo: LIB.into(),
        rref: "local".into(),
        path: format!("workflows/{workflow}.yaml"),
    };
    match resolve(&entry, &loader, &Options::default()) {
        Ok((r, diags)) => {
            assert!(diags.is_empty(), "{workflow}: {diags:?}");
            r
        }
        Err(d) => panic!("{workflow}: {d}"),
    }
}

/// Frames to every wait node: `[top phase, nested phase node, …]`.
fn waits(p: &RPhase, frames: &mut Vec<Frame>, out: &mut Vec<Vec<Frame>>) {
    for (id, n) in &p.nodes {
        match &n.kind {
            RKind::Wait(_) => {
                let mut f = frames.clone();
                f.push(Frame {
                    id: p.id.clone(),
                    node: id.clone(),
                    visits: Default::default(),
                });
                out.push(f);
            }
            RKind::Phase(inner) => {
                frames.push(Frame {
                    id: p.id.clone(),
                    node: id.clone(),
                    visits: Default::default(),
                });
                waits(inner, frames, out);
                frames.pop();
            }
            _ => {}
        }
    }
}

fn all_waits(res: &Resolved) -> Vec<Vec<Frame>> {
    let mut out = Vec::new();
    for p in &res.phases {
        waits(p, &mut Vec::new(), &mut out);
    }
    out
}

/// The nested frames of a path are keyed by the phase node id, as the interpreter does.
fn state_at(frames: &[Frame]) -> TaskState {
    let mut s = TaskState {
        status: Status::Waiting,
        frames: frames.to_vec(),
        ..Default::default()
    };
    // Nested frames carry the phase node id as their id.
    for i in 1..s.frames.len() {
        s.frames[i].id = s.frames[i - 1].node.clone();
    }
    s
}

fn comment(body: &str, writer: bool) -> TaskEvent {
    let mut e = Event::new(Payload::Comment(Comment::parsed(body, Some("enzarb"))));
    e.actor = Some(Actor {
        login: "u".into(),
        can_write: writer,
        bot: false,
    });
    TaskEvent::Event { event: e }
}

const WORKFLOWS: [&str; 3] = ["issue", "pull_request", "mention"];

#[test]
fn the_library_declares_the_common_commands() {
    for wf in WORKFLOWS {
        // Loading fails on any lint error and `diags.is_empty()` rules out warnings.
        load(wf);
    }
}

#[test]
fn every_wait_leaves_on_retry_and_ends_on_cancel() {
    let mut seen = 0;
    for wf in WORKFLOWS {
        let res = load(wf);
        for frames in all_waits(&res) {
            let at = state_at(&frames);
            let here = format!("{wf}: {}", at.position().unwrap());
            seen += 1;

            let (after, _) = step(&res, &at, &comment("/retry", true));
            assert!(
                after.path.len() > at.path.len() || after.status != Status::Waiting,
                "{here}: /retry did not leave the wait"
            );

            let (after, _) = step(&res, &at, &comment("/cancel", true));
            assert_eq!(
                after.status,
                Status::Ended {
                    status: "cancelled".into()
                },
                "{here}: /cancel"
            );

            // A reader gets an answer, and the wait stays.
            let (after, fx) = step(&res, &at, &comment("/retry", false));
            assert_eq!(after.status, Status::Waiting, "{here}");
            assert!(
                fx.iter().any(
                    |e| matches!(e, Effect::Reply { body, .. } if body.contains("write access"))
                ),
                "{here}: no reply to a reader's /retry"
            );
        }
    }
    assert!(seen >= 10, "only {seen} waits found");
}

#[test]
fn a_command_no_wait_knows_gets_a_reply_with_the_accepted_ones() {
    let res = load("issue");
    for frames in all_waits(&res) {
        let at = state_at(&frames);
        let (after, fx) = step(&res, &at, &comment("/frobnicate", true));
        assert_eq!(after.position(), at.position(), "{:?}", at.position());
        let replies: Vec<_> = fx
            .iter()
            .filter_map(|e| match e {
                Effect::Reply { body, .. } => Some(body.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(replies.len(), 1, "{:?}", at.position());
        for c in ["`/retry`", "`/cancel`"] {
            assert!(replies[0].contains(c), "{c} in {}", replies[0]);
        }
    }
}
