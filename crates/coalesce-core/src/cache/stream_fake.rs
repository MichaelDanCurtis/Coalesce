use bytes::Bytes;
use futures::stream;
use crate::providers::ByteStream;

/// Convert a non-streaming JSON response into a fake SSE stream.
/// Prevents client timeouts when a provider doesn't support streaming
/// by chunking the response content into SSE events.
pub fn response_to_stream(response: &serde_json::Value) -> ByteStream {
    let id = response.get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("stream-fake")
        .to_string();
    let model = response.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let content = response
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let usage = response.get("usage").cloned();
    let reasoning = response
        .pointer("/choices/0/message/reasoning_content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let timestamp = response.get("created")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let chunk_size = 30;
    let mut events: Vec<std::result::Result<Bytes, reqwest::Error>> = Vec::new();

    // Emit reasoning chunks first (if present)
    if let Some(ref reasoning_text) = reasoning {
        let reasoning_chunks: Vec<String> = reasoning_text
            .chars()
            .collect::<Vec<_>>()
            .chunks(chunk_size)
            .map(|c| c.iter().collect::<String>())
            .collect();

        for (i, chunk_text) in reasoning_chunks.iter().enumerate() {
            let chunk = serde_json::json!({
                "id": &id,
                "object": "chat.completion.chunk",
                "created": timestamp,
                "model": &model,
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": if i == 0 { "assistant" } else { "" },
                        "reasoning_content": chunk_text,
                    },
                    "finish_reason": serde_json::Value::Null,
                }],
            });
            events.push(Ok(Bytes::from(format!("data: {}\n\n", chunk))));
        }
    }

    // Emit content chunks
    let content_chunks: Vec<String> = content
        .chars()
        .collect::<Vec<_>>()
        .chunks(chunk_size)
        .map(|c| c.iter().collect::<String>())
        .collect();

    for (i, chunk_text) in content_chunks.iter().enumerate() {
        let needs_role = reasoning.is_none() && i == 0;
        let chunk = serde_json::json!({
            "id": &id,
            "object": "chat.completion.chunk",
            "created": timestamp,
            "model": &model,
            "choices": [{
                "index": 0,
                "delta": {
                    "role": if needs_role { "assistant" } else { "" },
                    "content": chunk_text,
                },
                "finish_reason": serde_json::Value::Null,
            }],
        });
        events.push(Ok(Bytes::from(format!("data: {}\n\n", chunk))));
    }

    // Final chunk with finish_reason and usage
    let mut final_data = serde_json::json!({
        "id": &id,
        "object": "chat.completion.chunk",
        "created": timestamp,
        "model": &model,
        "choices": [{
            "index": 0,
            "delta": {"content": serde_json::Value::Null},
            "finish_reason": "stop",
        }],
    });
    if let Some(usage) = usage {
        final_data["usage"] = usage;
    }
    events.push(Ok(Bytes::from(format!("data: {}\n\n", final_data))));
    events.push(Ok(Bytes::from("data: [DONE]\n\n")));

    Box::pin(stream::iter(events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    #[tokio::test]
    async fn test_response_to_stream() {
        let response = json!({
            "id": "test-123",
            "model": "gpt-4",
            "created": 1234567890u64,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help you today?",
                },
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 8,
                "total_tokens": 18,
            }
        });

        let stream = response_to_stream(&response);
        let chunks: Vec<_> = stream.collect().await;

        // Should have content chunks + final + [DONE]
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.is_ok()));

        // First chunk should contain content
        let first = chunks[0].as_ref().unwrap();
        let first_str = String::from_utf8_lossy(first);
        assert!(first_str.starts_with("data: "));
        assert!(first_str.contains("assistant"));
    }

    #[tokio::test]
    async fn test_stream_with_reasoning() {
        let response = json!({
            "id": "test-456",
            "model": "glm-4.5",
            "created": 1234567890u64,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "The answer is 42.",
                    "reasoning_content": "Let me think about this...",
                },
                "finish_reason": "stop",
            }],
        });

        let stream = response_to_stream(&response);
        let chunks: Vec<_> = stream.collect().await;

        // Should have reasoning + content + final + [DONE]
        assert!(chunks.len() >= 4);

        // First chunk should be reasoning
        let first = String::from_utf8_lossy(chunks[0].as_ref().unwrap());
        assert!(first.contains("reasoning_content"));
    }
}
