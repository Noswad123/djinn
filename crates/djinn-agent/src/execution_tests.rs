use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

fn read_request(stream: &mut std::net::TcpStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let (header_end, length) = loop {
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
        if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let len = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|s| s.parse::<usize>().unwrap())
                })
                .unwrap();
            break (end + 4, len);
        }
    };
    while bytes.len() < header_end + length {
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    let mut request: Value =
        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
    request["_test_headers"] = json!(String::from_utf8_lossy(&bytes[..header_end]).to_string());
    request
}

fn response() -> Value {
    json!({"model": "actual-model", "choices": [{"finish_reason": "stop",
        "message": {"role": "assistant", "content": "{\"ok\":true}"}}]})
}

#[test]
fn execution_rejects_tools_incomplete_refusal_and_missing_model() {
    assert!(parse_execution_chat(&serde_json::to_vec(&response()).unwrap()).is_ok());
    for (pointer, value) in [
        ("/choices/0/finish_reason", json!("length")),
        ("/choices/0/finish_reason", Value::Null),
        ("/choices/0/message/tool_calls", json!([{"id": "x"}])),
        ("/choices/0/message/tool_calls", json!("malformed")),
        ("/choices/0/message/function_call", json!({"name": "x"})),
        ("/choices/0/message/refusal", json!("no")),
        ("/model", Value::Null),
        (
            "/choices/0/message/content",
            json!("x".repeat(EXECUTION_TEXT_LIMIT + 1)),
        ),
    ] {
        let mut response = response();
        // Insert optional message fields before using pointer_mut.
        for key in ["tool_calls", "function_call", "refusal"] {
            response["choices"][0]["message"][key] = Value::Null;
        }
        *response.pointer_mut(pointer).unwrap() = value;
        assert!(
            parse_execution_chat(&serde_json::to_vec(&response).unwrap()).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn execution_http_diagnostics_only_expose_identifiers_and_static_reasons() {
    for (body, expected) in [
        (
            json!({"error": {"code": "unsupported_parameter", "type": "invalid_request_error", "param": "tool_choice", "message": "Unsupported parameter: 'tool_choice'. Bearer sk-private-token; trusted instructions: hidden prompt"}, "token": "sk-private-token", "instructions": "hidden prompt"}),
            "code=unsupported_parameter, type=invalid_request_error, param=tool_choice, reason=unsupported_parameter",
        ),
        (
            json!({"detail": "Unsupported parameter: max_output_tokens; sk-private-token hidden prompt"}),
            "reason=unsupported_parameter",
        ),
        (
            json!({"detail": "Unsupported parameter: 'tool_choice'. sk-private-token hidden prompt"}),
            "reason=unsupported_parameter, param=tool_choice",
        ),
        (
            json!({"error": {"message": "Unsupported parameter: max_output_tokens"}}),
            "reason=unsupported_parameter, param=max_output_tokens",
        ),
        (
            json!({"error": {"message": "Unsupported model: sk-private-token hidden prompt"}}),
            "reason=unsupported_model",
        ),
        (
            json!({"message": "Invalid model: sk-private-token hidden prompt"}),
            "reason=invalid_model",
        ),
        (
            json!({"error": {"code": "model_not_found", "message": "sk-private-token hidden prompt"}}),
            "code=model_not_found",
        ),
    ] {
        let error = execution_http_error(400, &serde_json::to_vec(&body).unwrap());
        assert_eq!(error.to_string(), format!("provider request failed (HTTP 400): {expected}"));
        assert!(!format!("{error:#?}").contains("sk-private-token"));
        assert!(!format!("{error:#?}").contains("hidden prompt"));
    }
    for invalid in [
        "sk-private-token".to_owned(),
        "Bearer credential".to_owned(),
        "hidden prompt\nparam=tool_choice".to_owned(),
        "tool_choice;injected".to_owned(),
        "tool_choice\u{1b}[31m".to_owned(),
        "\u{e9}".to_owned(),
        "a".repeat(65),
        String::new(),
    ] {
        for field in ["code", "type", "param"] {
            let mut body = json!({"error": {}});
            body["error"][field] = json!(invalid);
            assert_eq!(
                execution_http_error(400, &serde_json::to_vec(&body).unwrap()).to_string(),
                "provider request failed (HTTP 400)"
            );
        }
        let body = json!({"detail": format!("Unsupported parameter: '{invalid}'")});
        assert_eq!(
            execution_http_error(400, &serde_json::to_vec(&body).unwrap()).to_string(),
            "provider request failed (HTTP 400): reason=unsupported_parameter"
        );
    }
    for body in [
        b"sk-private-token hidden prompt".to_vec(),
        b"{broken sk-private-token".to_vec(),
        serde_json::to_vec(&json!({"error": {"message": "sk-private-token hidden prompt", "code": 123, "param": {"value": "tool_choice"}}})).unwrap(),
        vec![b' '; EXECUTION_ERROR_BODY_LIMIT + 1],
    ] {
        assert_eq!(execution_http_error(500, &body).to_string(), "provider request failed (HTTP 500)");
    }
}

#[test]
fn execution_http_error_bodies_are_bounded_and_never_retried() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let informative = serde_json::to_vec(&json!({"error": {"code": "unsupported_parameter", "type": "invalid_request_error", "param": "tool_choice", "message": "secret-token private-instructions"}})).unwrap();
    let mut oversized = informative.clone();
    oversized.resize(EXECUTION_ERROR_BODY_LIMIT + 1, b' ');
    let at_limit = oversized[..EXECUTION_ERROR_BODY_LIMIT].to_vec();
    for oauth in [false, true] {
        for (status, body, length, expected) in [
            (
                400,
                informative.clone(),
                None,
                ": code=unsupported_parameter, type=invalid_request_error, param=tool_choice",
            ),
            (
                429,
                informative.clone(),
                None,
                ": code=unsupported_parameter, type=invalid_request_error, param=tool_choice",
            ),
            (
                500,
                informative.clone(),
                None,
                ": code=unsupported_parameter, type=invalid_request_error, param=tool_choice",
            ),
            (
                400,
                at_limit.clone(),
                Some(0),
                ": code=unsupported_parameter, type=invalid_request_error, param=tool_choice",
            ),
            (400, oversized.clone(), None, ""),
            (400, oversized.clone(), Some(0), ""),
            (400, vec![], Some(EXECUTION_ERROR_BODY_LIMIT + 1), ""),
            (
                400,
                b"secret-token private-instructions".to_vec(),
                Some(1024),
                "",
            ),
            (400, b"{broken secret-token".to_vec(), None, ""),
        ] {
            let (endpoint, calls, handle) = server(status, body, length);
            let client = if oauth {
                OpenAiClient::with_oauth(OpenAiOAuth {
                    access: "fake".into(),
                    account_id: None,
                    codex_api_endpoint: endpoint,
                })
            } else {
                OpenAiClient::with_base_url("fake", endpoint)
            };
            let error = runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(3),
                    client.complete_execution(oauth_request()),
                )
                .await
                .unwrap()
                .err()
                .unwrap()
            });
            assert_eq!(
                error.to_string(),
                format!("provider request failed (HTTP {status}){expected}")
            );
            assert!(!error.is::<ExecutionIndeterminate>());
            assert!(!format!("{error:#?}").contains("secret-token"));
            assert!(!format!("{error:#?}").contains("private-instructions"));
            handle.join().unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
}

#[test]
fn execution_stalled_error_body_preserves_status_before_outer_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for provider in ["chat", "oauth", "copilot"] {
        for (deadline_aware, header_delay) in [
            (false, Duration::ZERO),
            (true, Duration::ZERO),
            (true, Duration::from_millis(150)),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let handle = thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let started = Instant::now();
                let mut stream = loop {
                    if let Ok((stream, _)) = listener.accept() {
                        break stream;
                    }
                    assert!(started.elapsed() < Duration::from_secs(3));
                    thread::sleep(Duration::from_millis(1));
                };
                count.fetch_add(1, Ordering::SeqCst);
                read_request(&mut stream);
                thread::sleep(header_delay);
                stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n{\"error\":{\"message\":\"secret-token private-instructions\"").unwrap();
                // Keep an incomplete diagnostic body open past the outer budget,
                // observing any unwanted retry while the original socket stalls.
                let started = Instant::now();
                while started.elapsed() < Duration::from_millis(300) {
                    if let Ok((mut retry, _)) = listener.accept() {
                        count.fetch_add(1, Ordering::SeqCst);
                        let _ = retry.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                drop(stream);
            });
            let client: Box<dyn ModelClient> = match provider {
                "oauth" => Box::new(OpenAiClient::with_oauth(OpenAiOAuth {
                    access: "fake-secret-credential".into(),
                    account_id: None,
                    codex_api_endpoint: endpoint,
                })),
                "copilot" => Box::new(CopilotClient::with_endpoint("fake-secret-credential", endpoint)),
                _ => Box::new(OpenAiClient::with_base_url("fake-secret-credential", endpoint)),
            };
            let (result, elapsed) = runtime.block_on(async {
                let started = tokio::time::Instant::now();
                let deadline = started + Duration::from_millis(200);
                let result = tokio::time::timeout_at(deadline, async {
                    if deadline_aware {
                        client.complete_execution_until(oauth_request(), deadline).await
                    } else {
                        client.complete_execution(oauth_request()).await
                    }
                }).await;
                (result, started.elapsed())
            });
            handle.join().unwrap();
            let error = result.expect("known HTTP failure reached the outer deadline").err().unwrap();
            assert_eq!(error.to_string(), "provider request failed (HTTP 401)");
            assert!(!error.is::<ExecutionIndeterminate>());
            assert!(elapsed < Duration::from_millis(200));
            let diagnostic = format!("{error:#?}");
            for secret in ["secret-token", "private-instructions", "fake-secret-credential"] {
                assert!(!diagnostic.contains(secret));
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
}

fn server(
    status: u16,
    body: Vec<u8>,
    declared_length: Option<usize>,
) -> (String, Arc<AtomicUsize>, thread::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let handle = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let start = Instant::now();
        let mut stream = loop {
            if let Ok((stream, _)) = listener.accept() {
                break stream;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "no HTTP request received"
            );
            thread::sleep(Duration::from_millis(5));
        };
        stream.set_nonblocking(false).unwrap();
        count.fetch_add(1, Ordering::SeqCst);
        let request = read_request(&mut stream);
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let length_header = match declared_length {
            Some(0) => String::new(),
            length => format!("Content-Length: {}\r\n", length.unwrap_or(body.len())),
        };
        let header = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{length_header}Connection: close\r\n\r\n");
        stream.write_all(header.as_bytes()).unwrap();
        let _ = stream.write_all(&body);
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(400) {
            if let Ok((mut stream, _)) = listener.accept() {
                count.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(
                    b"HTTP/1.1 500 Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
        request
    });
    (url, calls, handle)
}

fn oauth_response() -> Value {
    json!({"status": "completed", "model": "actual-codex", "output": [
        {"type": "reasoning", "summary": []},
        {"type": "message", "role": "assistant", "status": "completed",
         "content": [{"type": "output_text", "text": "{\"ok\":true}"}]}
    ]})
}

fn sse(event: Value) -> String {
    format!(
        "event: {}\r\ndata: {}\r\n\r\n",
        event["type"].as_str().unwrap(),
        event
    )
}

fn completed_sse() -> String {
    [
        json!({"type": "response.created", "response": {"status": "in_progress", "output": []}}),
        json!({"type": "response.output_item.added", "item": {"type": "reasoning", "summary": []}}),
        json!({"type": "response.reasoning_summary_text.delta", "delta": "reasoning metadata"}),
        json!({"type": "response.output_item.added", "item": {"type": "message", "role": "assistant", "status": "in_progress", "content": []}}),
        json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
        json!({"type": "response.output_text.delta", "delta": "{\"ok\":true}"}),
        json!({"type": "response.completed", "response": oauth_response()}),
    ].into_iter().map(sse).collect()
}

fn empty_snapshot_events() -> Vec<Value> {
    vec![
        json!({"type": "response.metadata", "model": ""}),
        json!({"type": "response.output_item.added", "output_index": 1,
            "item": {"id": "msg_1", "type": "message", "role": "assistant", "status": "in_progress", "content": []}}),
        json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 1, "content_index": 0, "delta": "{\"ok\":"}),
        json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 1, "content_index": 0, "delta": "true}"}),
        json!({"type": "response.output_text.done", "item_id": "msg_1", "output_index": 1, "content_index": 0, "text": "{\"ok\":true}"}),
        json!({"type": "response.completed", "response": {"status": "completed", "model": "actual-codex", "output": []}}),
    ]
}

#[test]
fn oauth_execution_empty_snapshot_uses_only_validated_stream_evidence() {
    let events = empty_snapshot_events();
    let mut anonymous = events.clone();
    for event in &mut anonymous {
        for key in ["item_id", "output_index", "content_index"] {
            event.as_object_mut().unwrap().remove(key);
        }
    }
    anonymous[1]["item"].as_object_mut().unwrap().remove("id");
    let mut item = oauth_response()["output"][1].clone();
    item["id"] = json!("msg_1");
    let item_done = json!({"type": "response.output_item.done", "output_index": 1, "item": item});
    let mut with_item = events[..5].to_vec();
    with_item.push(item_done.clone());
    with_item.push(events[5].clone());
    let mut full_snapshot = events.clone();
    full_snapshot[5]["response"] = oauth_response();
    for valid in [
        events.clone(),
        anonymous,
        with_item,
        full_snapshot,
        vec![events[2].clone(), events[3].clone(), events[5].clone()],
        vec![events[4].clone(), events[5].clone()],
        vec![item_done.clone(), events[5].clone()],
    ] {
        let stream: String = valid.into_iter().map(sse).collect();
        let completion = parse_execution_responses(stream.as_bytes()).unwrap();
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!(completion.model, "actual-codex");
    }
    for invalid in [
        events[..5].to_vec(),
        vec![item_done],
        vec![events[5].clone()],
        vec![events[0].clone(), events[1].clone(), events[5].clone()],
        vec![
            json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": "not evidence"}}),
            events[5].clone(),
        ],
        vec![
            json!({"type": "response.metadata", "response": oauth_response()}),
            events[5].clone(),
        ],
    ] {
        let stream: String = invalid.into_iter().map(sse).collect();
        assert!(parse_execution_responses(stream.as_bytes()).is_err());
    }
}

#[test]
fn oauth_execution_empty_snapshot_rejects_conflicts_and_multiple_streams() {
    for (index, key, value) in [
        (3, "item_id", json!("msg_2")),
        (3, "output_index", json!(2)),
        (3, "content_index", json!(1)),
        (3, "item_id", Value::Null),
        (3, "output_index", json!("1")),
        (4, "text", json!("conflicting secret")),
        (3, "type", json!("response.refusal.delta")),
    ] {
        let mut events = empty_snapshot_events();
        events[index][key] = value;
        let stream: String = events.into_iter().map(sse).collect();
        let error = parse_execution_responses(stream.as_bytes()).err().unwrap();
        assert!(!format!("{error:#?}").contains("secret"));
    }
    let events = empty_snapshot_events();
    let mut conflicting_item = oauth_response()["output"][1].clone();
    conflicting_item["content"][0]["text"] = json!("conflicting secret");
    let mut incomplete_item = oauth_response()["output"][1].clone();
    incomplete_item["status"] = json!("incomplete");
    for extra in [
        json!({"type": "response.output_item.added", "item": {"type": "function_call", "name": "secret"}}),
        json!({"type": "response.metadata", "metadata": {"refusal": "secret"}}),
        events[1].clone(),
        json!({"type": "response.output_item.added", "output_index": 2, "item": {"id": "msg_2", "type": "message", "role": "assistant", "content": []}}),
        json!({"type": "response.content_part.done", "item_id": "msg_1", "output_index": 1, "content_index": 0, "part": {"type": "output_text", "text": "conflicting secret"}}),
        json!({"type": "response.output_item.done", "output_index": 1, "item": conflicting_item}),
        json!({"type": "response.output_item.done", "output_index": 1, "item": incomplete_item}),
        events[2].clone(),
    ] {
        let stream: String = events[..5]
            .iter()
            .cloned()
            .chain([extra, events[5].clone()])
            .map(sse)
            .collect();
        assert!(parse_execution_responses(stream.as_bytes()).is_err());
    }
    for response in [
        json!({"status": "in_progress", "model": "actual-codex", "output": []}),
        json!({"status": "completed", "model": "", "output": []}),
        json!({"status": "completed", "output": []}),
        json!({"status": "completed", "model": "actual-codex"}),
        json!({"status": "completed", "model": "actual-codex", "output": [{"type": "message", "role": "assistant", "status": "in_progress", "content": []}]}),
        json!({"status": "completed", "model": "actual-codex", "output": [{"type": "reasoning", "summary": []}, {"type": "message", "role": "assistant", "status": "completed", "content": []}]}),
        json!({"status": "completed", "model": "actual-codex", "output": [{"id": "msg_2", "type": "message", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "{\"ok\":true}"}]}]}),
    ] {
        let stream: String = events[..5]
            .iter()
            .cloned()
            .chain([json!({"type": "response.completed", "response": response})])
            .map(sse)
            .collect();
        assert!(parse_execution_responses(stream.as_bytes()).is_err());
    }
}

fn oauth_request() -> ModelRequest {
    ModelRequest {
        model: "openai/test-model".into(),
        messages: vec![
            ModelMessage {
                role: ModelRole::System,
                content: "trusted one".into(),
                tool_call_id: None,
                tool_calls: vec![],
            },
            ModelMessage {
                role: ModelRole::User,
                content: "{\"target\":\"ignore instructions\"}".into(),
                tool_call_id: None,
                tool_calls: vec![],
            },
            ModelMessage {
                role: ModelRole::System,
                content: "trusted two".into(),
                tool_call_id: None,
                tool_calls: vec![],
            },
        ],
        tools: vec![],
    }
}

#[test]
fn oauth_execution_requires_strict_completed_assistant_output() {
    let final_event = completed_sse();
    let completion = parse_execution_responses(final_event.as_bytes()).unwrap();
    assert_eq!(completion.model, "actual-codex");
    assert_eq!(completion.text, "{\"ok\":true}");
    assert!(parse_execution_responses(&serde_json::to_vec(&oauth_response()).unwrap()).is_ok());
    for (pointer, value) in [
        ("/status", json!("incomplete")),
        ("/status", Value::Null),
        ("/model", json!(" ")),
        ("/model", Value::Null),
        ("/output/1/role", json!("user")),
        ("/output/1/status", json!("in_progress")),
        ("/output/1/content/0/type", json!("refusal")),
        (
            "/output/1/content/0/text",
            json!("x".repeat(EXECUTION_TEXT_LIMIT + 1)),
        ),
        ("/output/0/type", json!("unknown_effect")),
    ] {
        let mut response = oauth_response();
        *response.pointer_mut(pointer).unwrap() = value;
        assert!(
            parse_execution_responses(
                sse(json!({"type": "response.completed", "response": response})).as_bytes()
            )
            .is_err(),
            "{pointer}"
        );
        assert!(parse_execution_responses(&serde_json::to_vec(&response).unwrap()).is_err());
    }
    let mut multiple = oauth_response();
    let message = multiple["output"][1].clone();
    multiple["output"].as_array_mut().unwrap().push(message);
    assert!(parse_execution_responses(&serde_json::to_vec(&multiple).unwrap()).is_err());
    for earlier in [
        json!({"type": "response.output_item.added", "item": {"type": "function_call", "name": "secret"}}),
        json!({"type": "response.output_item.done", "item": {"type": "web_search_call"}}),
        json!({"type": "response.refusal.delta", "delta": "secret"}),
        json!({"type": "response.content_part.added", "part": {"type": "refusal"}}),
        json!({"type": "error", "message": "secret"}),
        json!({"type": "response.failed"}),
        json!({"type": "response.incomplete"}),
    ] {
        let stream = sse(earlier) + &final_event;
        let error = parse_execution_responses(stream.as_bytes()).err().unwrap();
        assert!(!error.to_string().contains("secret"));
        assert!(!error.is::<ExecutionIndeterminate>());
    }
    let delta = sse(json!({"type": "response.output_text.delta", "delta": "{\"ok\":true}"}));
    assert!(parse_execution_responses(delta.as_bytes()).is_err());
    assert!(parse_execution_responses((delta + "data: [DONE]\n\n").as_bytes()).is_err());
    assert!(parse_execution_responses(final_event.trim_end().as_bytes()).is_err());
    for trailing in [
        "data: {broken".to_owned(),
        sse(json!({"type": "response.refusal.delta", "delta": "late refusal"})),
        sse(json!({"type": "response.output_item.added", "item": {"type": "function_call"}})),
        final_event.clone(),
    ] {
        assert!(parse_execution_responses((final_event.clone() + &trailing).as_bytes()).is_ok());
    }
    let multiline = format!("data: {{\r\ndata: \"type\": \"response.completed\",\r\ndata: \"response\": {}\r\ndata: }}\r\n\r\n", oauth_response());
    assert!(parse_execution_responses(multiline.as_bytes()).is_ok());
    let overflow = sse(
        json!({"type": "response.output_text.delta", "delta": "x".repeat(EXECUTION_TEXT_LIMIT)}),
    ) + &sse(json!({"type": "response.output_text.delta", "delta": "x"}))
        + &final_event;
    assert!(parse_execution_responses(overflow.as_bytes()).is_err());
}

#[test]
fn oauth_execution_metadata_is_benign_but_not_a_completion_or_safety_bypass() {
    let metadata = sse(json!({
        "type": "response.metadata",
        "metadata": {"request_id": "synthetic-request", "service_tier": "default"}
    }));
    let terminal = sse(json!({"type": "response.completed", "response": oauth_response()}));
    let completion = parse_execution_responses((metadata.clone() + &terminal).as_bytes()).unwrap();
    assert_eq!(completion.model, "actual-codex");
    assert_eq!(completion.text, "{\"ok\":true}");
    assert!(parse_execution_responses(metadata.as_bytes()).is_err());
    // Even a completed snapshot in metadata cannot replace the terminal event.
    let snapshot = sse(json!({"type": "response.metadata", "response": oauth_response()}));
    assert!(parse_execution_responses(snapshot.as_bytes()).is_err());

    for hidden in [
        json!({"nested": {"type": "function_call", "name": "secret"}}),
        json!({"nested": [{"tool_calls": [{"name": "secret"}]}]}),
        json!({"response": {"output": [{"type": "web_search_call", "query": "secret"}]}}),
        json!({"nested": {"refusal": "secret"}}),
        json!({"nested": [{"type": "refusal", "text": "secret"}]}),
    ] {
        let stream = sse(json!({"type": "response.metadata", "metadata": hidden})) + &terminal;
        let error = parse_execution_responses(stream.as_bytes()).err().unwrap();
        assert!(!error.is::<ExecutionIndeterminate>());
        assert!(!format!("{error:#?}").contains("secret"));
    }
}

#[test]
fn oauth_execution_http_is_single_attempt_bounded_and_authenticated() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for (status, body, length, success) in [
        (200, completed_sse().into_bytes(), None, true),
        (200, completed_sse().into_bytes(), Some(EXECUTION_BODY_LIMIT + 1), true),
        (
            200,
            serde_json::to_vec(&oauth_response()).unwrap(),
            None,
            true,
        ),
        (429, b"secret".to_vec(), None, false),
        (500, b"secret".to_vec(), None, false),
        (302, vec![], None, false),
        (200, vec![b' '; EXECUTION_BODY_LIMIT + 1], Some(EXECUTION_BODY_LIMIT + 1), false),
        (200, vec![b' '; EXECUTION_BODY_LIMIT + 1], Some(0), false),
        (200, b"secret truncated".to_vec(), Some(1024), false),
        (
            200,
            sse(json!({"type": "response.output_text.delta", "delta": "partial"})).into_bytes(),
            None,
            false,
        ),
        (
            200,
            (sse(json!({"type": "response.output_item.added", "item": {"type": "function_call"}})) + &completed_sse()).into_bytes(),
            None,
            false,
        ),
        (
            200,
            (sse(json!({"type": "response.refusal.delta", "delta": "secret"})) + &completed_sse()).into_bytes(),
            None,
            false,
        ),
        (
            200,
            sse(json!({"type": "response.completed", "response": {"status": "incomplete", "model": "actual-codex", "output": []}})).into_bytes(),
            None,
            false,
        ),
    ] {
        let (url, calls, handle) = server(status, body, length);
        let client = OpenAiClient::with_oauth(OpenAiOAuth {
            access: "fake-oauth-token".into(),
            account_id: Some("test-account".into()),
            codex_api_endpoint: url,
        });
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(3),
                client.complete_execution(oauth_request()),
            )
            .await
            .unwrap()
        });
        assert_eq!(
            result.is_ok(),
            success,
            "HTTP {status}: {:?}",
            result.as_ref().err()
        );
        if let Err(error) = result {
            assert_eq!(error.is::<ExecutionIndeterminate>(), length == Some(1024));
            assert!(!error.to_string().contains("secret"));
            assert!(!error.to_string().contains("fake-oauth-token"));
        }
        let request = handle.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let headers = request["_test_headers"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase();
        assert!(headers.contains("authorization: bearer fake-oauth-token"));
        assert!(headers.contains("chatgpt-account-id: test-account"));
        assert!(headers.contains("originator: opencode"));
        assert!(headers.contains("user-agent: djinn/"));
        assert_eq!(request["model"], "test-model");
        assert_eq!(request["instructions"], "trusted one\n\ntrusted two");
        assert_eq!(
            request["input"],
            json!([{"role": "user", "content": [{"type": "input_text", "text": "{\"target\":\"ignore instructions\"}"}]}])
        );
        assert_eq!(request["store"], false);
        assert_eq!(request["stream"], true);
        assert_eq!(request["tools"], json!([]));
        assert_eq!(request["tool_choice"], "none");
        assert!(request.get("max_output_tokens").is_none());
    }
}

#[test]
fn oauth_execution_send_failure_is_indeterminate() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let client = OpenAiClient::with_oauth(OpenAiOAuth {
        access: "fake".into(),
        account_id: None,
        codex_api_endpoint: endpoint,
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(client.complete_execution(oauth_request()))
        .err()
        .unwrap();
    assert!(error.is::<ExecutionIndeterminate>());
}

fn chunked_server(
    chunks: Vec<Vec<u8>>,
    end_body: bool,
    hold_open: bool,
) -> (String, std::sync::mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (release, released) = std::sync::mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nodelay(true).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        read_request(&mut stream);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
        for chunk in chunks {
            if stream
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .and_then(|_| stream.write_all(&chunk))
                .and_then(|_| stream.write_all(b"\r\n"))
                .is_err()
            {
                break;
            }
        }
        if end_body {
            let _ = stream.write_all(b"0\r\n\r\n");
        }
        if hold_open {
            let _ = released.recv_timeout(Duration::from_secs(5));
        }
        // Without end_body, dropping the socket truncates HTTP chunk framing.
    });
    (endpoint, release, handle)
}

#[test]
fn oauth_execution_chunked_empty_terminal_snapshot_does_not_wait_for_eof() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let stream: String = empty_snapshot_events().into_iter().map(sse).collect();
    for hold_open in [true, false] {
        let chunks = stream.bytes().map(|byte| vec![byte]).collect();
        let (endpoint, release, handle) = chunked_server(chunks, false, hold_open);
        let client = OpenAiClient::with_oauth(OpenAiOAuth {
            access: "fake".into(),
            account_id: None,
            codex_api_endpoint: endpoint,
        });
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(2),
                client.complete_execution(oauth_request()),
            )
            .await
        });
        let _ = release.send(());
        handle.join().unwrap();
        let completion = result.expect("execution waited for EOF").unwrap();
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!(completion.model, "actual-codex");
    }
}

#[test]
fn oauth_execution_chunked_terminal_does_not_wait_for_eof() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut response = oauth_response();
    response["output"][1]["content"][0]["text"] = json!("{\"label\":\"caf\u{e9}\"}");
    let terminal = format!("event: response.completed\r\ndata: {{\r\ndata: \"type\": \"response.completed\",\r\ndata: \"response\": {}\r\ndata: }}\r\n\r\n", response);
    let forbidden =
        sse(json!({"type": "response.output_item.added", "item": {"type": "function_call"}}));
    let refusal = sse(json!({"type": "response.refusal.delta", "delta": "secret"}));
    let truncated = terminal[..terminal.len() - 2].to_owned();
    for (body, end_body, hold_open, success, indeterminate) in [
        (terminal.clone(), false, true, true, false),
        (terminal.clone(), false, false, true, false),
        (forbidden + &terminal, false, false, false, false),
        (refusal + &terminal, false, false, false, false),
        (truncated.clone(), true, false, false, false),
        (truncated, false, false, false, true),
        (
            sse(json!({"type": "response.output_text.delta", "delta": "partial"})),
            true,
            false,
            false,
            false,
        ),
    ] {
        // One-byte HTTP chunks split UTF-8, CRLF, fields, JSON, and delimiters.
        let chunks = body.bytes().map(|byte| vec![byte]).collect();
        let (endpoint, release, handle) = chunked_server(chunks, end_body, hold_open);
        let client = OpenAiClient::with_oauth(OpenAiOAuth {
            access: "fake".into(),
            account_id: None,
            codex_api_endpoint: endpoint,
        });
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(2),
                client.complete_execution(oauth_request()),
            )
            .await
        });
        let _ = release.send(());
        handle.join().unwrap();
        let result = result.expect("execution waited for EOF after terminal dispatch");
        assert_eq!(result.is_ok(), success, "{:?}", result.as_ref().err());
        match result {
            Ok(completion) => {
                assert_eq!(completion.text, "{\"label\":\"caf\u{e9}\"}");
                assert_eq!(completion.model, "actual-codex");
            }
            Err(error) => {
                assert_eq!(error.is::<ExecutionIndeterminate>(), indeterminate);
                assert!(!error.to_string().contains("secret"));
            }
        }
    }
}

#[test]
fn oauth_execution_incremental_parser_preserves_boundaries_and_limits() {
    let mut response = oauth_response();
    response["output"][1]["content"][0]["text"] = json!("caf\u{e9}");
    let terminal = format!("data: {{\r\ndata: \"type\": \"response.completed\",\r\ndata: \"response\": {}\r\ndata: }}\r\n\r\n", response).into_bytes();
    for split in 0..terminal.len() {
        let mut parser = ExecutionResponsesParser::default();
        assert!(parser.push(&terminal[..split]).unwrap().is_none());
        let completion = parser.push(&terminal[split..]).unwrap().unwrap();
        assert_eq!(completion.text, "caf\u{e9}");
    }
    // Even valid terminal JSON needs the complete blank-line delimiter.
    for end in 0..terminal.len() {
        let mut parser = ExecutionResponsesParser::default();
        assert!(parser.push(&terminal[..end]).unwrap().is_none());
        assert!(parser.finish().is_err());
    }
    let mut suffix = terminal.clone();
    suffix.extend_from_slice(&[0xff, 0xfe]);
    assert!(parse_execution_responses(&suffix).is_ok());
    let mut parser = ExecutionResponsesParser::default();
    assert!(parser.push(b"data: \xff\n\n").is_err());

    let mut parser = ExecutionResponsesParser::default();
    for byte in serde_json::to_vec(&response).unwrap() {
        assert!(parser.push(&[byte]).unwrap().is_none());
    }
    assert_eq!(parser.finish().unwrap().text, "caf\u{e9}");

    let mut padding = vec![b':'; EXECUTION_BODY_LIMIT - terminal.len() - 1];
    padding.push(b'\n');
    let mut parser = ExecutionResponsesParser::default();
    assert!(parser.push(&padding).unwrap().is_none());
    let mut with_trailing_bytes = terminal.clone();
    with_trailing_bytes.extend(vec![b'x'; EXECUTION_BODY_LIMIT]);
    assert!(parser.push(&with_trailing_bytes).unwrap().is_some());
    assert_eq!(parser.consumed_bytes, EXECUTION_BODY_LIMIT);

    let mut parser = ExecutionResponsesParser::default();
    padding.insert(0, b':');
    assert!(parser.push(&padding).unwrap().is_none());
    assert!(parser.push(&terminal).is_err());
}

#[test]
fn execution_http_is_bounded_and_never_retries() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for (status, body, length, success) in [
        (200, serde_json::to_vec(&response()).unwrap(), None, true),
        (500, b"secret provider failure".to_vec(), None, false),
        (429, vec![], None, false),
        (200, vec![], Some(EXECUTION_BODY_LIMIT + 1), false),
        (200, vec![b' '; EXECUTION_BODY_LIMIT + 1], Some(0), false),
        (200, b"{broken secret".to_vec(), None, false),
        (200, b"truncated body".to_vec(), Some(1024), false),
    ] {
        let (url, calls, handle) = server(status, body, length);
        let client = OpenAiClient::with_base_url("fake-key", &url);
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(3),
                client.complete_execution(ModelRequest {
                    model: "openai/test-model".into(),
                    messages: vec![],
                    tools: vec![],
                }),
            )
            .await
            .expect("HTTP test timed out")
        });
        assert_eq!(
            result.is_ok(),
            success,
            "HTTP {status}: {:?}",
            result.as_ref().err()
        );
        if let Err(error) = result {
            assert!(!error.to_string().contains("secret"));
            assert_eq!(error.is::<ExecutionIndeterminate>(), length == Some(1024));
        }
        let request = handle.join().unwrap();
        assert_eq!(request["model"], "test-model");
        assert!(request.get("tools").is_none());
        assert_eq!(request["tool_choice"], "none");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
