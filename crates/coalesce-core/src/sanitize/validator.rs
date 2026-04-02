use crate::types::ChatRequest;

/// Maximum request body size (1MB default)
pub const MAX_REQUEST_SIZE_BYTES: usize = 1_048_576;
/// Maximum number of messages per request
pub const MAX_MESSAGE_COUNT: usize = 100;
/// Maximum length of a single message content
pub const MAX_MESSAGE_LENGTH: usize = 500_000;

#[derive(Debug)]
pub struct ValidationError {
    pub code: &'static str,
    pub message: String,
}

/// Validate a chat request against safety constraints.
pub fn validate_request(request: &ChatRequest) -> Result<(), ValidationError> {
    // Must have at least one message
    if request.messages.is_empty() {
        return Err(ValidationError {
            code: "empty_messages",
            message: "Request must contain at least one message".into(),
        });
    }

    // Message count limit
    if request.messages.len() > MAX_MESSAGE_COUNT {
        return Err(ValidationError {
            code: "too_many_messages",
            message: format!(
                "Request contains {} messages, max is {}",
                request.messages.len(),
                MAX_MESSAGE_COUNT
            ),
        });
    }

    // Validate each message
    for (i, msg) in request.messages.iter().enumerate() {
        // Role must be valid
        match msg.role.as_str() {
            "system" | "user" | "assistant" | "tool" => {}
            other => {
                return Err(ValidationError {
                    code: "invalid_role",
                    message: format!("Message {} has invalid role: '{}'", i, other),
                });
            }
        }

        // Check content length
        if let Some(content) = &msg.content {
            if let Some(text) = content.as_text() {
                if text.len() > MAX_MESSAGE_LENGTH {
                    return Err(ValidationError {
                        code: "message_too_long",
                        message: format!(
                            "Message {} content is {} bytes, max is {}",
                            i,
                            text.len(),
                            MAX_MESSAGE_LENGTH
                        ),
                    });
                }
            }
        }
    }

    // Temperature range
    if let Some(temp) = request.temperature {
        if !(0.0..=2.0).contains(&temp) {
            return Err(ValidationError {
                code: "invalid_temperature",
                message: format!("Temperature {} is out of range [0.0, 2.0]", temp),
            });
        }
    }

    // top_p range
    if let Some(top_p) = request.top_p {
        if !(0.0..=1.0).contains(&top_p) {
            return Err(ValidationError {
                code: "invalid_top_p",
                message: format!("top_p {} is out of range [0.0, 1.0]", top_p),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use std::collections::HashMap;

    fn simple_request(messages: Vec<(&str, &str)>) -> ChatRequest {
        ChatRequest {
            model: "test".into(),
            messages: messages
                .into_iter()
                .map(|(role, content)| Message {
                    role: role.into(),
                    content: Some(MessageContent::Text(content.into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: HashMap::new(),
                })
                .collect(),
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            extra: HashMap::new(),
        }
    }

    #[test]
    fn test_valid_request() {
        let req = simple_request(vec![("user", "Hello")]);
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_empty_messages() {
        let req = simple_request(vec![]);
        assert!(validate_request(&req).is_err());
    }

    #[test]
    fn test_invalid_role() {
        let req = simple_request(vec![("hacker", "Hello")]);
        let err = validate_request(&req).unwrap_err();
        assert_eq!(err.code, "invalid_role");
    }

    #[test]
    fn test_valid_roles() {
        for role in &["system", "user", "assistant", "tool"] {
            let req = simple_request(vec![(role, "Hello")]);
            assert!(validate_request(&req).is_ok());
        }
    }

    #[test]
    fn test_too_many_messages() {
        let msgs: Vec<(&str, &str)> = (0..101).map(|_| ("user", "hi")).collect();
        let req = simple_request(msgs);
        let err = validate_request(&req).unwrap_err();
        assert_eq!(err.code, "too_many_messages");
    }

    #[test]
    fn test_temperature_range() {
        let mut req = simple_request(vec![("user", "hi")]);
        req.temperature = Some(3.0);
        assert!(validate_request(&req).is_err());
        req.temperature = Some(1.0);
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_top_p_range() {
        let mut req = simple_request(vec![("user", "hi")]);
        req.top_p = Some(1.5);
        assert!(validate_request(&req).is_err());
        req.top_p = Some(0.9);
        assert!(validate_request(&req).is_ok());
    }
}
