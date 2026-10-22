//! Provider end-to-end tests against a tiny local HTTP server serving canned SSE streams.

use ez_llm::anthropic::Anthropic;
use ez_llm::openai::OpenAi;
use ez_llm::{ContentBlock, Message, Provider, Request, StopReason, StreamEvent, ToolSpec, Usage};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Captured {
    headers: Vec<(String, String)>,
    body: Value,
    path: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Serve exactly one request with the given status/content-type/body, then close.
/// Returns the base URL and a handle yielding the captured request.
async fn serve(
    status: &str,
    content_type: &'static str,
    body: String,
) -> (String, tokio::task::JoinHandle<Captured>) {
    // Loopback must never go through the sandbox HTTPS/HTTP proxy.
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for k in ["NO_PROXY", "no_proxy"] {
            // Safe: runs once before any client in this process is built.
            // SAFETY: test-only; set before or while no other thread reads this variable.
            unsafe {
                std::env::set_var(k, "127.0.0.1,localhost");
            }
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_string();
    let h = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let (head_end, content_len) = loop {
            let n = sock.read(&mut tmp).await.unwrap();
            assert!(n > 0, "client closed early");
            buf.extend_from_slice(&tmp[..n]);
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
                let cl = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                break (p + 4, cl);
            }
        };
        while buf.len() < head_end + content_len {
            let n = sock.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&tmp[..n]);
        }
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut lines = head.lines();
        let path = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_string();
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let body_json =
            serde_json::from_slice(&buf[head_end..head_end + content_len]).unwrap_or(Value::Null);

        // Connection-close framing; write the body in small pieces to exercise chunk splitting.
        let resp_head = format!(
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n"
        );
        sock.write_all(resp_head.as_bytes()).await.unwrap();
        for piece in body.as_bytes().chunks(37) {
            sock.write_all(piece).await.unwrap();
            sock.flush().await.unwrap();
            tokio::task::yield_now().await;
        }
        let _ = sock.shutdown().await;
        Captured {
            headers,
            body: body_json,
            path,
        }
    });
    (url, h)
}

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| {
            if name.is_empty() {
                format!("data: {data}\n\n")
            } else {
                format!("event: {name}\ndata: {data}\n\n")
            }
        })
        .collect()
}

fn request() -> Request {
    Request {
        model: "test-model".into(),
        system: "be helpful".into(),
        messages: vec![Message::user_text("list files")],
        tools: vec![ToolSpec {
            name: "Bash".into(),
            description: "run a command".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"}}}),
        }],
        max_tokens: 1024,
        thinking: None,
    }
}

fn drain(mut rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut v = vec![];
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

#[tokio::test]
async fn anthropic_stream_thinking_text_and_tool() {
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","role":"assistant","usage":{"input_tokens":25,"cache_read_input_tokens":5,"output_tokens":1}}}),
        ),
        ("ping", json!({"type":"ping"})),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me "}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"think."}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQB"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"Cg=="}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"I'll run "}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"ls."}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":1}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_01","name":"Bash","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"comm"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"and\": \"ls -"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"la\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":2}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":77}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ]);
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let p = Anthropic::new("sk-ant-test", Some(url));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut req = request();
    req.thinking = Some(json!({"type":"enabled","budget_tokens":2000}));
    let r = p.complete(req, tx).await.unwrap();

    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(
        r.usage,
        Usage {
            input_tokens: 30,
            output_tokens: 77
        }
    );
    assert_eq!(
        r.message.content,
        vec![
            ContentBlock::Thinking {
                thinking: "Let me think.".into(),
                signature: Some("EqQBCg==".into())
            },
            ContentBlock::Text {
                text: "I'll run ls.".into()
            },
            ContentBlock::ToolUse {
                id: "toolu_01".into(),
                name: "Bash".into(),
                input: json!({"command":"ls -la"})
            },
        ]
    );
    assert_eq!(
        drain(rx),
        vec![
            StreamEvent::ThinkingDelta("Let me ".into()),
            StreamEvent::ThinkingDelta("think.".into()),
            StreamEvent::TextDelta("I'll run ".into()),
            StreamEvent::TextDelta("ls.".into()),
            StreamEvent::ToolUseStart {
                id: "toolu_01".into(),
                name: "Bash".into()
            },
        ]
    );

    let c = cap.await.unwrap();
    assert_eq!(c.path, "/v1/messages");
    assert_eq!(c.header("x-api-key"), Some("sk-ant-test"));
    assert_eq!(c.header("anthropic-version"), Some("2023-06-01"));
    assert!(c.header("authorization").is_none());
    assert_eq!(c.body["stream"], true);
    assert_eq!(c.body["model"], "test-model");
    assert_eq!(c.body["max_tokens"], 1024);
    assert_eq!(c.body["thinking"]["budget_tokens"], 2000);
    assert_eq!(c.body["system"][0]["text"], "be helpful");
    assert_eq!(c.body["tools"][0]["name"], "Bash");
    assert_eq!(c.body["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(c.body["messages"][0]["content"][0]["text"], "list files");
}

#[tokio::test]
async fn anthropic_error_surfaces_body() {
    let body =
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
    let (url, _cap) = serve("401 Unauthorized", "application/json", body.into()).await;
    let p = Anthropic::new("bad", Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = p.complete(request(), tx).await.unwrap_err().to_string();
    assert!(e.contains("401"), "{e}");
    assert!(e.contains("invalid x-api-key"), "{e}");
}

#[tokio::test]
async fn anthropic_stream_error_event() {
    let body = sse(&[(
        "error",
        json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
    )]);
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let p = Anthropic::new("k", Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = p.complete(request(), tx).await.unwrap_err().to_string();
    assert!(e.contains("Overloaded"), "{e}");
}

#[tokio::test]
async fn openai_stream_reasoning_text_and_tool_calls() {
    let chunk = |delta: Value, fr: Value| json!({"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":delta,"finish_reason":fr}]});
    let mut body = sse(&[
        (
            "",
            chunk(json!({"role":"assistant","content":""}), Value::Null),
        ),
        ("", chunk(json!({"reasoning_content":"Need "}), Value::Null)),
        (
            "",
            chunk(json!({"reasoning_content":"files."}), Value::Null),
        ),
        ("", chunk(json!({"content":"Running "}), Value::Null)),
        ("", chunk(json!({"content":"it."}), Value::Null)),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"Bash","arguments":""}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"pwd\"}"}}]}),
                Value::Null,
            ),
        ),
        ("", chunk(json!({}), json!("tool_calls"))),
        (
            "",
            json!({"id":"c1","choices":[],"usage":{"prompt_tokens":40,"completion_tokens":12,"total_tokens":52}}),
        ),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let p = OpenAi::new(Some("sk-test".into()), Some(format!("{url}/v1")));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut req = request();
    req.thinking = Some(json!("high"));
    let r = p.complete(req, tx).await.unwrap();

    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(
        r.usage,
        Usage {
            input_tokens: 40,
            output_tokens: 12
        }
    );
    assert_eq!(
        r.message.content,
        vec![
            ContentBlock::Thinking {
                thinking: "Need files.".into(),
                signature: None
            },
            ContentBlock::Text {
                text: "Running it.".into()
            },
            ContentBlock::ToolUse {
                id: "call_a".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"})
            },
            ContentBlock::ToolUse {
                id: "call_b".into(),
                name: "Bash".into(),
                input: json!({"command":"pwd"})
            },
        ]
    );
    assert_eq!(
        drain(rx),
        vec![
            StreamEvent::ThinkingDelta("Need ".into()),
            StreamEvent::ThinkingDelta("files.".into()),
            StreamEvent::TextDelta("Running ".into()),
            StreamEvent::TextDelta("it.".into()),
            StreamEvent::ToolUseStart {
                id: "call_a".into(),
                name: "Bash".into()
            },
            StreamEvent::ToolUseStart {
                id: "call_b".into(),
                name: "Bash".into()
            },
        ]
    );

    let c = cap.await.unwrap();
    assert_eq!(c.path, "/v1/chat/completions");
    assert_eq!(c.header("authorization"), Some("Bearer sk-test"));
    assert!(c.header("x-api-key").is_none());
    assert_eq!(c.body["stream"], true);
    assert_eq!(c.body["stream_options"]["include_usage"], true);
    assert_eq!(c.body["reasoning_effort"], "high");
    assert_eq!(c.body["messages"][0]["role"], "system");
    assert_eq!(c.body["messages"][1]["content"], "list files");
    assert_eq!(c.body["tools"][0]["type"], "function");
    assert_eq!(c.body["tools"][0]["function"]["name"], "Bash");
    assert_eq!(
        c.body["tools"][0]["function"]["parameters"]["type"],
        "object"
    );
}

#[tokio::test]
async fn openai_no_key_sends_no_auth_and_plain_text() {
    let chunk =
        |delta: Value, fr: Value| json!({"choices":[{"index":0,"delta":delta,"finish_reason":fr}]});
    let mut body = sse(&[
        ("", chunk(json!({"content":"hello"}), Value::Null)),
        ("", chunk(json!({}), json!("stop"))),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let p = OpenAi::new(None, Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = p.complete(request(), tx).await.unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    assert_eq!(r.message.text(), "hello");
    let c = cap.await.unwrap();
    assert_eq!(c.path, "/chat/completions");
    assert!(c.header("authorization").is_none());
}

#[tokio::test]
async fn openai_error_surfaces_body() {
    let body = r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
    let (url, _cap) = serve("404 Not Found", "application/json", body.into()).await;
    let p = OpenAi::new(None, Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = p.complete(request(), tx).await.unwrap_err().to_string();
    assert!(e.contains("404"), "{e}");
    assert!(e.contains("model not found"), "{e}");
}
