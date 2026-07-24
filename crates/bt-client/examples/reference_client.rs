#![forbid(unsafe_code)]

use bt_client::BelltowerClient;
use bt_core::{
    ApprovalDecision, ApprovalDecisionSource, ApprovalScope, CompletionDelta, ConnectionId,
    EventEnvelope, EventPayload, Message, Role,
};
use bt_protocol::{ApproveToolRequest, CreateSessionRequest, SendMessageRequest};
use futures_util::StreamExt;
use std::env;
use std::error::Error;
use std::io::{self, Write};
use time::OffsetDateTime;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = ExampleConfig::from_env()?;
    let client = BelltowerClient::new_with_auth_token(config.base_url.parse()?, config.auth_token)?;

    let info = client.server_info().await?;
    println!(
        "connected to belltower {} ({})",
        info.server_version, info.protocol_version
    );
    println!(
        "capabilities: approvals={} exports={}",
        info.capabilities.approvals, info.capabilities.exports
    );

    let created = client
        .create_session(&CreateSessionRequest {
            approval_mode: None,
            project_root: config.project_root,
            connection_id: ConnectionId::new(config.connection_id),
            model_id: config.model_id,
            tool_mode: None,
            display_name: Some("bt-client reference example".to_owned()),
            objective: Some("Demonstrate the remote bt-client consumer seam.".to_owned()),
            budget: None,
        })
        .await?;

    println!(
        "created session {} branch {}",
        created.session.session_id, created.branch.branch_id
    );

    let mut events = client
        .stream_events(created.session.session_id, None)
        .await?;
    let send = client
        .send_message(
            created.session.session_id,
            &SendMessageRequest {
                branch_id: created.branch.branch_id,
                message: Message::text(Role::User, config.prompt),
            },
        )
        .await?;
    println!("message outcome: {send:?}");

    while let Some(envelope) = events.next().await {
        let envelope = envelope?;
        let event: EventEnvelope = serde_json::from_value(envelope.data)?;
        print_event_summary(&event);
        if let EventPayload::ToolApprovalRequested {
            call_id, tool_name, ..
        } = &event.payload
        {
            let decision = ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "bt-client-reference-example".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            };
            client
                .approve_tool(
                    created.session.session_id,
                    &ApproveToolRequest {
                        call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                        scope: ApprovalScope::Once,
                        decision,
                    },
                )
                .await?;
            println!("approved tool request {tool_name} ({call_id})");
        }
        if matches!(event.payload, EventPayload::TurnFinished { .. }) {
            break;
        }
    }

    let export = client
        .export_session(created.session.session_id, "jsonl")
        .await?;
    println!(
        "exported {} as {:?} ({} bytes)",
        export.session_id,
        export.format,
        export.content.len()
    );

    Ok(())
}

struct ExampleConfig {
    base_url: String,
    auth_token: String,
    project_root: String,
    connection_id: String,
    model_id: Option<String>,
    prompt: String,
}

impl ExampleConfig {
    fn from_env() -> Result<Self, Box<dyn Error>> {
        let auth_token = env::var("BELLTOWER_AUTH_TOKEN").map_err(
            |_| "set BELLTOWER_AUTH_TOKEN to an explicit bearer token for the target server",
        )?;
        let project_root = match env::var("BELLTOWER_REFERENCE_PROJECT_ROOT") {
            Ok(value) => value,
            Err(_) => env::current_dir()?.display().to_string(),
        };
        Ok(Self {
            base_url: env::var("BELLTOWER_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:7400".to_owned()),
            auth_token,
            project_root,
            connection_id: env::var("BELLTOWER_REFERENCE_CONNECTION")
                .unwrap_or_else(|_| "local".to_owned()),
            model_id: env::var("BELLTOWER_REFERENCE_MODEL").ok(),
            prompt: env::var("BELLTOWER_REFERENCE_PROMPT")
                .unwrap_or_else(|_| "Say hello from the bt-client reference example.".to_owned()),
        })
    }
}

fn print_event_summary(event: &EventEnvelope) {
    match &event.payload {
        EventPayload::CompletionChunk { deltas, .. } => {
            for delta in deltas {
                match delta {
                    CompletionDelta::AppendText { text } => {
                        print!("{text}");
                        let _ = io::stdout().flush();
                    }
                    CompletionDelta::OpenToolCall { tool_name, .. } => {
                        println!("\nrequested tool: {tool_name}");
                    }
                    _ => {}
                }
            }
        }
        EventPayload::ToolExecutionFinished {
            tool_name, result, ..
        } => {
            println!(
                "\ntool finished: {tool_name} error={} output={}",
                result.is_error, result.output
            );
        }
        EventPayload::SessionError {
            class,
            code,
            message,
            ..
        } => {
            println!("\nsession error: {class}:{code}: {message}");
        }
        EventPayload::TurnFinished {
            status,
            finish_reason,
            ..
        } => {
            println!("\nturn finished: {status} {finish_reason:?}");
        }
        _ => {}
    }
}
