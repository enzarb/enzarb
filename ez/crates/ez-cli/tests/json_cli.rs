//! The machine-readable (`--json`) modes the desktop app uses inside WSL: shapes must stay stable.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Env {
    proj: tempfile::TempDir,
    cfg: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            proj: tempfile::tempdir().unwrap(),
            cfg: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn rc(&self, args: &[&str], stdin: &str) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ez"));
        c.args(args)
            .current_dir(self.proj.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("xdg"))
            .env("EZ_CONFIG_DIR", self.cfg.path())
            .env("RC_TEST_PRESENT", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn json(&self, args: &[&str], stdin: &str) -> Value {
        let o = self.rc(args, stdin);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let out = String::from_utf8(o.stdout).unwrap();
        assert_eq!(
            out.trim().lines().count(),
            1,
            "one JSON line expected: {out}"
        );
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }
}

impl Env {
    /// Like `json` but the document may be an array.
    fn json_any(&self, args: &[&str]) -> Value {
        self.json(args, "")
    }
}

const PLAN: &str = r#"{"profileName":"or","provider":"openai","baseUrl":"https://openrouter.ai/api/v1",
 "model":"a/one","models":["a/one","b/two"],"newKey":{"name":"work","secret":"sk-secret-0042","description":"w"},
 "keyScope":"user","setDefault":true,"modeModels":[{"mode":"explore","profile":"or","model":"b/two"},{"mode":"test"}]}"#;

const MODES: &str = "default_mode = \"explore\"\n[modes.explore]\ndescription = \"x\"\n[modes.test]\ndescription = \"y\"\nmodel = \"old\"\n";

#[test]
fn state_shape_and_onboarding() {
    let e = Env::new();
    let s = e.json(&["setup", "--json-state"], "");
    assert_eq!(s["needsOnboarding"], true);
    assert_eq!(s["defaultProfile"], "anthropic");
    assert_eq!(s["profiles"], serde_json::json!([]));
    assert_eq!(s["keys"], serde_json::json!({"keys": [], "missing": []}));
    assert!(s.get("env").is_none(), "no env-var presence in the state");
    assert_eq!(s["modes"], serde_json::json!({"status": "missing"}));
    assert!(s["project"].is_string());
}

#[test]
fn plan_stdin_preview_then_apply_then_state() {
    let e = Env::new();
    std::fs::create_dir_all(e.proj.path().join(".ez")).unwrap();
    std::fs::write(e.proj.path().join(".ez/modes.toml"), MODES).unwrap();

    let p = e.json(&["setup", "--plan-stdin", "--preview", "--json"], PLAN);
    let diff = p["diff"].as_str().unwrap();
    assert!(
        diff.contains("+[profiles.or]") || diff.contains("+provider"),
        "{diff}"
    );
    assert!(
        diff.contains("modes.toml") && !diff.contains("sk-secret"),
        "{diff}"
    );
    assert!(
        !e.cfg.path().join("config.toml").exists(),
        "preview must not write"
    );

    let r = e.json(&["setup", "--plan-stdin", "--json"], PLAN);
    for k in [
        "configPath",
        "projectConfigPath",
        "credentialsPath",
        "modesPath",
        "modesUpdated",
    ] {
        assert!(r.get(k).is_some(), "missing {k}: {r}");
    }
    assert!(
        r["credentialsPath"]
            .as_str()
            .unwrap()
            .ends_with("credentials.toml")
    );
    assert_eq!(r["projectConfigPath"], Value::Null);
    assert_eq!(r["modesUpdated"], 2);
    assert!(!r.to_string().contains("sk-secret"));

    let s = e.json(&["setup", "--json-state"], "");
    assert_eq!(s["needsOnboarding"], false);
    assert_eq!(s["defaultProfile"], "or");
    assert_eq!(s["profiles"][0]["apiKeyName"], "work");
    assert_eq!(
        s["profiles"][0]["models"],
        serde_json::json!(["a/one", "b/two"])
    );
    assert_eq!(s["keys"]["keys"][0]["masked"], "…0042");
    assert_eq!(s["keys"]["keys"][0]["usedBy"][0], "profile `or` (user)");
    assert_eq!(s["modes"]["status"], "ready");
    assert_eq!(s["modes"]["modes"][0]["model"], "b/two");
    assert!(!s.to_string().contains("sk-secret"));
    assert!(e.read_cfg("credentials.toml").contains("sk-secret-0042"));
}

impl Env {
    fn read_cfg(&self, rel: &str) -> String {
        std::fs::read_to_string(self.cfg.path().join(rel)).unwrap()
    }
}

#[test]
fn non_interactive_flags_json_and_preview() {
    let e = Env::new();
    let args = [
        "setup",
        "--non-interactive",
        "--provider",
        "openai",
        "--base-url",
        "http://localhost:11434/v1/",
        "--models",
        "llama3",
        "--json",
    ];
    let mut with_preview = args.to_vec();
    with_preview.push("--preview");
    assert!(e.json(&with_preview, "")["diff"].is_string());
    assert!(!e.cfg.path().join("config.toml").exists());
    let r = e.json(&args, "");
    assert!(r["configPath"].as_str().unwrap().ends_with("config.toml"));
    assert!(e.read_cfg("config.toml").contains("llama3"));
}

#[test]
fn bad_plan_fails_with_message_on_stderr() {
    let e = Env::new();
    let o = e.rc(&["setup", "--plan-stdin", "--json"], "{not json");
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("invalid plan JSON"));
    let o = e.rc(
        &["setup", "--plan-stdin", "--json"],
        r#"{"profileName":"a b","provider":"openai","model":"m"}"#,
    );
    assert!(!o.status.success());
    assert!(o.stdout.is_empty());
}

#[test]
fn keys_json_roundtrip() {
    let e = Env::new();
    let a = e.json(
        &[
            "keys",
            "add",
            "work",
            "--api-key-stdin",
            "--description",
            "d",
            "--json",
        ],
        "sk-abcdefgh-9999\n",
    );
    assert_eq!(a["name"], "work");
    assert!(a["path"].as_str().unwrap().ends_with("credentials.toml"));
    assert!(!a.to_string().contains("9999") || !a.to_string().contains("sk-abcdefgh"));

    let u = e.json(
        &[
            "keys",
            "use",
            "work",
            "--profile",
            "p",
            "--project",
            "--json",
        ],
        "",
    );
    assert_eq!(u["scope"], "project");
    assert_eq!(u["profile"], "p");
    assert_eq!(u["name"], "work");

    let l = e.json(&["keys", "list", "--json"], "");
    let k = &l["keys"][0];
    assert_eq!(k["name"], "work");
    assert_eq!(k["description"], "d");
    assert_eq!(k["masked"], "…9999");
    assert!(k["createdAt"].is_string());
    assert_eq!(k["usedBy"][0], "profile `p` (this project)");
    assert!(!l.to_string().contains("abcdefgh"));

    let r = e.json(&["keys", "remove", "work", "--json"], "");
    assert_eq!(r["stillUsedBy"].as_array().unwrap().len(), 1);
    let l = e.json(&["keys", "list", "--json"], "");
    assert_eq!(l["keys"], serde_json::json!([]));
    assert_eq!(l["missing"][0]["name"], "work");
    assert!(
        !e.rc(&["keys", "remove", "work", "--json"], "")
            .status
            .success()
    );
}

#[test]
fn init_json() {
    let e = Env::new();
    let r = e.json(&["init", "--minimal", "--json"], "");
    assert!(r["path"].as_str().unwrap().ends_with("modes.toml"));
    assert!(r["tasksDiscovered"].is_u64());
    assert_eq!(r["modes"]["status"], "ready");
    assert!(r["modes"]["modes"].as_array().unwrap().len() >= 2);
    // refuses to overwrite without --force
    assert!(!e.rc(&["init", "--json"], "").status.success());
    assert_eq!(
        e.json(&["init", "--json", "--force"], "")["modes"]["status"],
        "ready"
    );
}

/// Serve one canned HTTP response; the request head is sent back on the channel.
fn serve_once(status: &str, body: &str) -> (String, std::sync::mpsc::Receiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", l.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let (status, body) = (status.to_string(), body.to_string());
    std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).unwrap();
        tx.send(String::from_utf8_lossy(&buf[..n]).to_string())
            .unwrap();
        let _ = write!(
            s,
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    (url, rx)
}

#[test]
fn models_json_sorted_with_stdin_key() {
    let e = Env::new();
    let (url, rx) = serve_once("200 OK", r#"{"data":[{"id":"zeta"},{"id":"alpha"}]}"#);
    let v = e.json(
        &[
            "models",
            "--provider",
            "openai",
            "--base-url",
            &url,
            "--api-key-stdin",
            "--json",
        ],
        "sk-live-1\n",
    );
    assert_eq!(
        v,
        serde_json::json!({"models":[{"id":"alpha","displayName":null},{"id":"zeta","displayName":null}]})
    );
    let head = rx.recv().unwrap().to_lowercase();
    assert!(head.contains("authorization: bearer sk-live-1"), "{head}");
    // the key is never stored
    assert!(!e.cfg.path().join("credentials.toml").exists());
}

#[test]
fn models_by_name_and_errors() {
    let e = Env::new();
    e.json(
        &["keys", "add", "k", "--api-key-stdin", "--json"],
        "named-secret\n",
    );
    let (url, rx) = serve_once("200 OK", r#"{"data":[]}"#);
    let v = e.json(
        &[
            "models",
            "--provider",
            "openai",
            "--base-url",
            &url,
            "--api-key-name",
            "k",
            "--json",
        ],
        "",
    );
    assert_eq!(v["models"], serde_json::json!([]));
    assert!(rx.recv().unwrap().contains("named-secret"));

    let (url, _rx) = serve_once("401 Unauthorized", r#"{"error":{"message":"bad key"}}"#);
    let o = e.rc(
        &[
            "models",
            "--provider",
            "openai",
            "--base-url",
            &url,
            "--json",
        ],
        "",
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("bad key"));
    // The environment-variable key option is gone.
    let o = e.rc(
        &[
            "models",
            "--provider",
            "openai",
            "--api-key-env",
            "X",
            "--json",
        ],
        "",
    );
    assert!(!o.status.success());
}

#[test]
fn models_probe_detects_the_api_flavor() {
    let e = Env::new();
    // The server only answers the OpenAI-style path; the Anthropic-first probe 404s first when
    // the key looks like an Anthropic key, so serve two connections.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let (status, body) = if head.starts_with("GET /models") {
                ("200 OK", r#"{"data":[{"id":"m1"}]}"#)
            } else {
                ("404 Not Found", "{}")
            };
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let v = e.json(
        &[
            "models",
            "--probe",
            "--base-url",
            &url,
            "--api-key-stdin",
            "--json",
        ],
        "sk-ant-test\n",
    );
    assert_eq!(v["kind"], "openai", "{v}");
    assert_eq!(v["models"][0]["id"], "m1");
    assert_eq!(v["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(v["message"], "OpenAI-compatible API ✓ (1 model)");
    // A failure is reported in the JSON (exit 0) with a null kind. A port that was just bound
    // and released is refused (a fixed port like 1 may be silently dropped by a firewall).
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let v = e.json(&["models", "--probe", "--base-url", &closed, "--json"], "");
    assert!(
        v["kind"].is_null() && v["message"].as_str().unwrap().contains("connect"),
        "{v}"
    );
}

// ---- ez sessions --json -----------------------------------------------------------------------

fn write_session(e: &Env, id: &str, first_prompt: &str, extra: &[&str]) {
    let dir = e.proj.path().join(".ez/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let mut lines = vec![
        format!(
            r#"{{"ts":"2026-03-01T10:00:00.000Z","type":"session_start","id":"{id}","cwd":"/x","created_at":"2026-03-01T10:00:00.000Z","ez_version":"0.1.0","profile":"p","model":"m1","mode":"plan"}}"#
        ),
        format!(
            r#"{{"ts":"2026-03-01T10:00:01.000Z","type":"user_message","source":"user","message":{{"role":"user","content":[{{"type":"text","text":"{first_prompt}"}}]}}}}"#
        ),
        r#"{"ts":"2026-03-01T10:00:02.000Z","type":"assistant_message","request_id":1,"model":"m1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":4},"metrics":{"duration_ms":300,"time_to_first_token_ms":50,"output_tokens_per_sec":13.3},"message":{"role":"assistant","content":[{"type":"text","text":"Found the Needle here"}]}}"#.to_string(),
        r#"{"ts":"2026-03-01T10:00:03.000Z","type":"turn_end","stop_reason":"end_turn","duration_ms":900,"model_requests":1,"tool_calls":0,"input_tokens":10,"output_tokens":4}"#.to_string(),
    ];
    lines.extend(extra.iter().map(|s| s.to_string()));
    std::fs::write(dir.join(format!("{id}.jsonl")), lines.join("\n") + "\n").unwrap();
}

#[test]
fn sessions_list_show_search_json() {
    let e = Env::new();
    // No sessions at all.
    assert_eq!(e.json_any(&["sessions", "--json"]), serde_json::json!([]));
    write_session(&e, "20260301-100000-aaaaaa", "Fix the login bug", &[]);
    write_session(
        &e,
        "20260302-100000-bbbbbb",
        "Second session",
        &["not json at all"],
    );

    let list = e.json_any(&["sessions", "--json"]);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    let a = list
        .iter()
        .find(|s| s["id"] == "20260301-100000-aaaaaa")
        .unwrap();
    assert_eq!(a["firstPrompt"], "Fix the login bug");
    assert_eq!(a["mode"], "plan");
    assert_eq!(a["model"], "m1");
    assert_eq!(a["turns"], 1);
    assert_eq!(a["inputTokens"], 10);
    assert_eq!(a["outputTokens"], 4);
    assert_eq!(a["durationMs"], 900);
    assert_eq!(a["toolCalls"], 0);
    assert_eq!(a["createdAt"], "2026-03-01T10:00:00.000Z");
    assert_eq!(a["skippedLines"], 0);
    assert!(a["sizeBytes"].as_u64().unwrap() > 100);
    let b = list
        .iter()
        .find(|s| s["id"] == "20260302-100000-bbbbbb")
        .unwrap();
    assert_eq!(b["skippedLines"], 1);

    let q = e.json_any(&["sessions", "--json", "--query", "LOGIN"]);
    assert_eq!(q.as_array().unwrap().len(), 1);

    let show = e.json_any(&["sessions", "show", "20260301-100000-aaaaaa", "--json"]);
    assert_eq!(show["total"], 4);
    assert_eq!(show["records"].as_array().unwrap().len(), 4);
    assert_eq!(show["meta"]["id"], "20260301-100000-aaaaaa");
    assert_eq!(show["meta"]["cwd"], "/x");
    assert_eq!(show["records"][2]["metrics"]["timeToFirstTokenMs"], 50);
    assert_eq!(
        show["records"][2]["content"][0]["text"],
        "Found the Needle here"
    );

    let page = e.json_any(&[
        "sessions",
        "show",
        "20260301-100000-aaaaaa",
        "--json",
        "--offset",
        "1",
        "--limit",
        "2",
    ]);
    assert_eq!(page["total"], 4);
    assert_eq!(page["offset"], 1);
    assert_eq!(page["records"][0]["index"], 1);
    assert_eq!(page["records"].as_array().unwrap().len(), 2);

    let hits = e.json_any(&["sessions", "search", "needle", "--json"]);
    let hits = hits.as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["recordIndex"], 2);
    assert_eq!(hits[0]["type"], "assistant_message");
    assert!(hits[0]["snippet"].as_str().unwrap().contains("Needle"));
    let one = e.json_any(&["sessions", "search", "needle", "--json", "--limit", "1"]);
    assert_eq!(one.as_array().unwrap().len(), 1);
}

#[test]
fn sessions_errors_are_rc_lines() {
    let e = Env::new();
    write_session(&e, "20260301-100000-aaaaaa", "x", &[]);
    for args in [
        &["sessions", "show", "nope", "--json"][..],
        &["sessions", "show", "../../etc/passwd", "--json"][..],
        &["sessions", "show", "..", "--json"][..],
    ] {
        let o = e.rc(args, "");
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(o.stdout.is_empty(), "{args:?}");
        let err = String::from_utf8(o.stderr).unwrap();
        assert!(err.starts_with("ez: "), "{err}");
        assert_eq!(err.trim().lines().count(), 1, "{err}");
    }
}

#[test]
fn modes_get_validate_set() {
    let e = Env::new();
    let g = e.json(&["modes", "get", "--json"], "");
    assert_eq!(g["status"], "missing");
    assert!(g["path"].as_str().unwrap().ends_with("modes.toml"));
    assert!(
        g["catalog"]["builtinTools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "Bash" && t["readOnly"] == false && t["kind"] == "execute")
    );
    assert_eq!(g["catalog"]["hookEvents"][4]["name"], "post_edit");
    assert_eq!(
        g["catalog"]["permissionValues"],
        serde_json::json!(["allow", "ask", "deny"])
    );
    assert_eq!(g["catalog"]["templates"].as_array().unwrap().len(), 9);
    assert!(g.get("modes").unwrap().as_array().unwrap().is_empty());

    // create from a document, previewing first
    let doc = r#"{"defaultMode":"a","modes":[{"id":"a","description":"first","tools":["Read"],
      "hooks":{"post_edit":[{"run":"fmt {files}","glob":"*.rs","onFailure":"fix_turn"}]}},
      {"id":"b","next":["a"],"permissions":{"Bash":"deny"}}]}"#;
    let p = e.json(&["modes", "set", "--doc-stdin", "--preview", "--json"], doc);
    assert_eq!(p["changed"], true);
    assert!(p["diff"].as_str().unwrap().contains("+[modes.a]"));
    assert!(!e.proj.path().join(".ez/modes.toml").exists());
    let r = e.json(&["modes", "set", "--doc-stdin", "--json"], doc);
    assert_eq!(r["changed"], true);
    assert!(r["path"].as_str().unwrap().ends_with("modes.toml"));

    let g = e.json(&["modes", "get", "--json"], "");
    assert_eq!(g["status"], "ready");
    assert_eq!(g["defaultMode"], "a");
    assert_eq!(g["modes"][0]["originalId"], "a");
    assert_eq!(g["modes"][0]["tools"], serde_json::json!(["Read"]));
    assert_eq!(
        g["modes"][0]["hooks"]["post_edit"][0]["onFailure"],
        "fix_turn"
    );
    assert_eq!(g["modes"][0]["hooks"]["post_edit"][0]["maxFixTurns"], 2);
    assert_eq!(g["modes"][1]["permissions"]["Bash"], "deny");
    assert_eq!(g["modes"][1]["tools"], Value::Null);

    // validation issues
    let v = e.json(
        &["modes", "validate", "--doc-stdin", "--json"],
        r#"{"defaultMode":"zz","modes":[{"id":"a","next":["q"],"hooks":{"stop":[{"run":"x","matcher":"("}]}}]}"#,
    );
    let paths: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"defaultMode") && paths.contains(&"modes[0].next[0]"));
    assert!(paths.contains(&"modes[0].hooks.stop[0].matcher"));
    assert_eq!(v[0]["severity"], "error");

    // an invalid document is refused with `rc: msg` and exit 1
    let o = e.rc(
        &["modes", "set", "--doc-stdin", "--json"],
        r#"{"defaultMode":"x","modes":[]}"#,
    );
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.starts_with("ez: ") && err.contains("at least one mode"),
        "{err}"
    );
    let o = e.rc(&["modes", "set", "--doc-stdin", "--json"], "not json");
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).starts_with("ez: "));
}
