//! Slash-command dispatch and read-only command rendering for the TUI.
//!
//! This module maps operator slash commands to durable command actions and
//! renders command output that is recorded back into session history. Session
//! mutation/control helpers live in `session_control`.

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
                return self
                    .maybe_record_command_feedback(Some(trimmed), message, false)
                    .await;
            }
        };

        match command.canonical {
            "help" => match parts.next() {
                None => {
                    self.render_help_command(trimmed, false).await?;
                }
                Some("all") => {
                    if parts.next().is_some() {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /help [all]",
                                false,
                            )
                            .await;
                    }
                    self.render_help_command(trimmed, true).await?;
                }
                Some(other) => {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!("Unknown help scope `{other}`. Use /help or /help all."),
                            false,
                        )
                        .await;
                }
            },
            "status" => {
                self.render_status_command(trimmed).await?;
            }
            "models" => {
                self.render_models_command(trimmed).await?;
            }
            "doctor" => {
                self.render_doctor_command(trimmed).await?;
            }
            "mcp" => match parts.next() {
                Some("reload") => {
                    self.render_mcp_command(trimmed, true).await?;
                }
                Some(other) => {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!("Unknown mcp action `{other}`. Use /mcp or /mcp reload."),
                            false,
                        )
                        .await;
                }
                None => {
                    self.render_mcp_command(trimmed, false).await?;
                }
            },
            "session" => {
                self.render_session_command(trimmed).await?;
            }
            "inspect" => match parts.next() {
                None | Some("session") => {
                    self.render_session_command(trimmed).await?;
                }
                Some("tool") => {
                    let Some(raw_call_id) = parts.next() else {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /inspect tool <call-id>",
                                false,
                            )
                            .await;
                    };
                    self.render_inspect_tool_command(trimmed, ToolCallId::new(raw_call_id))
                        .await?;
                }
                Some(other) => {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!(
                                "Unknown inspect target `{other}`. Use /inspect, /inspect session, or /inspect tool <call-id>."
                            ),
                            false,
                        )
                        .await;
                }
            },
            "history" => {
                let limit = match parts.next() {
                    Some(value) => match value.parse::<usize>() {
                        Ok(limit) => limit,
                        Err(error) => {
                            return self
                                .maybe_record_command_feedback(
                                    Some(trimmed),
                                    format!("invalid history limit: {error}"),
                                    false,
                                )
                                .await;
                        }
                    },
                    None => 20,
                };
                self.render_history_command(trimmed, limit).await?;
            }
            "compact" => {
                self.render_compact_command(trimmed).await?;
            }
            "execution" => {
                let limit = match parts.next() {
                    Some(value) => match value.parse::<usize>() {
                        Ok(limit) => limit,
                        Err(error) => {
                            return self
                                .maybe_record_command_feedback(
                                    Some(trimmed),
                                    format!("invalid execution limit: {error}"),
                                    false,
                                )
                                .await;
                        }
                    },
                    None => 10,
                };
                self.render_execution_command(trimmed, limit).await?;
            }
            "raw" => {
                let args = parts.collect::<Vec<_>>();
                let (mode, turn_selection, llm_call_ordinal, limit) =
                    match parse_raw_command_args(&args) {
                        Ok(parsed) => parsed,
                        Err(message) => {
                            return self
                                .maybe_record_command_feedback(Some(trimmed), message, false)
                                .await;
                        }
                    };
                self.render_raw_command(trimmed, mode, turn_selection, llm_call_ordinal, limit)
                    .await?;
            }
            "usage" => {
                let limit = match parts.next() {
                    Some(value) => match value.parse::<usize>() {
                        Ok(limit) => limit,
                        Err(error) => {
                            return self
                                .maybe_record_command_feedback(
                                    Some(trimmed),
                                    format!("invalid usage limit: {error}"),
                                    false,
                                )
                                .await;
                        }
                    },
                    None => 10,
                };
                self.render_usage_command(trimmed, limit).await?;
            }
            "queue" => match parts.next() {
                Some("clear") => {
                    self.clear_queue_command(trimmed).await?;
                }
                Some(other) => {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!("Unknown queue action `{other}`. Use /queue or /queue clear."),
                            false,
                        )
                        .await;
                }
                None => {
                    self.render_queue_command(trimmed).await?;
                }
            },
            "branches" => {
                self.render_branches_command(trimmed).await?;
            }
            "tree" => {
                self.render_tree_command(trimmed).await?;
            }
            "lineage" => {
                self.render_lineage_command(trimmed).await?;
            }
            "workflow" => {
                self.render_workflow_command(trimmed).await?;
            }
            "export" => {
                let format = match parts.next() {
                    Some(format @ ("legacy-bundle" | "jsonl" | "html" | "sharegpt" | "otlp")) => {
                        format
                    }
                    Some(other) => {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                format!(
                                    "unknown export format `{other}`. Use legacy-bundle, jsonl, html, sharegpt, or otlp."
                                ),
                                false,
                            )
                            .await;
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
                            self.render_otlp_push_command(trimmed, endpoint, project_name)
                                .await?;
                        }
                        Some(output_path) => {
                            if parts.next().is_some() {
                                return self
                                    .maybe_record_command_feedback(
                                        Some(trimmed),
                                        export_usage_line(),
                                        false,
                                    )
                                    .await;
                            }
                            self.render_export_command(trimmed, format, output_path)
                                .await?;
                        }
                        None => {
                            self.prompt_export_path(format);
                        }
                    }
                } else {
                    match parts.next() {
                        Some(output_path) => {
                            if parts.next().is_some() {
                                return self
                                    .maybe_record_command_feedback(
                                        Some(trimmed),
                                        export_usage_line(),
                                        false,
                                    )
                                    .await;
                            }
                            self.render_export_command(trimmed, format, output_path)
                                .await?;
                        }
                        None => self.prompt_export_path(format),
                    }
                }
            }
            "refresh" => {
                self.refresh().await?;
                self.maybe_record_command_feedback(Some(trimmed), "Refreshed session state.", true)
                    .await?;
            }
            "new" => {
                self.create_session(Some(trimmed)).await?;
            }
            "resume" => {
                let Some(selection) = parts.next() else {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            "Use /resume <session-id> or pick a session below.",
                            false,
                        )
                        .await;
                };
                let session =
                    match resolve_resume_selection(&self.sessions, &self.project_root, selection) {
                        Ok(session) => session.clone(),
                        Err(message) => {
                            return self
                                .maybe_record_command_feedback(Some(trimmed), message, false)
                                .await;
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
                        return self
                            .maybe_record_command_feedback(Some(trimmed), message, false)
                            .await;
                    }
                };
                self.render_spawn_command(trimmed, spawn_args).await?;
            }
            "branch" => {
                self.create_branch(Some(trimmed)).await?;
            }
            "cancel" => {
                self.cancel_active_turn(Some(trimmed)).await?;
            }
            "detach" => {
                self.record_operator_command(
                    trimmed,
                    "Detached from the current TUI session.".to_owned(),
                )
                .await?;
                self.detach_requested = true;
                self.should_quit = true;
            }
            "steer" => {
                let message = parts.collect::<Vec<_>>().join(" ").trim().to_owned();
                if message.is_empty() {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            "Usage: /steer <message>",
                            false,
                        )
                        .await;
                }
                self.steer_active_turn(message, Some(trimmed)).await?;
            }
            "approve" => {
                let scope = match parse_approval_scope(parts.next()) {
                    Ok(scope) => scope,
                    Err(message) => {
                        return self
                            .maybe_record_command_feedback(Some(trimmed), message, false)
                            .await;
                    }
                };
                self.start_resolve_last_pending(true, scope, Some(trimmed))
                    .await?;
            }
            "deny" => {
                let scope = match parse_approval_scope(parts.next()) {
                    Ok(scope) => scope,
                    Err(message) => {
                        return self
                            .maybe_record_command_feedback(Some(trimmed), message, false)
                            .await;
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
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!(
                                "Current connection: {}. Available: {}",
                                self.connection_id, known
                            ),
                            true,
                        )
                        .await;
                };
                let connection_id = ConnectionId::new(connection);
                self.apply_session_settings(Some(connection_id), None, None, true, Some(trimmed))
                    .await?;
            }
            "model" => {
                let Some(model_id) = parts.next() else {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!(
                                "Current model: {}. Use /model <model-id> or /use <connection> [model].",
                                self.effective_model()
                            ),
                            true,
                        )
                        .await;
                };
                self.apply_session_settings(
                    None,
                    Some(model_id.to_owned()),
                    None,
                    false,
                    Some(trimmed),
                )
                .await?;
            }
            "defaults" => match parts.next() {
                None => {
                    self.render_defaults_command(trimmed).await?;
                }
                Some("connection") => {
                    let Some(connection) = parts.next() else {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /defaults connection <connection-id>",
                                false,
                            )
                            .await;
                    };
                    if parts.next().is_some() {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /defaults connection <connection-id>",
                                false,
                            )
                            .await;
                    }
                    self.persist_default_connection_command(
                        ConnectionId::new(connection),
                        Some(trimmed),
                    )
                    .await?;
                }
                Some("model") => {
                    let Some(model_id) = parts.next() else {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /defaults model <model-id>",
                                false,
                            )
                            .await;
                    };
                    if parts.next().is_some() {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /defaults model <model-id>",
                                false,
                            )
                            .await;
                    }
                    self.persist_default_model_command(
                        self.connection_id.clone(),
                        model_id.to_owned(),
                        Some(trimmed),
                    )
                    .await?;
                }
                Some("use") => {
                    let Some(connection) = parts.next() else {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /defaults use <connection-id> [model-id]",
                                false,
                            )
                            .await;
                    };
                    let model_id = parts.next().map(ToOwned::to_owned);
                    if parts.next().is_some() {
                        return self
                            .maybe_record_command_feedback(
                                Some(trimmed),
                                "Usage: /defaults use <connection-id> [model-id]",
                                false,
                            )
                            .await;
                    }
                    self.persist_defaults_use_command(
                        ConnectionId::new(connection),
                        model_id,
                        Some(trimmed),
                    )
                    .await?;
                }
                Some(other) => {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!(
                                "Unknown defaults action `{other}`. Use /defaults, /defaults connection <connection-id>, /defaults model <model-id>, or /defaults use <connection-id> [model-id]."
                            ),
                            false,
                        )
                        .await;
                }
            },
            "mode" => {
                let Some(mode) = parts.next() else {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            format!(
                                "Current tool mode: {}. Use /mode <standard|extended>.",
                                render_tool_mode(self.tool_mode)
                            ),
                            false,
                        )
                        .await;
                };
                let Ok(tool_mode) = parse_tool_mode(mode) else {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            "Unknown tool mode. Use standard or extended.",
                            false,
                        )
                        .await;
                };
                self.apply_session_settings(None, None, Some(tool_mode), false, Some(trimmed))
                    .await?;
            }
            "use" => {
                let Some(connection) = parts.next() else {
                    return self
                        .maybe_record_command_feedback(
                            Some(trimmed),
                            "Usage: /use <connection-id> [model-id]",
                            false,
                        )
                        .await;
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
                )
                .await?;
            }
            _ => {
                return self
                    .maybe_record_command_feedback(
                        Some(trimmed),
                        format!("Unknown command `{head}`. Use /help."),
                        false,
                    )
                    .await;
            }
        }

        Ok(())
    }

    pub(super) async fn render_help_command(
        &mut self,
        raw_input: &str,
        show_all: bool,
    ) -> Result<(), Box<dyn Error>> {
        let output = command_help_output(show_all);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_defaults_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let config = BelltowerConfig::load(None)?;
        let output = render_defaults_output(&config, &self.connection_id, self.effective_model());
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_status_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.status_inspection().await?;
        self.connection_status = Some(response.inspection.clone());
        self.last_readiness_refresh = Instant::now();
        let output = render_status_output(
            &response.inspection,
            &self.connection_id,
            self.effective_model(),
            self.tool_mode,
        );
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_models_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let mut parts = raw_input.split_whitespace();
        let _command = parts.next();
        let target_connection = match (parts.next(), parts.next()) {
            (None, None) => None,
            (Some(connection), None) => Some(ConnectionId::new(connection)),
            (Some(_), Some(_)) => {
                return self
                    .maybe_record_command_feedback(
                        Some(raw_input),
                        "Usage: /models [connection-id]",
                        false,
                    )
                    .await;
            }
            (None, Some(_)) => unreachable!("split_whitespace cannot yield a second token first"),
        };
        if let Some(connection_id) = target_connection.as_ref()
            && !self.connection_exists(connection_id)
        {
            return self
                .maybe_record_command_feedback(
                    Some(raw_input),
                    format!("Unknown connection `{connection_id}`."),
                    false,
                )
                .await;
        }

        let backends = self.client.model_backends().await?;
        self.model_backends = backends.backends.clone();
        let connections = if let Some(connection_id) = target_connection.as_ref() {
            self.client
                .connection_model_inventory(connection_id)
                .await?
        } else {
            self.client.connection_models().await?
        };
        if target_connection.is_some() {
            self.merge_connection_model_inventories(connections.connections.clone());
        } else {
            self.connection_models = connections.connections.clone();
        }
        self.last_readiness_refresh = Instant::now();
        let recommendations = self.client.model_recommendations().await?;
        let rendered_connections = if target_connection.is_some() {
            connections.connections.as_slice()
        } else {
            self.connection_models.as_slice()
        };
        let output = render_models_output(
            rendered_connections,
            &backends.backends,
            &recommendations.report,
            &self.connection_id,
            self.effective_model(),
        );
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_doctor_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let health = self.client.health().await?;
        self.refresh_readiness_cache(true).await?;
        self.refresh_mcp_cache(true).await?;
        let output = render_doctor_output(
            &health,
            self.connection_status
                .as_ref()
                .expect("readiness cache should be populated"),
            &self.connection_models,
            &self.model_backends,
            &self.mcp_servers,
            &self.mcp_tools,
            &self.connection_id,
            self.effective_model(),
            self.tool_mode,
        );
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_mcp_command(
        &mut self,
        raw_input: &str,
        reload: bool,
    ) -> Result<(), Box<dyn Error>> {
        if reload {
            self.client.reload_mcp().await?;
        }
        self.refresh_mcp_cache(true).await?;
        let output = render_mcp_output(&self.mcp_servers, &self.mcp_tools, reload);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_spawn_command(
        &mut self,
        raw_input: &str,
        args: SpawnCommandArgs,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(connection_id) = args.connection_id.as_ref()
            && !self.connection_exists(connection_id)
        {
            return self
                .maybe_record_command_feedback(
                    Some(raw_input),
                    format!("Unknown connection `{connection_id}`."),
                    false,
                )
                .await;
        }

        let turns = self.client.session_turns(self.session_id).await?;
        let origin_turn_id = turns
            .turns
            .iter()
            .rev()
            .find(|turn| turn.branch_id == self.branch_id)
            .map(|turn| turn.turn_id);
        let response = match self
            .client
            .spawn_session(
                self.session_id,
                &SpawnSessionRequest {
                    parent_branch_id: self.branch_id,
                    parent_turn_id: origin_turn_id,
                    objective: args.objective,
                    display_name: None,
                    connection_id: args.connection_id,
                    model_id: args.model_id,
                },
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return self
                    .maybe_record_command_feedback(
                        Some(raw_input),
                        format!("Failed to spawn child session: {error}"),
                        false,
                    )
                    .await;
            }
        };
        self.refresh_metadata().await?;
        let output = render_spawn_output(&response);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_session_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.inspect_session(self.session_id).await?;
        let output = render_session_output(&response.inspection);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_history_command(
        &mut self,
        raw_input: &str,
        limit: usize,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_turns(self.session_id).await?;
        let output = render_turn_history_output(&response.turns, limit);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_inspect_tool_command(
        &mut self,
        raw_input: &str,
        call_id: ToolCallId,
    ) -> Result<(), Box<dyn Error>> {
        let response = self
            .client
            .session_tool_call(self.session_id, &call_id)
            .await?;
        let output = render_tool_call_inspection_output(&response.inspection);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_compact_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self
            .client
            .compact_session(
                self.session_id,
                &CompactSessionRequest {
                    branch_id: self.branch_id,
                },
            )
            .await?;
        let output = render_compaction_output(
            short_id_string(&response.branch_id.to_string()),
            &response.model_id,
            response.compaction.as_ref(),
        );
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_execution_command(
        &mut self,
        raw_input: &str,
        limit: usize,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_execution(self.session_id).await?;
        let output = render_execution_output(&response.inspection, limit);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_raw_command(
        &mut self,
        raw_input: &str,
        mode: RawCommandMode,
        turn_selection: Option<&str>,
        llm_call_ordinal: Option<u32>,
        limit: usize,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_turns(self.session_id).await?;
        let turn =
            match resolve_raw_turn_selection(&response.turns, &self.branch_id, turn_selection) {
                Ok(turn) => turn,
                Err(message) => {
                    self.maybe_record_command_feedback(Some(raw_input), message, false)
                        .await?;
                    return Ok(());
                }
            };
        let page = self
            .client
            .turn_raw_chunks_page(
                self.session_id,
                turn.branch_id,
                turn.turn_id,
                llm_call_ordinal,
                None,
                limit.max(1),
            )
            .await?;
        let output = match mode {
            RawCommandMode::Chunks => {
                render_raw_output(turn, &page.chunks, llm_call_ordinal, page.has_more_before)
            }
            RawCommandMode::Diff => {
                let turn_events = self.load_turn_events_for_diff(turn).await?;
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
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn load_turn_events_for_diff(
        &self,
        turn: &TurnInspection,
    ) -> Result<Vec<EventEnvelope>, Box<dyn Error>> {
        let Some(event_seq_start) = turn.event_seq_start else {
            return Ok(Vec::new());
        };
        let mut after_seq_id = Some(event_seq_start.saturating_sub(1));
        let mut events = Vec::new();

        loop {
            let response = self
                .client
                .session_events(self.session_id, after_seq_id)
                .await?;
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

    pub(super) async fn render_usage_command(
        &mut self,
        raw_input: &str,
        limit: usize,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_execution(self.session_id).await?;
        let output = render_usage_output(&response.inspection, limit);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_queue_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_queue(self.session_id).await?;
        let output = render_queue_output(&response.inspection);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn clear_queue_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let mut dropped_pending_submissions = 0usize;
        while let Some(pending) = self.pending_message_submissions.pop_front() {
            pending.handle.abort();
            dropped_pending_submissions += 1;
        }
        let response = self.client.clear_session_queue(self.session_id).await?;
        let output = render_queue_clear_output(&response);
        self.request_queue_refresh();
        self.record_operator_command(raw_input, output).await?;
        if dropped_pending_submissions > 0 {
            self.show_notice(format!(
                "Dropped {dropped_pending_submissions} pending background message submission(s) before clearing the server queue."
            ));
        }
        Ok(())
    }

    pub(super) async fn render_branches_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.inspect_branches(self.session_id).await?;
        let output = render_branches_output(response.active_branch_id.as_ref(), &response.branches);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_tree_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_tree(self.session_id).await?;
        let output = render_tree_output(&response.inspection);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_lineage_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_lineage(self.session_id).await?;
        let output = render_lineage_output(&response.inspection);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_workflow_command(
        &mut self,
        raw_input: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.session_workflow(self.session_id).await?;
        let output = render_workflow_output(&response.inspection);
        self.record_operator_command(raw_input, output).await
    }

    pub(super) async fn render_export_command(
        &mut self,
        raw_input: &str,
        format: &str,
        output_path: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = self.client.export_session(self.session_id, format).await?;
        let path = self.resolve_export_output_path(output_path);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &response.content)?;
        let path_label = path.display().to_string();
        let output = render_export_output(
            &format!("{:?}", response.format).to_ascii_lowercase(),
            &response.content_type,
            &response.content,
            Some(&path_label),
        );
        self.record_operator_command(raw_input, output).await
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

    pub(super) async fn render_otlp_push_command(
        &mut self,
        raw_input: &str,
        endpoint: Option<String>,
        project_name: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        let response = self
            .client
            .push_otlp_export(
                self.session_id,
                &PushOtlpExportRequest {
                    endpoint,
                    project_name,
                    api_key: None,
                    headers: BTreeMap::new(),
                },
            )
            .await?;
        let output = render_otlp_push_output(&response);
        self.record_operator_command(raw_input, output).await
    }
}
