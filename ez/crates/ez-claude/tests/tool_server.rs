use ez_claude::{ToolReply, ToolServer};
use ez_llm::ToolSpec;
use serde_json::{Value, json};

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("{name} tool"),
        input_schema: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
    }
}

async fn post(server: &ToolServer, token: Option<&str>, body: Value) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(server.url())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_string());
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    req.send().await.unwrap()
}

#[tokio::test]
async fn rejects_requests_without_the_bearer_token() {
    let (server, _calls) = ToolServer::start(vec![spec("Read")]).await.unwrap();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "t", "version": "1"}}});
    assert_eq!(post(&server, None, init.clone()).await.status(), 401);
    assert_eq!(post(&server, Some("wrong"), init).await.status(), 401);
}

#[tokio::test]
async fn lists_and_forwards_tool_calls() {
    let (server, mut calls) = ToolServer::start(vec![spec("Read")]).await.unwrap();
    let token = server.token().to_string();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "t", "version": "1"}}});
    let r = post(&server, Some(&token), init).await;
    assert_eq!(r.status(), 200);

    server.set_tools(vec![spec("Read"), spec("Write")]);
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    let body: Value = post(&server, Some(&token), list)
        .await
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Read", "Write"]);

    tokio::spawn(async move {
        while let Some(c) = calls.recv().await {
            let text = format!("{} got {}", c.name, c.arguments["x"]);
            let _ = c.reply.send(ToolReply {
                text,
                is_error: c.name == "Write",
            });
        }
    });
    let call = |id: u64, name: &str| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": {"name": name, "arguments": {"x": "hi"}}})
    };
    let body: Value = post(&server, Some(&token), call(3, "Read"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["result"]["content"][0]["text"], "Read got \"hi\"");
    assert_eq!(body["result"]["isError"], json!(false));
    let body: Value = post(&server, Some(&token), call(4, "Write"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["result"]["isError"], json!(true));
}
