//! Chat APIs called directly: OpenAI-compatible chat completions and Anthropic's Messages API,
//! streamed and reported as `atelier_agents` events so the panel shows them like a CLI's.

use atelier::agents::AgentEvent;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

/// Who said a message of the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

/// How to reach a chat API.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatApi {
    OpenAi {
        base_url: String,
        api_key: Option<String>,
        model: String,
    },
    Anthropic {
        base_url: String,
        api_key: String,
        model: String,
    },
}

/// The Messages API version this app is written against.
const ANTHROPIC_VERSION: &str = "2026-10-21";

impl ChatApi {
    fn request(&self, client: &reqwest::Client, system: &str, history: &[Message]) -> reqwest::RequestBuilder {
        let role = |r: Role| match r {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        match self {
            Self::OpenAi {
                base_url,
                api_key,
                model,
            } => {
                let mut messages = vec![json!({ "role": "system", "content": system })];
                messages.extend(
                    history
                        .iter()
                        .map(|m| json!({ "role": role(m.role), "content": m.text })),
                );
                let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
                let request = client.post(url).json(&json!({
                    "model": model,
                    "messages": messages,
                    "stream": true,
                }));
                match api_key {
                    Some(key) => request.bearer_auth(key),
                    None => request,
                }
            }
            Self::Anthropic {
                base_url,
                api_key,
                model,
            } => {
                let messages: Vec<Value> = history
                    .iter()
                    .map(|m| json!({ "role": role(m.role), "content": m.text }))
                    .collect();
                let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
                client
                    .post(url)
                    .header("x-api-key", api_key)
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    // On a policy decline, the API answers with a fallback model instead of
                    // stopping.
                    .header("anthropic-beta", "server-side-fallback-2026-07-01")
                    .json(&json!({
                        "model": model,
                        "max_tokens": 64000,
                        "stream": true,
                        "system": system,
                        "thinking": { "type": "adaptive", "display": "summarized" },
                        "fallbacks": "default",
                        "messages": messages,
                    }))
            }
        }
    }

    /// What one streamed record says, as events; `Err` ends the turn with that message.
    fn read(&self, data: &str) -> Result<Vec<AgentEvent>, String> {
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return Ok(Vec::new());
        };
        let text = |s: &Value| s.as_str().filter(|t| !t.is_empty()).map(str::to_string);
        match self {
            Self::OpenAi { .. } => {
                if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
                    return Err(message.to_string());
                }
                let delta = &value["choices"][0]["delta"];
                let mut events = Vec::new();
                // Ollama and others stream reasoning beside the answer under one of these.
                for key in ["reasoning_content", "reasoning"] {
                    if let Some(t) = text(&delta[key]) {
                        events.push(AgentEvent::Reasoning { text: t });
                    }
                }
                if let Some(t) = text(&delta["content"]) {
                    events.push(AgentEvent::Text { text: t });
                }
                Ok(events)
            }
            Self::Anthropic { .. } => match value["type"].as_str() {
                Some("content_block_delta") => {
                    let delta = &value["delta"];
                    Ok(match delta["type"].as_str() {
                        Some("text_delta") => text(&delta["text"])
                            .map(|t| AgentEvent::Text { text: t })
                            .into_iter()
                            .collect(),
                        Some("thinking_delta") => text(&delta["thinking"])
                            .map(|t| AgentEvent::Reasoning { text: t })
                            .into_iter()
                            .collect(),
                        _ => Vec::new(),
                    })
                }
                Some("message_delta") if value["delta"]["stop_reason"] == "refusal" => {
                    Err("モデルがこの依頼を断りました".into())
                }
                Some("error") => Err(value["error"]["message"]
                    .as_str()
                    .unwrap_or("API がエラーを返しました")
                    .to_string()),
                _ => Ok(Vec::new()),
            },
        }
    }
}

/// Streams one answer to `history`, sending its events to `events` and ending with
/// [`AgentEvent::TurnCompleted`] or [`AgentEvent::TurnFailed`].
pub async fn answer(api: ChatApi, system: String, history: Vec<Message>, events: UnboundedSender<AgentEvent>) {
    let send = |event| {
        events.unbounded_send(event).ok();
    };
    match stream(&api, &system, &history, &send).await {
        Ok(answer) => send(AgentEvent::TurnCompleted { answer }),
        Err(message) => send(AgentEvent::TurnFailed { message }),
    }
}

async fn stream(
    api: &ChatApi,
    system: &str,
    history: &[Message],
    send: &impl Fn(AgentEvent),
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let response = api
        .request(&client, system, history)
        .send()
        .await
        .map_err(|e| format!("API に接続できませんでした: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or(body);
        return Err(format!("API がエラーを返しました（{status}）: {}", detail.trim()));
    }
    let mut answer = String::new();
    let mut lines = SseLines::default();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| format!("応答が途中で切れました: {e}"))?;
        for data in lines.push(&chunk) {
            if data == "[DONE]" {
                return Ok(answer);
            }
            for event in api.read(&data)? {
                if let AgentEvent::Text { text } = &event {
                    answer.push_str(text);
                }
                send(event);
            }
        }
    }
    Ok(answer)
}

/// Splits a server-sent event stream into the payloads of its `data:` lines.
#[derive(Default)]
struct SseLines {
    pending: Vec<u8>,
}

impl SseLines {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(data) = line.strip_prefix("data:") {
                out.push(data.trim_start().to_string());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_lines_are_joined_across_chunks() {
        let mut lines = SseLines::default();
        assert!(lines.push(b"event: content_block_delta\ndata: {\"a\"").is_empty());
        assert_eq!(lines.push(b":1}\r\n\ndata: [DONE]\n"), vec!["{\"a\":1}", "[DONE]"]);
        // A character split between chunks survives.
        let bytes = "data: 日本\n".as_bytes();
        assert!(lines.push(&bytes[..8]).is_empty());
        assert_eq!(lines.push(&bytes[8..]), vec!["日本"]);
    }

    fn openai() -> ChatApi {
        ChatApi::OpenAi {
            base_url: "http://localhost:11434/v1".into(),
            api_key: None,
            model: "m".into(),
        }
    }

    fn anthropic() -> ChatApi {
        ChatApi::Anthropic {
            base_url: "https://api.anthropic.com".into(),
            api_key: "k".into(),
            model: "claude-opus-5-5".into(),
        }
    }

    #[test]
    fn reads_both_stream_formats() {
        let text = |s: &str| AgentEvent::Text { text: s.into() };
        assert_eq!(
            openai().read(r#"{"choices":[{"delta":{"content":"こん"}}]}"#).unwrap(),
            vec![text("こん")]
        );
        assert_eq!(
            openai()
                .read(r#"{"choices":[{"delta":{"reasoning":"考え中"}}]}"#)
                .unwrap(),
            vec![AgentEvent::Reasoning {
                text: "考え中".into()
            }]
        );
        assert!(openai().read(r#"{"error":{"message":"no model"}}"#).is_err());
        assert_eq!(
            anthropic()
                .read(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#)
                .unwrap(),
            vec![text("Hi")]
        );
        assert!(
            anthropic()
                .read(r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#)
                .is_err()
        );
        assert!(
            anthropic()
                .read(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)
                .is_err()
        );
        assert!(anthropic().read(r#"{"type":"ping"}"#).unwrap().is_empty());
    }

    #[test]
    fn streams_an_answer_from_an_openai_compatible_server() {
        crate::runtime::runtime().block_on(async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let _ = socket.read(&mut buf).await.unwrap();
                let body = "data: {\"choices\":[{\"delta\":{\"content\":\"要約\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"です\"}}]}\n\ndata: [DONE]\n\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let api = ChatApi::OpenAi {
                base_url: format!("http://{addr}/v1"),
                api_key: None,
                model: "m".into(),
            };
            let (sender, receiver) = futures::channel::mpsc::unbounded();
            let history = vec![Message {
                role: Role::User,
                text: "要約して".into(),
            }];
            answer(api, "system".into(), history, sender).await;
            let events: Vec<AgentEvent> = receiver.collect().await;
            assert_eq!(
                events.last(),
                Some(&AgentEvent::TurnCompleted {
                    answer: "要約です".into()
                })
            );
        });
    }
}
