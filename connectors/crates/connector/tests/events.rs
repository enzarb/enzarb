use enzarb_connector::*;
use serde_json::Value;

fn load(name: &str) -> (String, Event) {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(path).unwrap();
    let ev = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
    (raw, ev)
}

/// Fixtures must round-trip to the same JSON value.
#[test]
fn fixtures_round_trip() {
    for name in [
        "comment_added",
        "work_item_closed",
        "review_submitted",
        "work_item_opened",
    ] {
        let (raw, ev) = load(name);
        let want: Value = serde_json::from_str(&raw).unwrap();
        let got = serde_json::to_value(&ev).unwrap();
        // Absent optional fields are omitted on output; fixtures with explicit
        // nulls only use them inside `item`, which always serializes them.
        assert_eq!(got, want, "{name}");
        assert_eq!(serde_json::from_value::<Event>(got).unwrap(), ev);
    }
}

#[test]
fn comment_added_fields() {
    let (_, ev) = load("comment_added");
    assert_eq!(ev.specversion, "1.0");
    assert_eq!(ev.event_type, EventType::CommentAdded);
    assert_eq!(ev.subject.to_string(), "github:enzarb/enzarb-ui#42");
    assert_eq!(ev.data.repo.to_string(), "github:enzarb/enzarb-ui");
    assert_eq!(ev.data.item.kind, ItemKind::Issue);
    assert_eq!(ev.data.comment.as_ref().unwrap().id, 987);
    assert_eq!(ev.data.merged, None);
    assert_eq!(
        ev.dedupe_key(),
        "github:72d3162e-cc78-11e3-81ab-4c9367dc0958"
    );
    assert_eq!(ev.time.to_rfc3339(), "2026-10-08T12:00:00+00:00");
}

#[test]
fn closed_and_review_extras() {
    let (_, ev) = load("work_item_closed");
    assert_eq!(ev.data.merged, Some(true));
    assert_eq!(ev.data.item.kind, ItemKind::ChangeRequest);
    assert_eq!(ev.data.item.head_ref.as_deref(), Some("enzarb/issue-42"));
    let (_, ev) = load("review_submitted");
    let r = ev.data.review.unwrap();
    assert_eq!((r.id, r.state.as_str()), (555, "changes_requested"));
}

#[test]
fn event_types_use_contract_names() {
    for t in EventType::ALL {
        assert_eq!(
            serde_json::to_value(t).unwrap(),
            Value::String(t.as_str().into())
        );
    }
    assert!(serde_json::from_str::<EventType>("\"Bogus\"").is_err());
}

#[test]
fn bot_flag_defaults_false() {
    let a: Actor = serde_json::from_str(r#"{"login":"x","id":1}"#).unwrap();
    assert!(!a.is_bot);
}

#[test]
fn new_derives_subject_and_version() {
    let (_, ev) = load("comment_added");
    let rebuilt = Event::new(
        "github",
        ev.id.clone(),
        ev.event_type,
        ev.time,
        ev.data.clone(),
    );
    assert_eq!(rebuilt, ev);
}

#[test]
fn cred_debug_redacts_token() {
    let c = ShortLivedCred {
        token: "ghs_secret".into(),
        repo: "github:a/b".parse().unwrap(),
        scope: CredScope::Read,
        expires_at: "2026-10-08T12:00:00Z".parse().unwrap(),
    };
    assert!(!format!("{c:?}").contains("ghs_secret"));
}

#[test]
fn permission_ordering() {
    assert!(Permission::Admin.can_write());
    assert!(Permission::Write.can_write());
    assert!(!Permission::Triage.can_write());
    assert!(Permission::None < Permission::Read);
}

fn ci_event_json(ci: Option<&str>) -> String {
    let ci = ci.map(|c| format!(r#","ci":{c}"#)).unwrap_or_default();
    format!(
        r#"{{"specversion":"1.0","source":"github","id":"d1","type":"CiStatusChanged",
"subject":"github:enzarb/enzarb-ui#7","time":"2026-10-08T12:00:00Z",
"data":{{"repo":"github:enzarb/enzarb-ui","actor":{{"login":"ci","id":1}},
"item":{{"kind":"change_request","number":7,"url":"u"}}{ci}}}}}"#
    )
}

#[test]
fn ci_status_changed_round_trips() {
    let raw = ci_event_json(Some(
        r#"{"head_sha":"abc","state":"failure","context":"build","url":"https://x/1"}"#,
    ));
    let ev: Event = serde_json::from_str(&raw).unwrap();
    assert_eq!(ev.event_type, EventType::CiStatusChanged);
    let ci = ev.data.ci.clone().unwrap();
    assert_eq!(ci.state, CiState::Failure);
    assert_eq!(ci.context.as_deref(), Some("build"));
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["data"]["ci"]["state"], "failure");
    assert_eq!(serde_json::from_value::<Event>(v).unwrap(), ev);
}

#[test]
fn ci_optional_fields_and_old_payloads() {
    let ev: Event = serde_json::from_str(&ci_event_json(Some(
        r#"{"head_sha":"abc","state":"pending"}"#,
    )))
    .unwrap();
    let v = serde_json::to_value(&ev).unwrap();
    assert!(v["data"]["ci"].get("context").is_none());
    assert!(v["data"]["ci"].get("url").is_none());

    // Payloads from before `ci` existed still deserialize and omit it again.
    let (raw, old) = load("comment_added");
    assert!(old.data.ci.is_none());
    let want: Value = serde_json::from_str(&raw).unwrap();
    assert!(want["data"].get("ci").is_none());
    assert!(
        serde_json::to_value(&old).unwrap()["data"]
            .get("ci")
            .is_none()
    );
}

#[test]
fn ci_event_type_registered() {
    assert_eq!(EventType::ALL.len(), 9);
    assert!(EventType::ALL.contains(&EventType::CiStatusChanged));
    assert_eq!(EventType::CiStatusChanged.as_str(), "CiStatusChanged");
    assert_eq!(
        serde_json::from_str::<EventType>("\"CiStatusChanged\"").unwrap(),
        EventType::CiStatusChanged
    );
    for s in ["success", "failure", "pending"] {
        assert!(serde_json::from_str::<CiState>(&format!("\"{s}\"")).is_ok());
    }
}
