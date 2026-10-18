//! Sandbox code that creates files, run where creation fails (a read-only directory).

mod common;

use sandbox_init::capture::write_atomic;
use sandbox_init::mcp;
use serde_json::json;

#[test]
fn write_atomic_fails_cleanly_in_a_read_only_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let target = dir.join("out.json");
    std::fs::write(&target, b"old").unwrap();
    let res = common::read_only(dir, || write_atomic(&target, b"new", 0o644));
    assert!(res.is_err(), "creating the temp file must fail");
    assert_eq!(std::fs::read(&target).unwrap(), b"old", "target untouched");
    let names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["out.json"], "no temp file left behind");
    write_atomic(&target, b"new", 0o644).expect("works again once writable");
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
}

#[test]
fn submit_result_reports_a_tool_error_when_the_result_cannot_be_stored() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("result.json");
    let schema = json!({"type": "object"});
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": mcp::TOOL, "arguments": {}}});
    let reply = common::read_only(tmp.path(), || mcp::handle(&schema, &out, &msg)).unwrap();
    assert!(
        reply.get("error").is_none(),
        "not a protocol error: {reply}"
    );
    assert_eq!(reply["result"]["isError"], true);
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("cannot store the result"), "{text}");
    assert!(!out.exists());
}
