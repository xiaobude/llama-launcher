// ======================================================================================
// Anthropic Messages API 兼容代理模块
//
// 📌 维护说明 (PR #17570)：
// llama.cpp 已经在 PR #17570 中增加了对 Anthropic Messages API (/v1/messages)
// 的原生支持。新版客户端推荐直接指向 llama-server 的主服务端口以获得最佳性能。
// 
// 本模块（内嵌 Axum 代理）之所以作为备用保留，出于以下考虑：
// 1. 兼容老版本 llama.cpp（未合并 PR #17570 的旧构建）。
// 2. 局域网访问下的简单 API Key 验证（llama-server 暂无自带轻量安全校验）。
// 3. 应对官方或客户端接口细节的兼容性微调需求。
// ======================================================================================

use async_stream::stream;
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::sse::{Event, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::convert::Infallible;

#[derive(Clone)]
struct ProxyConfig {
    upstream_port: u16,
    api_key: String,
}

fn extract_provided_key(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.to_string())
}

async fn auth_middleware(State(cfg): State<ProxyConfig>, req: Request, next: Next) -> Response {
    if !cfg.api_key.is_empty() {
        let provided = extract_provided_key(req.headers());
        if provided.as_deref() != Some(cfg.api_key.as_str()) {
            return error_response(StatusCode::UNAUTHORIZED, "无效的 API key");
        }
    }
    next.run(req).await
}

#[derive(Debug, Deserialize)]
struct AnthropicRequest {
    model: String,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    messages: Vec<Value>,
    #[serde(default)]
    system: Option<Value>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    top_p: Option<f64>,
    #[serde(default)]
    top_k: Option<i64>,
    #[serde(default)]
    stop_sequences: Option<Vec<String>>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    tools: Option<Vec<Value>>,
}

fn default_max_tokens() -> u32 {
    1024
}

#[derive(Debug, Deserialize)]
struct CountTokensRequest {
    messages: Vec<Value>,
    #[serde(default)]
    system: Option<Value>,
}

pub async fn run_proxy(listen_port: u16, upstream_port: u16, api_key: String) {
    let cfg = ProxyConfig { upstream_port, api_key };
    let app = Router::new()
        .route("/v1/messages", post(messages_handler))
        .route("/v1/messages/count_tokens", post(count_tokens_handler))
        .layer(middleware::from_fn_with_state(cfg.clone(), auth_middleware))
        .with_state(cfg);

    let addr = format!("0.0.0.0:{}", listen_port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("anthropic proxy: failed to bind {}: {}", addr, e);
            return;
        }
    };
    let _ = axum::serve(listener, app).await;
}

fn error_response(status: axum::http::StatusCode, msg: &str) -> Response {
    (
        status,
        Json(json!({"type":"error","error":{"type":"api_error","message": msg}})),
    )
        .into_response()
}

fn extract_text_blocks(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn map_stop_reason(openai_reason: Option<&str>) -> &'static str {
    match openai_reason {
        Some("length") => "max_tokens",
        Some("tool_calls") => "tool_use",
        _ => "end_turn",
    }
}

// Anthropic Messages -> OpenAI chat.completions request body
fn to_openai_request(req: &AnthropicRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    if let Some(sys) = &req.system {
        let text = extract_text_blocks(sys);
        if !text.is_empty() {
            messages.push(json!({"role": "system", "content": text}));
        }
    }

    for m in &req.messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        match m.get("content") {
            Some(Value::String(s)) => {
                messages.push(json!({"role": role, "content": s}));
            }
            Some(Value::Array(blocks)) => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<Value> = Vec::new();

                for b in blocks {
                    match b.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                text_parts.push(t.to_string());
                            }
                        }
                        Some("tool_use") => {
                            let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let input = b.get("input").cloned().unwrap_or_else(|| json!({}));
                            tool_calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": { "name": name, "arguments": input.to_string() }
                            }));
                        }
                        Some("tool_result") => {
                            let tool_use_id = b
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let content_text = b
                                .get("content")
                                .map(extract_text_blocks)
                                .unwrap_or_default();
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content_text
                            }));
                        }
                        _ => {}
                    }
                }

                if !tool_calls.is_empty() {
                    messages.push(json!({
                        "role": role,
                        "content": Value::Null,
                        "tool_calls": tool_calls
                    }));
                } else if !text_parts.is_empty() {
                    messages.push(json!({"role": role, "content": text_parts.join("\n")}));
                }
            }
            _ => {}
        }
    }

    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": req.max_tokens,
        "stream": req.stream,
    });

    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = req.top_p {
        body["top_p"] = json!(t);
    }
    if let Some(t) = req.top_k {
        body["top_k"] = json!(t);
    }
    if let Some(s) = &req.stop_sequences {
        body["stop"] = json!(s);
    }
    if let Some(tools) = &req.tools {
        let openai_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.get("name").cloned().unwrap_or_else(|| json!("")),
                        "description": t.get("description").cloned().unwrap_or_else(|| json!("")),
                        "parameters": t.get("input_schema").cloned().unwrap_or_else(|| json!({"type":"object","properties":{}}))
                    }
                })
            })
            .collect();
        body["tools"] = json!(openai_tools);
    }
    if req.stream {
        body["stream_options"] = json!({"include_usage": true});
    }

    body
}

// OpenAI chat.completion response -> Anthropic Messages response (non-streaming)
fn openai_to_anthropic_response(body: &Value, model: &str) -> Value {
    let choice = body.get("choices").and_then(|c| c.get(0));
    let message = choice.and_then(|c| c.get("message"));
    let finish_reason = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str());

    let mut content: Vec<Value> = Vec::new();
    if let Some(text) = message.and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
        if !text.is_empty() {
            content.push(json!({"type": "text", "text": text}));
        }
    }
    if let Some(tool_calls) = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(|t| t.as_array())
    {
        for tc in tool_calls {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let input: Value = serde_json::from_str(args_str).unwrap_or_else(|_| json!({}));
            content.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
        }
    }

    let usage = body.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": map_stop_reason(finish_reason),
        "stop_sequence": null,
        "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
    })
}

async fn messages_handler(
    State(cfg): State<ProxyConfig>,
    Json(req): Json<AnthropicRequest>,
) -> Response {
    let openai_body = to_openai_request(&req);
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", cfg.upstream_port);

    if req.stream {
        return stream_messages(client, url, openai_body, req.model.clone()).await;
    }

    let resp = match client.post(&url).json(&openai_body).send().await {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                &format!("无法连接本地模型服务: {}", e),
            )
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return error_response(status, &text);
    }

    match resp.json::<Value>().await {
        Ok(body) => Json(openai_to_anthropic_response(&body, &req.model)).into_response(),
        Err(e) => error_response(
            axum::http::StatusCode::BAD_GATEWAY,
            &format!("解析上游响应失败: {}", e),
        ),
    }
}

struct ToolAcc {
    id: String,
    name: String,
    arguments: String,
}

async fn stream_messages(
    client: reqwest::Client,
    url: String,
    openai_body: Value,
    model: String,
) -> Response {
    let resp = match client.post(&url).json(&openai_body).send().await {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                &format!("无法连接本地模型服务: {}", e),
            )
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return error_response(status, &text);
    }

    let message_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    let stream = build_sse_stream(resp, message_id, model);
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

fn build_sse_stream(
    upstream: reqwest::Response,
    message_id: String,
    model: String,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream! {
        yield Ok(Event::default().event("message_start").data(json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": model,
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": 0, "output_tokens": 0}
            }
        }).to_string()));

        let mut text_index: Option<usize> = None;
        let mut tool_accs: BTreeMap<u64, ToolAcc> = BTreeMap::new();
        let mut next_index: usize = 0;
        let mut finish_reason: Option<String> = None;
        let mut usage_completion: u64 = 0;

        let mut byte_stream = upstream.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(_) => break,
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim_end_matches('\r').to_string();
                buf.drain(..=pos);
                if line.is_empty() || !line.starts_with("data:") {
                    continue;
                }
                let payload = line["data:".len()..].trim();
                if payload == "[DONE]" {
                    continue;
                }
                let parsed: Value = match serde_json::from_str(payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let choice = parsed.get("choices").and_then(|c| c.get(0));

                if let Some(delta) = choice.and_then(|c| c.get("delta")) {
                    if let Some(text) = delta.get("content").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            if text_index.is_none() {
                                let idx = next_index;
                                next_index += 1;
                                text_index = Some(idx);
                                yield Ok(Event::default().event("content_block_start").data(json!({
                                    "type": "content_block_start", "index": idx,
                                    "content_block": {"type": "text", "text": ""}
                                }).to_string()));
                            }
                            yield Ok(Event::default().event("content_block_delta").data(json!({
                                "type": "content_block_delta", "index": text_index.unwrap(),
                                "delta": {"type": "text_delta", "text": text}
                            }).to_string()));
                        }
                    }

                    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            let tc_index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                            let entry = tool_accs.entry(tc_index).or_insert_with(|| ToolAcc {
                                id: String::new(),
                                name: String::new(),
                                arguments: String::new(),
                            });
                            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                                entry.id = id.to_string();
                            }
                            if let Some(f) = tc.get("function") {
                                if let Some(name) = f.get("name").and_then(|v| v.as_str()) {
                                    entry.name.push_str(name);
                                }
                                if let Some(args) = f.get("arguments").and_then(|v| v.as_str()) {
                                    entry.arguments.push_str(args);
                                }
                            }
                        }
                    }
                }

                if let Some(fr) = choice.and_then(|c| c.get("finish_reason")).and_then(|f| f.as_str()) {
                    finish_reason = Some(fr.to_string());
                }
                if let Some(u) = parsed.get("usage") {
                    if let Some(c) = u.get("completion_tokens").and_then(|v| v.as_u64()) {
                        usage_completion = c;
                    }
                }
            }
        }

        if let Some(idx) = text_index {
            yield Ok(Event::default().event("content_block_stop").data(json!({
                "type": "content_block_stop", "index": idx
            }).to_string()));
        }

        for (_, acc) in tool_accs.into_iter() {
            let idx = next_index;
            next_index += 1;
            if let Err(e) = serde_json::from_str::<Value>(&acc.arguments) {
                eprintln!("⚠️ [Anthropic Proxy] Tool arguments are not valid JSON: {} | raw: {}", e, acc.arguments);
            }
            yield Ok(Event::default().event("content_block_start").data(json!({
                "type": "content_block_start", "index": idx,
                "content_block": {"type": "tool_use", "id": acc.id, "name": acc.name, "input": {}}
            }).to_string()));
            yield Ok(Event::default().event("content_block_delta").data(json!({
                "type": "content_block_delta", "index": idx,
                "delta": {"type": "input_json_delta", "partial_json": acc.arguments}
            }).to_string()));
            yield Ok(Event::default().event("content_block_stop").data(json!({
                "type": "content_block_stop", "index": idx
            }).to_string()));
        }

        yield Ok(Event::default().event("message_delta").data(json!({
            "type": "message_delta",
            "delta": {"stop_reason": map_stop_reason(finish_reason.as_deref()), "stop_sequence": null},
            "usage": {"output_tokens": usage_completion}
        }).to_string()));

        yield Ok(Event::default().event("message_stop").data(json!({"type": "message_stop"}).to_string()));
    }
}

async fn count_tokens_handler(
    State(cfg): State<ProxyConfig>,
    Json(req): Json<CountTokensRequest>,
) -> Response {
    let mut text = String::new();
    if let Some(sys) = &req.system {
        text.push_str(&extract_text_blocks(sys));
        text.push('\n');
    }
    for m in &req.messages {
        if let Some(content) = m.get("content") {
            text.push_str(&extract_text_blocks(content));
            text.push('\n');
        }
    }

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/tokenize", cfg.upstream_port);
    match client.post(&url).json(&json!({"content": text})).send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(body) => {
                let count = body
                    .get("tokens")
                    .and_then(|t| t.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                Json(json!({"input_tokens": count})).into_response()
            }
            Err(e) => error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                &format!("解析 tokenize 响应失败: {}", e),
            ),
        },
        Err(e) => error_response(
            axum::http::StatusCode::BAD_GATEWAY,
            &format!("无法连接本地模型服务: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_stop_reason_translates_known_values() {
        assert_eq!(map_stop_reason(Some("length")), "max_tokens");
        assert_eq!(map_stop_reason(Some("tool_calls")), "tool_use");
        assert_eq!(map_stop_reason(Some("stop")), "end_turn");
        assert_eq!(map_stop_reason(None), "end_turn");
    }

    #[test]
    fn to_openai_request_maps_system_and_text_message() {
        let req: AnthropicRequest = serde_json::from_value(json!({
            "model": "m",
            "max_tokens": 50,
            "system": "be nice",
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .unwrap();
        let openai = to_openai_request(&req);
        assert_eq!(openai["messages"][0]["role"], "system");
        assert_eq!(openai["messages"][0]["content"], "be nice");
        assert_eq!(openai["messages"][1]["role"], "user");
        assert_eq!(openai["messages"][1]["content"], "hello");
    }

    #[test]
    fn to_openai_request_maps_tool_use_and_tool_result() {
        let req: AnthropicRequest = serde_json::from_value(json!({
            "model": "m",
            "max_tokens": 50,
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "get_weather", "input": {"city": "NY"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "sunny"}
                ]}
            ]
        }))
        .unwrap();
        let openai = to_openai_request(&req);
        let msgs = openai["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "t1");
        assert_eq!(msgs[1]["content"], "sunny");
    }

    #[test]
    fn openai_to_anthropic_response_maps_text_and_usage() {
        let body = json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello world"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        });
        let anthropic = openai_to_anthropic_response(&body, "m");
        assert_eq!(anthropic["type"], "message");
        assert_eq!(anthropic["content"][0]["type"], "text");
        assert_eq!(anthropic["content"][0]["text"], "Hello world");
        assert_eq!(anthropic["stop_reason"], "end_turn");
        assert_eq!(anthropic["usage"]["input_tokens"], 5);
        assert_eq!(anthropic["usage"]["output_tokens"], 2);
    }

    async fn mock_chat_completions(Json(body): Json<Value>) -> Response {
        let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
        if stream {
            let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
                       data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n\
                       data: [DONE]\n\n";
            ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], sse).into_response()
        } else {
            Json(json!({
                "choices": [{"message": {"role": "assistant", "content": "Hello world"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 2}
            }))
            .into_response()
        }
    }

    async fn mock_tokenize(Json(_body): Json<Value>) -> Response {
        Json(json!({"tokens": [1, 2, 3, 4, 5]})).into_response()
    }

    async fn spawn_mock_upstream() -> u16 {
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_chat_completions))
            .route("/tokenize", post(mock_tokenize));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        port
    }

    async fn free_port() -> u16 {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    }

    #[tokio::test]
    async fn end_to_end_non_streaming_messages() {
        let upstream_port = spawn_mock_upstream().await;
        let proxy_port = free_port().await;
        tokio::spawn(run_proxy(proxy_port, upstream_port, String::new()));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/v1/messages", proxy_port))
            .json(&json!({
                "model": "test-model",
                "max_tokens": 100,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["type"], "message");
        assert_eq!(body["content"][0]["text"], "Hello world");
        assert_eq!(body["stop_reason"], "end_turn");
        assert_eq!(body["usage"]["input_tokens"], 5);
        assert_eq!(body["usage"]["output_tokens"], 2);
    }

    #[tokio::test]
    async fn end_to_end_streaming_messages() {
        let upstream_port = spawn_mock_upstream().await;
        let proxy_port = free_port().await;
        tokio::spawn(run_proxy(proxy_port, upstream_port, String::new()));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/v1/messages", proxy_port))
            .json(&json!({
                "model": "test-model",
                "max_tokens": 100,
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let text = resp.text().await.unwrap();
        assert!(text.contains("event: message_start"));
        assert!(text.contains("\"text\":\"Hello\""));
        assert!(text.contains("\"text\":\" world\""));
        assert!(text.contains("event: message_stop"));
    }

    #[tokio::test]
    async fn end_to_end_count_tokens() {
        let upstream_port = spawn_mock_upstream().await;
        let proxy_port = free_port().await;
        tokio::spawn(run_proxy(proxy_port, upstream_port, String::new()));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/v1/messages/count_tokens", proxy_port))
            .json(&json!({"messages": [{"role": "user", "content": "hello there"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["input_tokens"], 5);
    }

    #[tokio::test]
    async fn api_key_rejects_missing_or_wrong_key_and_accepts_correct_key() {
        let upstream_port = spawn_mock_upstream().await;
        let proxy_port = free_port().await;
        tokio::spawn(run_proxy(proxy_port, upstream_port, "secret123".to_string()));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let client = reqwest::Client::new();
        let body = json!({
            "model": "test-model",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}]
        });

        let no_key = client
            .post(format!("http://127.0.0.1:{}/v1/messages", proxy_port))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(no_key.status(), 401);

        let wrong_key = client
            .post(format!("http://127.0.0.1:{}/v1/messages", proxy_port))
            .header("x-api-key", "wrong")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(wrong_key.status(), 401);

        let right_key = client
            .post(format!("http://127.0.0.1:{}/v1/messages", proxy_port))
            .header("x-api-key", "secret123")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(right_key.status(), 200);
    }
}
