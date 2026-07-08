//! Export format selection and non-OTLP renderers live here. Message
//! presentation helpers are delegated to `render`, and OTLP conversion stays in
//! `otlp` so this module only owns top-level export dispatch.

use bt_core::{BelltowerError, EventEnvelope, Message, Result, Role, SessionRecord};
use bt_protocol::ExportFormat;
use serde::{Deserialize, Serialize};

use crate::otlp::export_otlp_json;
use crate::render::{escape_html, render_message_text};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionExport {
    pub format: ExportFormat,
    pub content_type: String,
    pub content: String,
}

pub fn export_session(
    session: &SessionRecord,
    messages: &[Message],
    events: &[EventEnvelope],
    format: ExportFormat,
) -> Result<SessionExport> {
    let content = match format {
        ExportFormat::LegacyBundle => {
            return Err(BelltowerError::InvalidState(
                "legacy bundle exports are rendered from bt-session bundles".to_owned(),
            ));
        }
        ExportFormat::Jsonl => export_jsonl(events)?,
        ExportFormat::Html => export_html(session, messages)?,
        ExportFormat::ShareGpt => export_sharegpt(session, messages)?,
        ExportFormat::Otlp => export_otlp_json(session, events)?,
    };

    Ok(SessionExport {
        content_type: format.content_type().to_owned(),
        format,
        content,
    })
}

fn export_jsonl(events: &[EventEnvelope]) -> Result<String> {
    let lines = events
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(lines.join("\n"))
}

fn export_html(session: &SessionRecord, messages: &[Message]) -> Result<String> {
    let created_at = session
        .created_at
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| BelltowerError::Storage(error.to_string()))?;
    let message_html = messages
        .iter()
        .map(render_message_html)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "<!doctype html>\
<html lang=\"en\">\
<head>\
<meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title}</title>\
<style>\
body {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; background: #f6f3ea; color: #1f1c17; margin: 0; padding: 32px; }}\
.wrap {{ max-width: 960px; margin: 0 auto; }}\
.meta {{ background: #e7dfcf; border: 1px solid #cdbfa6; padding: 16px; margin-bottom: 20px; }}\
.message {{ border: 1px solid #d6c9b2; background: #fffaf0; padding: 16px; margin-bottom: 12px; }}\
.role {{ font-size: 12px; letter-spacing: 0.08em; text-transform: uppercase; color: #7c6645; margin-bottom: 8px; }}\
pre {{ white-space: pre-wrap; word-break: break-word; margin: 0; }}\
</style>\
</head>\
<body>\
<div class=\"wrap\">\
<div class=\"meta\">\
<h1>{title}</h1>\
<div>Session: {session_id}</div>\
<div>Project root: {project_root}</div>\
<div>Connection: {connection_id}</div>\
<div>Created: {created_at}</div>\
</div>\
{message_html}\
</div>\
</body>\
</html>",
        title = escape_html(
            session
                .display_name
                .as_deref()
                .unwrap_or("Belltower Session Export")
        ),
        session_id = escape_html(&session.session_id.to_string()),
        project_root = escape_html(session.project_root.as_str()),
        connection_id = escape_html(&session.connection_id.to_string()),
        created_at = escape_html(&created_at),
        message_html = message_html,
    ))
}

fn render_message_html(message: &Message) -> String {
    format!(
        "<section class=\"message\"><div class=\"role\">{}</div><pre>{}</pre></section>",
        escape_html(&format!("{:?}", message.role)),
        escape_html(&render_message_text(message))
    )
}

fn export_sharegpt(session: &SessionRecord, messages: &[Message]) -> Result<String> {
    let conversations = messages
        .iter()
        .map(|message| {
            let from = match message.role {
                Role::User => "human",
                Role::Assistant => "gpt",
                Role::Tool => "tool",
                Role::System => "system",
            };
            serde_json::json!({
                "from": from,
                "value": render_message_text(message),
            })
        })
        .collect::<Vec<_>>();

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "id": session.session_id.to_string(),
        "system": session.objective,
        "conversations": conversations,
    }))?)
}
