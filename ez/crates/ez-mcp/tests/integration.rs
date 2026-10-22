use ez_mcp::{McpManager, McpServerConfig};
use ez_tools::{Answer, Question, ToolContext, ToolKind, UserBridge};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct NoUser;

#[async_trait::async_trait]
impl UserBridge for NoUser {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
        anyhow::bail!("no user")
    }
}

fn ctx(cancel: CancellationToken) -> ToolContext {
    ToolContext {
        cwd: std::env::temp_dir(),
        session_id: "t".into(),
        user: Arc::new(NoUser),
        cancel,
    }
}

fn fake(extra: &str) -> McpServerConfig {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_mcp_server.py");
    McpServerConfig::stdio("python3", &[script.to_str().unwrap(), extra])
}

fn configs(items: Vec<(&str, McpServerConfig)>) -> BTreeMap<String, McpServerConfig> {
    items.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn all() -> Vec<String> {
    vec!["mcp__*".to_string()]
}

#[tokio::test]
async fn discovery_naming_and_calls() {
    let m = McpManager::start(configs(vec![("my srv", fake("0"))]), &std::env::temp_dir()).await;
    assert_eq!(m.statuses(), vec![("my srv".to_string(), Ok(5))]);
    let tools = m.tools_for(&all(), 20);
    let names: Vec<_> = tools.iter().map(|t| t.name().to_string()).collect();
    // Pagination worked (5 tools over two pages) and names are sanitized.
    assert!(names.contains(&"mcp__my_srv__echo".to_string()));
    assert!(names.contains(&"mcp__my_srv__weird_name".to_string()));
    assert_eq!(names.len(), 5);

    let echo = tools.iter().find(|t| t.name().ends_with("__echo")).unwrap();
    assert_eq!(echo.kind(), ToolKind::Read);
    assert_eq!(echo.title(&json!({})), "my srv: echo");
    assert_eq!(echo.description(), "Echo text back");
    assert_eq!(echo.input_schema()["required"][0], "text");
    let c = ctx(CancellationToken::new());
    let out = echo.call(json!({"text": "hi"}), &c).await.unwrap();
    assert!(!out.is_error);
    assert_eq!(out.content, "echo: hi");

    let fail = tools.iter().find(|t| t.name().ends_with("__fail")).unwrap();
    assert_eq!(fail.kind(), ToolKind::Other);
    let out = fail.call(json!({}), &c).await.unwrap();
    assert!(out.is_error);
    assert_eq!(out.content, "boom");

    let mixed = tools
        .iter()
        .find(|t| t.name().ends_with("__mixed"))
        .unwrap();
    let out = mixed.call(json!({}), &c).await.unwrap();
    assert!(out.content.contains("hello"));
    assert!(out.content.contains("[image: image/png]"));
    assert!(out.content.contains("file:///x.bin"));

    // Cancellation interrupts a slow call.
    let slow = tools.iter().find(|t| t.name().ends_with("__slow")).unwrap();
    let cancel = CancellationToken::new();
    let c2 = ctx(cancel.clone());
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        cancel.cancel();
    });
    let out = slow.call(json!({}), &c2).await.unwrap();
    assert!(out.is_error && out.content.contains("cancelled"));
}

#[tokio::test]
async fn failing_server_does_not_block_others() {
    let m = McpManager::start(
        configs(vec![
            ("good", fake("0")),
            (
                "bad",
                McpServerConfig::stdio("/nonexistent/mcp-server", &[]),
            ),
            (
                "bad2",
                McpServerConfig::stdio("python3", &["-c", "import sys; sys.exit(3)"]),
            ),
            ("empty", McpServerConfig::default()),
        ]),
        &std::env::temp_dir(),
    )
    .await;
    let st = m.statuses();
    assert_eq!(st.len(), 4);
    let get = |n: &str| st.iter().find(|(k, _)| k == n).unwrap().1.clone();
    assert_eq!(get("good"), Ok(5));
    assert!(get("bad").unwrap_err().contains("failed to start"));
    assert!(get("bad2").is_err());
    assert!(get("empty").unwrap_err().contains("command"));
    assert_eq!(m.tools_for(&all(), 20).len(), 5);
}

#[tokio::test]
async fn selection_and_lazy_mode() {
    let m = McpManager::start(
        configs(vec![("a", fake("30")), ("b", fake("0"))]),
        &std::env::temp_dir(),
    )
    .await;
    assert_eq!(m.statuses()[0].1, Ok(35));
    // Selection by glob.
    let only_b = m.tools_for(&["mcp__b__*".to_string()], 20);
    assert_eq!(only_b.len(), 5);
    assert!(m.tools_for(&[], 20).is_empty());
    // Above the threshold: meta-tools.
    let lazy = m.tools_for(&all(), 20);
    let names: Vec<_> = lazy.iter().map(|t| t.name().to_string()).collect();
    assert_eq!(names, vec!["McpListTools", "McpCall"]);
    let c = ctx(CancellationToken::new());
    let list = lazy[0].call(json!({"server": "b"}), &c).await.unwrap();
    assert!(list.content.contains("mcp__b__echo"));
    assert!(!list.content.contains("mcp__a__echo"));
    let call = lazy[1]
        .call(
            json!({"tool": "mcp__b__echo", "arguments": {"text": "x"}}),
            &c,
        )
        .await
        .unwrap();
    assert_eq!(call.content, "echo: x");
    let bad = lazy[1].call(json!({"tool": "mcp__zzz"}), &c).await.unwrap();
    assert!(bad.is_error);
    // A selection under the threshold stays eager.
    assert_eq!(m.tools_for(&["mcp__b__*".to_string()], 5).len(), 5);
    assert_eq!(m.tools_for(&["mcp__b__*".to_string()], 4).len(), 2);
}
