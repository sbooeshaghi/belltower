//! Slash-command dispatch and read-only command rendering for the TUI.
//!
//! This module maps operator slash commands to durable command actions and
//! renders command output that is recorded back into session history. Session
//! mutation/control helpers live in `session_control`; the background
//! pending-command queue that keeps network commands off the event loop lives
//! in `pending`.

pub(crate) mod pending;
mod session_control;

use super::*;
use std::{fs, path::PathBuf};

fn export_usage_line() -> &'static str {
    "Usage: /export [legacy-bundle|jsonl|html|sharegpt|otlp] <path>"
}

fn spawn_usage_line() -> &'static str {
    "Usage: /spawn [--connection <id>] [--model <model-id>] <objective>"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnCommandArgs {
    pub(crate) objective: String,
    pub(crate) connection_id: Option<ConnectionId>,
    pub(crate) model_id: Option<String>,
}

pub(crate) fn parse_spawn_command_args(args: &[&str]) -> Result<SpawnCommandArgs, String> {
    let mut connection_id = None;
    let mut model_id = None;
    let mut objective_parts = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index] {
            "--connection" | "-c" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(spawn_usage_line().to_owned());
                };
                if value.starts_with('-') {
                    return Err(spawn_usage_line().to_owned());
                }
                connection_id = Some(ConnectionId::new(*value));
            }
            "--model" | "-m" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(spawn_usage_line().to_owned());
                };
                if value.starts_with('-') {
                    return Err(spawn_usage_line().to_owned());
                }
                model_id = Some((*value).to_owned());
            }
            "--" => {
                objective_parts.extend(args[index + 1..].iter().copied());
                break;
            }
            option if option.starts_with('-') => {
                return Err(format!(
                    "Unknown spawn option `{option}`. {}",
                    spawn_usage_line()
                ));
            }
            part => {
                objective_parts.push(part);
                objective_parts.extend(args[index + 1..].iter().copied());
                break;
            }
        }
        index += 1;
    }

    let objective = objective_parts.join(" ").trim().to_owned();
    if objective.is_empty() {
        return Err(spawn_usage_line().to_owned());
    }

    Ok(SpawnCommandArgs {
        objective,
        connection_id,
        model_id,
    })
}

impl ChatApp {
    pub(super) async fn handle_command(&mut self, command: &str) -> Result<(), Box<dyn Error>> {
        let trimmed = command.trim();
        let mut parts = trimmed.split_whitespace();
        let Some(head) = parts.next() else {
            return Ok(());
        };

        let command = match resolve_command(head) {
            Ok(command) => command,
            Err(message) => {
                self.enqueue_command_feedback(Some(trimmed), message, false);
                return Ok(());
            }
        };

        match command.canonical {
            "help" => match parts.next() {
                None => {
                    self.render_help_command(trimmed, false);
                }
                Some("all") => {
                    if parts.next().is_some() {
                        self.enqueue_command_feedback(Some(trimmed), "Usage: /help [all]", false);
                        return Ok(());
                    }
                    self.render_help_command(trimmed, true);
                }
                Some(other) => {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!("Unknown help scope `{other}`. Use /help or /help all."),
                        false,
                    );
                    return Ok(());
                }
            },
            "status" => {
                self.render_status_command(trimmed);
            }
            "models" => {
                self.render_models_command(trimmed);
            }
            "doctor" => {
                self.render_doctor_command(trimmed);
            }
            "mcp" => match parts.next() {
                Some("reload") => {
                    self.render_mcp_command(trimmed, true);
                }
                Some(other) => {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!("Unknown mcp action `{other}`. Use /mcp or /mcp reload."),
                        false,
                    );
                    return Ok(());
                }
                None => {
                    self.render_mcp_command(trimmed, false);
                }
            },
            "session" => {
                self.render_session_command(trimmed);
            }
            "inspect" => match parts.next() {
                None | Some("session") => {
                    self.render_session_command(trimmed);
                }
                Some("tool") => {
                    let Some(raw_call_id) = parts.next() else {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            "Usage: /inspect tool <call-id>",
                            false,
                        );
                        return Ok(());
                    };
                    self.render_inspect_tool_command(trimmed, ToolCallId::new(raw_call_id));
                }
                Some(other) => {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!(
                            "Unknown inspect target `{other}`. Use /inspect, /inspect session, or /inspect tool <call-id>."
                        ),
                        false,
                    );
                    return Ok(());
                }
            },
            "history" => {
                let limit = match parts.next() {
                    Some(value) => match value.parse::<usize>() {
                        Ok(limit) => limit,
                        Err(error) => {
                            self.enqueue_command_feedback(
                                Some(trimmed),
                                format!("invalid history limit: {error}"),
                                false,
                            );
                            return Ok(());
                        }
                    },
                    None => 20,
                };
                self.render_history_command(trimmed, limit);
            }
            "compact" => {
                self.render_compact_command(trimmed);
            }
            "execution" => {
                let limit = match parts.next() {
                    Some(value) => match value.parse::<usize>() {
                        Ok(limit) => limit,
                        Err(error) => {
                            self.enqueue_command_feedback(
                                Some(trimmed),
                                format!("invalid execution limit: {error}"),
                                false,
                            );
                            return Ok(());
                        }
                    },
                    None => 10,
                };
                self.render_execution_command(trimmed, limit);
            }
            "raw" => {
                let args = parts.collect::<Vec<_>>();
                let (mode, turn_selection, llm_call_ordinal, limit) =
                    match parse_raw_command_args(&args) {
                        Ok(parsed) => parsed,
                        Err(message) => {
                            self.enqueue_command_feedback(Some(trimmed), message, false);
                            return Ok(());
                        }
                    };
                self.render_raw_command(trimmed, mode, turn_selection, llm_call_ordinal, limit);
            }
            "usage" => {
                let limit = match parts.next() {
                    Some(value) => match value.parse::<usize>() {
                        Ok(limit) => limit,
                        Err(error) => {
                            self.enqueue_command_feedback(
                                Some(trimmed),
                                format!("invalid usage limit: {error}"),
                                false,
                            );
                            return Ok(());
                        }
                    },
                    None => 10,
                };
                self.render_usage_command(trimmed, limit);
            }
            "queue" => match parts.next() {
                Some("clear") => {
                    self.clear_queue_command(trimmed);
                }
                Some(other) => {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!("Unknown queue action `{other}`. Use /queue or /queue clear."),
                        false,
                    );
                    return Ok(());
                }
                None => {
                    self.render_queue_command(trimmed);
                }
            },
            "branches" => {
                self.render_branches_command(trimmed);
            }
            "tree" => {
                self.render_tree_command(trimmed);
            }
            "lineage" => {
                self.render_lineage_command(trimmed);
            }
            "workflow" => {
                self.render_workflow_command(trimmed);
            }
            "export" => {
                let format = match parts.next() {
                    Some(format @ ("legacy-bundle" | "jsonl" | "html" | "sharegpt" | "otlp")) => {
                        format
                    }
                    Some(other) => {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            format!(
                                "unknown export format `{other}`. Use legacy-bundle, jsonl, html, sharegpt, or otlp."
                            ),
                            false,
                        );
                        return Ok(());
                    }
                    None => {
                        self.prompt_export_path("jsonl");
                        return Ok(());
                    }
                };
                if format == "otlp" {
                    match parts.next() {
                        Some("push") => {
                            let endpoint = parts.next().map(str::to_owned);
                            let project_name = parts.next().map(str::to_owned);
                            self.render_otlp_push_command(trimmed, endpoint, project_name);
                        }
                        Some(output_path) => {
                            if parts.next().is_some() {
                                self.enqueue_command_feedback(
                                    Some(trimmed),
                                    export_usage_line(),
                                    false,
                                );
                                return Ok(());
                            }
                            self.render_export_command(trimmed, format, output_path);
                        }
                        None => {
                            self.prompt_export_path(format);
                        }
                    }
                } else {
                    match parts.next() {
                        Some(output_path) => {
                            if parts.next().is_some() {
                                self.enqueue_command_feedback(
                                    Some(trimmed),
                                    export_usage_line(),
                                    false,
                                );
                                return Ok(());
                            }
                            self.render_export_command(trimmed, format, output_path);
                        }
                        None => self.prompt_export_path(format),
                    }
                }
            }
            "refresh" => {
                self.request_transcript_refresh(Some(trimmed));
            }
            "new" => {
                self.create_session(Some(trimmed));
            }
            "resume" => {
                let Some(selection) = parts.next() else {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        "Use /resume <session-id> or pick a session below.",
                        false,
                    );
                    return Ok(());
                };
                let session =
                    match resolve_resume_selection(&self.sessions, &self.project_root, selection) {
                        Ok(session) => session.clone(),
                        Err(message) => {
                            self.enqueue_command_feedback(Some(trimmed), message, false);
                            return Ok(());
                        }
                    };
                self.start_resume_session(session, Some(trimmed.to_owned()))
                    .await?;
            }
            "spawn" => {
                let args = parts.collect::<Vec<_>>();
                let spawn_args = match parse_spawn_command_args(&args) {
                    Ok(args) => args,
                    Err(message) => {
                        self.enqueue_command_feedback(Some(trimmed), message, false);
                        return Ok(());
                    }
                };
                self.render_spawn_command(trimmed, spawn_args);
            }
            "branch" => {
                self.create_branch(Some(trimmed));
            }
            "cancel" => {
                self.cancel_active_turn(Some(trimmed));
            }
            "detach" => {
                self.enqueue_detach_command(trimmed);
            }
            "steer" => {
                let message = parts.collect::<Vec<_>>().join(" ").trim().to_owned();
                if message.is_empty() {
                    self.enqueue_command_feedback(Some(trimmed), "Usage: /steer <message>", false);
                    return Ok(());
                }
                self.steer_active_turn(message, Some(trimmed));
            }
            "approve" => {
                let scope = match parse_approval_scope(parts.next()) {
                    Ok(scope) => scope,
                    Err(message) => {
                        self.enqueue_command_feedback(Some(trimmed), message, false);
                        return Ok(());
                    }
                };
                self.start_resolve_last_pending(true, scope, Some(trimmed))
                    .await?;
            }
            "deny" => {
                let scope = match parse_approval_scope(parts.next()) {
                    Ok(scope) => scope,
                    Err(message) => {
                        self.enqueue_command_feedback(Some(trimmed), message, false);
                        return Ok(());
                    }
                };
                self.start_resolve_last_pending(false, scope, Some(trimmed))
                    .await?;
            }
            "connection" => {
                let Some(connection) = parts.next() else {
                    let known = self
                        .connections
                        .iter()
                        .map(|connection| connection.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!(
                            "Current connection: {}. Available: {}",
                            self.connection_id, known
                        ),
                        true,
                    );
                    return Ok(());
                };
                let connection_id = ConnectionId::new(connection);
                self.apply_session_settings(Some(connection_id), None, None, true, Some(trimmed));
            }
            "model" => {
                let Some(model_id) = parts.next() else {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!(
                            "Current model: {}. Use /model <model-id> or /use <connection> [model].",
                            self.effective_model()
                        ),
                        true,
                    );
                    return Ok(());
                };
                self.apply_session_settings(
                    None,
                    Some(model_id.to_owned()),
                    None,
                    false,
                    Some(trimmed),
                );
            }
            "defaults" => match parts.next() {
                None => {
                    self.render_defaults_command(trimmed)?;
                }
                Some("connection") => {
                    let Some(connection) = parts.next() else {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            "Usage: /defaults connection <connection-id>",
                            false,
                        );
                        return Ok(());
                    };
                    if parts.next().is_some() {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            "Usage: /defaults connection <connection-id>",
                            false,
                        );
                        return Ok(());
                    }
                    self.persist_default_connection_command(
                        ConnectionId::new(connection),
                        Some(trimmed),
                    );
                }
                Some("model") => {
                    let Some(model_id) = parts.next() else {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            "Usage: /defaults model <model-id>",
                            false,
                        );
                        return Ok(());
                    };
                    if parts.next().is_some() {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            "Usage: /defaults model <model-id>",
                            false,
                        );
                        return Ok(());
                    }
                    self.persist_default_model_command(
                        self.connection_id.clone(),
                        model_id.to_owned(),
                        Some(trimmed),
                    );
                }
                Some("use") => {
                    let Some(connection) = parts.next() else {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            "Usage: /defaults use <connection-id> [model-id]",
                            false,
                        );
                        return Ok(());
                    };
                    let model_id = parts.next().map(ToOwned::to_owned);
                    if parts.next().is_some() {
                        self.enqueue_command_feedback(
                            Some(trimmed),
                            "Usage: /defaults use <connection-id> [model-id]",
                            false,
                        );
                        return Ok(());
                    }
                    self.persist_defaults_use_command(
                        ConnectionId::new(connection),
                        model_id,
                        Some(trimmed),
                    );
                }
                Some(other) => {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!(
                            "Unknown defaults action `{other}`. Use /defaults, /defaults connection <connection-id>, /defaults model <model-id>, or /defaults use <connection-id> [model-id]."
                        ),
                        false,
                    );
                    return Ok(());
                }
            },
            "mode" => {
                let Some(mode) = parts.next() else {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        format!(
                            "Current tool mode: {}. Use /mode <standard|extended>.",
                            render_tool_mode(self.tool_mode)
                        ),
                        false,
                    );
                    return Ok(());
                };
                let Ok(tool_mode) = parse_tool_mode(mode) else {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        "Unknown tool mode. Use standard or extended.",
                        false,
                    );
                    return Ok(());
                };
                self.apply_session_settings(None, None, Some(tool_mode), false, Some(trimmed));
            }
            "use" => {
                let Some(connection) = parts.next() else {
                    self.enqueue_command_feedback(
                        Some(trimmed),
                        "Usage: /use <connection-id> [model-id]",
                        false,
                    );
                    return Ok(());
                };
                let connection_id = ConnectionId::new(connection);
                let model_id = parts.next().map(ToOwned::to_owned);
                let reset_model_to_default = model_id.is_none();
                self.apply_session_settings(
                    Some(connection_id),
                    model_id,
                    None,
                    reset_model_to_default,
                    Some(trimmed),
                );
            }
            _ => {
                self.enqueue_command_feedback(
                    Some(trimmed),
                    format!("Unknown command `{head}`. Use /help."),
                    false,
                );
                return Ok(());
            }
        }

        Ok(())
    }

    pub(super) fn render_help_command(&mut self, raw_input: &str, show_all: bool) {
        let output = command_help_output(show_all);
        self.enqueue_recorded_command_output(raw_input, output);
    }

    pub(super) fn render_defaults_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let config = BelltowerConfig::load(None)?;
        let output = render_defaults_output(&config, &self.connection_id, self.effective_model());
        self.enqueue_recorded_command_output(raw_input, output);
        Ok(())
    }

    pub(super) fn render_status_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let connection_id = self.connection_id.clone();
        let model = self.effective_model().to_owned();
        let tool_mode = self.tool_mode;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .status_inspection()
                .await
                .map_err(|error| error.to_string())?;
            let output =
                render_status_output(&response.inspection, &connection_id, &model, tool_mode);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::ReadinessStatus {
                inspection: response.inspection,
            })
        });
    }

    pub(super) fn render_models_command(&mut self, raw_input: &str) {
        let mut parts = raw_input.split_whitespace();
        let _command = parts.next();
        let target_connection = match (parts.next(), parts.next()) {
            (None, None) => None,
            (Some(connection), None) => Some(ConnectionId::new(connection)),
            (Some(_), Some(_)) => {
                self.enqueue_command_feedback(
                    Some(raw_input),
                    "Usage: /models [connection-id]",
                    false,
                );
                return;
            }
            (None, Some(_)) => unreachable!("split_whitespace cannot yield a second token first"),
        };
        if let Some(connection_id) = target_connection.as_ref()
            && !self.connection_exists(connection_id)
        {
            self.enqueue_command_feedback(
                Some(raw_input),
                format!("Unknown connection `{connection_id}`."),
                false,
            );
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let connection_id = self.connection_id.clone();
        let model = self.effective_model().to_owned();
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let backends = client
                .model_backends()
                .await
                .map_err(|error| error.to_string())?;
            let connections = if let Some(target) = target_connection.as_ref() {
                client
                    .connection_model_inventory(target)
                    .await
                    .map_err(|error| error.to_string())?
            } else {
                client
                    .connection_models()
                    .await
                    .map_err(|error| error.to_string())?
            };
            let recommendations = client
                .model_recommendations()
                .await
                .map_err(|error| error.to_string())?;
            let output = render_models_output(
                &connections.connections,
                &backends.backends,
                &recommendations.report,
                &connection_id,
                &model,
            );
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::ModelInventory {
                backends: backends.backends,
                connections: connections.connections,
                merge: target_connection.is_some(),
            })
        });
    }

    pub(super) fn render_doctor_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let connection_id = self.connection_id.clone();
        let model = self.effective_model().to_owned();
        let tool_mode = self.tool_mode;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let health = client.health().await.map_err(|error| error.to_string())?;
            let (status, backends, models) = tokio::try_join!(
                client.status_inspection(),
                client.model_backends(),
                client.connection_models()
            )
            .map_err(|error| error.to_string())?;
            let inventory = client
                .mcp_inventory()
                .await
                .map_err(|error| error.to_string())?;
            let output = render_doctor_output(
                &health,
                &status.inspection,
                &models.connections,
                &backends.backends,
                &inventory.servers,
                &inventory.tools,
                &connection_id,
                &model,
                tool_mode,
            );
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Doctor {
                status: status.inspection,
                backends: backends.backends,
                connections: models.connections,
                servers: inventory.servers,
                tools: inventory.tools,
            })
        });
    }

    pub(super) fn render_mcp_command(&mut self, raw_input: &str, reload: bool) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            if reload {
                client
                    .reload_mcp()
                    .await
                    .map_err(|error| error.to_string())?;
            }
            let inventory = client
                .mcp_inventory()
                .await
                .map_err(|error| error.to_string())?;
            let output = render_mcp_output(&inventory.servers, &inventory.tools, reload);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::McpInventory {
                servers: inventory.servers,
                tools: inventory.tools,
            })
        });
    }

    pub(super) fn render_spawn_command(&mut self, raw_input: &str, args: SpawnCommandArgs) {
        if let Some(connection_id) = args.connection_id.as_ref()
            && !self.connection_exists(connection_id)
        {
            self.enqueue_command_feedback(
                Some(raw_input),
                format!("Unknown connection `{connection_id}`."),
                false,
            );
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let turns = client
                .session_turns(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let origin_turn_id = turns
                .turns
                .iter()
                .rev()
                .find(|turn| turn.branch_id == branch_id)
                .map(|turn| turn.turn_id);
            match client
                .spawn_session(
                    session_id,
                    &SpawnSessionRequest {
                        dispatch: None,
                        parent_branch_id: branch_id,
                        parent_turn_id: origin_turn_id,
                        objective: args.objective,
                        display_name: None,
                        connection_id: args.connection_id,
                        model_id: args.model_id,
                    },
                )
                .await
            {
                Ok(response) => {
                    let output = render_spawn_output(&response);
                    record_slash_command(&client, session_id, &raw, output, true).await?;
                    Ok(CommandOutcome::ChildSpawned)
                }
                Err(error) => {
                    let message = format!("Failed to spawn child session: {error}");
                    record_slash_command(&client, session_id, &raw, message.clone(), false).await?;
                    Ok(CommandOutcome::Notice(message))
                }
            }
        });
    }

    pub(super) fn render_session_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .inspect_session(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_session_output(&response.inspection);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_history_command(&mut self, raw_input: &str, limit: usize) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_turns(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_turn_history_output(&response.turns, limit);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_inspect_tool_command(&mut self, raw_input: &str, call_id: ToolCallId) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_tool_call(session_id, &call_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_tool_call_inspection_output(&response.inspection);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_compact_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .compact_session(session_id, &CompactSessionRequest { branch_id })
                .await
                .map_err(|error| error.to_string())?;
            let output = render_compaction_output(
                short_id_string(&response.branch_id.to_string()),
                &response.model_id,
                response.compaction.as_ref(),
            );
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_execution_command(&mut self, raw_input: &str, limit: usize) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_execution(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_execution_output(&response.inspection, limit);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_raw_command(
        &mut self,
        raw_input: &str,
        mode: RawCommandMode,
        turn_selection: Option<&str>,
        llm_call_ordinal: Option<u32>,
        limit: usize,
    ) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let turn_selection = turn_selection.map(str::to_owned);
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_turns(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let turn = match resolve_raw_turn_selection(
                &response.turns,
                &branch_id,
                turn_selection.as_deref(),
            ) {
                Ok(turn) => turn,
                Err(message) => {
                    record_slash_command(&client, session_id, &raw, message.clone(), false).await?;
                    return Ok(CommandOutcome::Notice(message));
                }
            };
            let page = client
                .turn_raw_chunks_page(
                    session_id,
                    turn.branch_id,
                    turn.turn_id,
                    llm_call_ordinal,
                    None,
                    limit.max(1),
                )
                .await
                .map_err(|error| error.to_string())?;
            let output = match mode {
                RawCommandMode::Chunks => {
                    render_raw_output(turn, &page.chunks, llm_call_ordinal, page.has_more_before)
                }
                RawCommandMode::Diff => {
                    let turn_events = load_turn_events_for_diff(&client, session_id, turn).await?;
                    let correlated =
                        correlate_raw_diff_events(&page.chunks, llm_call_ordinal, &turn_events);
                    render_raw_diff_output(
                        turn,
                        &page.chunks,
                        llm_call_ordinal,
                        page.has_more_before,
                        &correlated,
                    )
                }
            };
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_usage_command(&mut self, raw_input: &str, limit: usize) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_execution(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_usage_output(&response.inspection, limit);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_queue_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_queue(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_queue_output(&response.inspection);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn clear_queue_command(&mut self, raw_input: &str) {
        let mut dropped_pending_submissions = 0usize;
        while let Some(pending) = self.pending_message_submissions.pop_front() {
            pending.handle.abort();
            dropped_pending_submissions += 1;
        }
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .clear_session_queue(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_queue_clear_output(&response);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::QueueCleared {
                dropped_submissions: dropped_pending_submissions,
            })
        });
    }

    pub(super) fn render_branches_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .inspect_branches(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output =
                render_branches_output(response.active_branch_id.as_ref(), &response.branches);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_tree_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_tree(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_tree_output(&response.inspection);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_lineage_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_lineage(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_lineage_output(&response.inspection);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_workflow_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .session_workflow(session_id)
                .await
                .map_err(|error| error.to_string())?;
            let output = render_workflow_output(&response.inspection);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn render_export_command(
        &mut self,
        raw_input: &str,
        format: &str,
        output_path: &str,
    ) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let format = format.to_owned();
        let path = self.resolve_export_output_path(output_path);
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .export_session(session_id, &format)
                .await
                .map_err(|error| error.to_string())?;
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::write(&path, &response.content).map_err(|error| error.to_string())?;
            let path_label = path.display().to_string();
            let output = render_export_output(
                &format!("{:?}", response.format).to_ascii_lowercase(),
                &response.content_type,
                &response.content,
                Some(&path_label),
            );
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    pub(super) fn prompt_export_path(&mut self, format: &str) {
        self.composer.input = format!("/export {format} ");
        self.composer.cursor = self.composer.input.len();
        self.clear_composer_preferred_column();
        self.clear_input_history_browse();
        self.show_notice("Type an export path, then press Enter.");
    }

    fn resolve_export_output_path(&self, output_path: &str) -> PathBuf {
        let path = PathBuf::from(output_path);
        if path.is_absolute() {
            path
        } else {
            PathBuf::from(&self.project_root).join(path)
        }
    }

    pub(super) fn render_otlp_push_command(
        &mut self,
        raw_input: &str,
        endpoint: Option<String>,
        project_name: Option<String>,
    ) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            let response = client
                .push_otlp_export(
                    session_id,
                    &PushOtlpExportRequest {
                        endpoint,
                        project_name,
                        api_key: None,
                        headers: BTreeMap::new(),
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            let output = render_otlp_push_output(&response);
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    /// Reload the transcript off the event loop (`/refresh` and the refresh
    /// keybinding). The command records only when invoked as a slash command,
    /// matching the old inline behavior.
    pub(super) fn request_transcript_refresh(&mut self, raw_input: Option<&str>) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let load_full = self.messages.is_empty() && self.operator_commands.is_empty();
        let raw = raw_input.map(str::to_owned);
        let label = raw_input.map_or_else(|| "refresh".to_owned(), command_label);
        self.enqueue_pending_command(label, raw.clone(), async move {
            let page = if load_full {
                load_full_transcript(client.clone(), session_id, branch_id).await?
            } else {
                load_transcript_page(client.clone(), session_id, branch_id, None).await?
            };
            let notice = if let Some(raw) = raw.as_deref() {
                let message = "Refreshed session state.".to_owned();
                record_slash_command(&client, session_id, raw, message.clone(), true).await?;
                Some(message)
            } else {
                None
            };
            Ok(CommandOutcome::TranscriptRefreshed { page, notice })
        });
    }

    pub(super) fn enqueue_detach_command(&mut self, raw_input: &str) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        self.enqueue_pending_command(command_label(raw_input), Some(raw.clone()), async move {
            record_slash_command(
                &client,
                session_id,
                &raw,
                "Detached from the current TUI session.".to_owned(),
                true,
            )
            .await?;
            Ok(CommandOutcome::Detached)
        });
    }
}

/// Fetch the events belonging to a turn for `/raw diff` correlation.
///
/// Runs on the background command task, so it borrows only the cloned
/// client and owned identifiers.
pub(super) async fn load_turn_events_for_diff(
    client: &BelltowerClient,
    session_id: SessionId,
    turn: &TurnInspection,
) -> std::result::Result<Vec<EventEnvelope>, String> {
    let Some(event_seq_start) = turn.event_seq_start else {
        return Ok(Vec::new());
    };
    let mut after_seq_id = Some(event_seq_start.saturating_sub(1));
    let mut events = Vec::new();

    loop {
        let response = client
            .session_events(session_id, after_seq_id)
            .await
            .map_err(|error| error.to_string())?;
        if response.events.is_empty() {
            break;
        }

        let mut advanced = false;
        for event in response.events {
            let Some(seq_id) = event.seq_id else {
                continue;
            };
            after_seq_id = Some(seq_id);
            advanced = true;

            if seq_id < event_seq_start {
                continue;
            }

            if turn_event_matches(&event, turn) {
                events.push(event);
            }

            if turn
                .event_seq_end
                .is_some_and(|event_seq_end| seq_id >= event_seq_end)
            {
                return Ok(events);
            }
        }

        if !advanced {
            break;
        }

        if turn
            .event_seq_end
            .is_none_or(|event_seq_end| after_seq_id.is_some_and(|value| value < event_seq_end))
        {
            if response
                .last_seq_id
                .is_some_and(|last_seq_id| after_seq_id == Some(last_seq_id))
            {
                break;
            }
            continue;
        }

        break;
    }

    Ok(events)
}
