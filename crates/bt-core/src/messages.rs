use crate::{MessageId, ToolResultEnvelope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_name: String,
    pub call_id: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MessagePart {
    Text {
        text: String,
    },
    Reasoning {
        text: Option<String>,
        redacted: bool,
        opaque_replay: Option<Value>,
    },
    ToolCall {
        call: ToolCall,
    },
    ToolResult {
        result: ToolResultEnvelope,
    },
    Refusal {
        text: Option<String>,
        provider_reason: Option<String>,
        opaque_metadata: Option<Value>,
    },
    Structured {
        schema_name: Option<String>,
        value: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub message_id: MessageId,
    pub role: Role,
    pub parts: Vec<MessagePart>,
    pub created_at: OffsetDateTime,
}

impl Message {
    #[must_use]
    pub fn new(role: Role, parts: Vec<MessagePart>) -> Self {
        let mut coalesced = Vec::new();
        extend_message_parts(&mut coalesced, parts);
        Self {
            message_id: MessageId::new(),
            role,
            parts: coalesced,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[must_use]
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self::new(role, vec![MessagePart::Text { text: text.into() }])
    }

    #[must_use]
    pub fn from_part(role: Role, part: MessagePart) -> Self {
        Self::new(role, vec![part])
    }

    pub fn push_part(&mut self, part: MessagePart) {
        push_message_part(&mut self.parts, part);
    }

    #[must_use]
    pub fn first_part(&self) -> Option<&MessagePart> {
        self.parts.first()
    }

    #[must_use]
    pub fn tool_call(&self) -> Option<&ToolCall> {
        self.parts.iter().find_map(|part| match part {
            MessagePart::ToolCall { call } => Some(call),
            _ => None,
        })
    }

    #[must_use]
    pub fn tool_result(&self) -> Option<&ToolResultEnvelope> {
        self.parts.iter().find_map(|part| match part {
            MessagePart::ToolResult { result } => Some(result),
            _ => None,
        })
    }

    pub fn text_parts(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().filter_map(MessagePart::visible_text)
    }
}

impl MessagePart {
    #[must_use]
    pub fn visible_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text.as_str()),
            Self::Refusal {
                text: Some(text), ..
            } => Some(text.as_str()),
            _ => None,
        }
    }

    fn can_coalesce_with(&self, next: &Self) -> bool {
        match (self, next) {
            (Self::Text { .. }, Self::Text { .. }) => true,
            (
                Self::Reasoning {
                    redacted: left_redacted,
                    opaque_replay: None,
                    ..
                },
                Self::Reasoning {
                    redacted: right_redacted,
                    opaque_replay: None,
                    ..
                },
            ) => left_redacted == right_redacted,
            _ => false,
        }
    }

    fn coalesce_with(&mut self, next: Self) -> bool {
        if !self.can_coalesce_with(&next) {
            return false;
        }

        match (self, next) {
            (Self::Text { text }, Self::Text { text: next_text }) => {
                text.push_str(&next_text);
                true
            }
            (
                Self::Reasoning {
                    text,
                    redacted: _,
                    opaque_replay: None,
                },
                Self::Reasoning {
                    text: next_text,
                    redacted: _,
                    opaque_replay: None,
                },
            ) => {
                match (text.as_mut(), next_text) {
                    (Some(current), Some(next)) => current.push_str(&next),
                    (None, Some(next)) => *text = Some(next),
                    _ => {}
                }
                true
            }
            _ => false,
        }
    }
}

pub fn extend_message_parts(
    parts: &mut Vec<MessagePart>,
    new_parts: impl IntoIterator<Item = MessagePart>,
) {
    for part in new_parts {
        push_message_part(parts, part);
    }
}

pub fn push_message_part(parts: &mut Vec<MessagePart>, part: MessagePart) {
    if let Some(current) = parts.last_mut()
        && current.coalesce_with(part.clone())
    {
        return;
    }
    parts.push(part);
}

#[must_use]
pub fn render_queue_message_input(message: &Message) -> String {
    let rendered = message
        .parts
        .iter()
        .map(render_message_part_for_queue)
        .collect::<Vec<_>>()
        .join("\n");
    if rendered.is_empty() {
        "<empty message>".to_owned()
    } else {
        rendered
    }
}

fn render_message_part_for_queue(part: &MessagePart) -> String {
    match part {
        MessagePart::Text { text } => text.clone(),
        MessagePart::Reasoning { text, redacted, .. } => {
            if *redacted {
                "[reasoning omitted]".to_owned()
            } else {
                text.clone().unwrap_or_else(|| "[reasoning]".to_owned())
            }
        }
        MessagePart::ToolCall { call } => {
            format!("[tool call] {} {}", call.tool_name, call.arguments)
        }
        MessagePart::ToolResult { result } => {
            format!("[tool result] {}", result.tool_name)
        }
        MessagePart::Refusal { text, .. } => text.clone().unwrap_or_else(|| "[refusal]".to_owned()),
        MessagePart::Structured { schema_name, value } => {
            let label = schema_name.as_deref().unwrap_or("structured");
            format!("[{label}] {value}")
        }
    }
}
