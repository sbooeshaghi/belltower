use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CommandDef {
    pub(super) canonical: &'static str,
    pub(super) aliases: &'static [&'static str],
    pub(super) args_hint: &'static str,
    pub(super) description: &'static str,
    pub(super) surface: CommandSurface,
    pub(super) allow_during_request: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InputCompletion {
    pub(super) replacement: Range<usize>,
    pub(super) candidates: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommandMenuItem {
    pub(super) insert_text: String,
    pub(super) label: String,
    pub(super) description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RawCommandMode {
    Chunks,
    Diff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandSurface {
    SlashPrimary,
    SlashHidden,
    ContextualOnly,
}

impl CommandSurface {
    fn is_slash_enabled(self) -> bool {
        matches!(self, Self::SlashPrimary | Self::SlashHidden)
    }

    fn is_default_visible(self) -> bool {
        matches!(self, Self::SlashPrimary)
    }
}

impl CommandDef {
    fn is_slash_enabled(self) -> bool {
        self.surface.is_slash_enabled()
    }

    fn is_default_visible(self) -> bool {
        self.surface.is_default_visible()
    }
}

pub(super) const COMMAND_REGISTRY: &[CommandDef] = &[
    CommandDef {
        canonical: "help",
        aliases: &[],
        args_hint: "",
        description: "Show available commands",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "status",
        aliases: &[],
        args_hint: "",
        description: "Render provider auth and readiness for configured connections",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "models",
        aliases: &[],
        args_hint: "[connection]",
        description: "Render model availability, optionally for one connection",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "doctor",
        aliases: &[],
        args_hint: "",
        description: "Render a full operator health report for server, connections, models, and MCP",
        surface: CommandSurface::SlashHidden,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "mcp",
        aliases: &[],
        args_hint: "[reload]",
        description: "Render MCP server and tool status, or reload MCP server registrations",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "session",
        aliases: &[],
        args_hint: "",
        description: "Render a structured summary of the current session",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "inspect",
        aliases: &[],
        args_hint: "[session|tool <call-id>]",
        description: "Inspect the current session or a specific tool call by id",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "history",
        aliases: &[],
        args_hint: "[limit]",
        description: "Render recent turn history in the chat transcript",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "execution",
        aliases: &[],
        args_hint: "[limit]",
        description: "Render execution-oriented turn and span metadata",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "raw",
        aliases: &[],
        args_hint: "[diff] [turn-id] [call <n>] [limit]",
        description: "Render raw chunks or a raw-vs-structured diff for a selected turn",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "usage",
        aliases: &[],
        args_hint: "[limit]",
        description: "Render token, cost, and latency summaries from executed turns",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "queue",
        aliases: &[],
        args_hint: "[clear]",
        description: "Render queue state or clear queued follow-up messages for the current session",
        surface: CommandSurface::ContextualOnly,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "branches",
        aliases: &[],
        args_hint: "",
        description: "Render structured branch summaries for the current session",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "tree",
        aliases: &[],
        args_hint: "",
        description: "Render the current branch tree and related session lineage",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "lineage",
        aliases: &[],
        args_hint: "",
        description: "Render the cross-session lineage view for the current session",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "workflow",
        aliases: &[],
        args_hint: "",
        description: "Render the multi-session workflow view for the current session graph",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "export",
        aliases: &[],
        args_hint: "[legacy-bundle|jsonl|html|sharegpt|otlp] <path>",
        description: "Write an export view to a local path or push OTLP to a collector",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "refresh",
        aliases: &["r"],
        args_hint: "",
        description: "Reload messages and metadata",
        surface: CommandSurface::SlashHidden,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "new",
        aliases: &[],
        args_hint: "",
        description: "Create a new session",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "compact",
        aliases: &[],
        args_hint: "",
        description: "Run runtime compaction on the current branch context",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "resume",
        aliases: &[],
        args_hint: "[session-id]",
        description: "Resume a previous session from a recency-ordered list",
        surface: CommandSurface::SlashHidden,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "spawn",
        aliases: &["delegate"],
        args_hint: "[--connection <id>] [--model <model-id>] <objective>",
        description: "Create a linked child session, optionally with a selected connection/model",
        surface: CommandSurface::SlashHidden,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "branch",
        aliases: &["fork"],
        args_hint: "",
        description: "Create and switch to a new branch",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "cancel",
        aliases: &["stop"],
        args_hint: "",
        description: "Cancel the current in-flight turn",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "detach",
        aliases: &[],
        args_hint: "",
        description: "Exit the TUI while leaving the current session running",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "steer",
        aliases: &["redirect"],
        args_hint: "<message>",
        description: "Interrupt the current turn with new instructions",
        surface: CommandSurface::SlashHidden,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "approve",
        aliases: &["yes"],
        args_hint: "[once|session|always]",
        description: "Approve the latest pending tool call",
        surface: CommandSurface::ContextualOnly,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "deny",
        aliases: &["no"],
        args_hint: "[once|session|always]",
        description: "Deny the latest pending tool call",
        surface: CommandSurface::ContextualOnly,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "connection",
        aliases: &["conn"],
        args_hint: "[id]",
        description: "Show or switch the session connection",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "model",
        aliases: &["mod"],
        args_hint: "[model-id]",
        description: "Show or switch the session model",
        surface: CommandSurface::SlashHidden,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "defaults",
        aliases: &["default"],
        args_hint: "[connection <id>|model <model-id>|use <connection> [model]]",
        description: "Show or update global defaults for future sessions",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: true,
    },
    CommandDef {
        canonical: "mode",
        aliases: &[],
        args_hint: "[standard|extended]",
        description: "Show or switch the session tool mode",
        surface: CommandSurface::SlashHidden,
        allow_during_request: false,
    },
    CommandDef {
        canonical: "use",
        aliases: &[],
        args_hint: "<connection> [model]",
        description: "Guided connection and model selection with readiness feedback",
        surface: CommandSurface::SlashPrimary,
        allow_during_request: false,
    },
];

pub(super) fn command_usage(command: &CommandDef) -> String {
    if command.args_hint.is_empty() {
        format!("/{}", command.canonical)
    } else {
        format!("/{} {}", command.canonical, command.args_hint)
    }
}

fn approval_scope_candidates() -> Vec<String> {
    vec!["once".to_owned(), "session".to_owned(), "always".to_owned()]
}

fn export_format_candidates() -> Vec<String> {
    vec![
        "legacy-bundle".to_owned(),
        "jsonl".to_owned(),
        "html".to_owned(),
        "sharegpt".to_owned(),
        "otlp".to_owned(),
    ]
}

fn queue_action_candidates() -> Vec<String> {
    vec!["clear".to_owned()]
}

fn defaults_action_candidates() -> Vec<String> {
    vec![
        "connection".to_owned(),
        "model".to_owned(),
        "use".to_owned(),
    ]
}

pub(super) fn parse_approval_scope(
    token: Option<&str>,
) -> std::result::Result<ApprovalScope, String> {
    match token.unwrap_or("once") {
        "once" => Ok(ApprovalScope::Once),
        "session" => Ok(ApprovalScope::Session),
        "always" => Ok(ApprovalScope::Always),
        other => Err(format!(
            "Unknown approval scope `{other}`. Use once, session, or always."
        )),
    }
}

pub(super) fn approval_scope_label(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::Once => "once",
        ApprovalScope::Session => "for this session",
        ApprovalScope::Always => "always",
    }
}

pub(super) fn command_catalog_line() -> String {
    COMMAND_REGISTRY
        .iter()
        .filter(|command| command.is_default_visible())
        .map(|command| {
            let mut label = format!("/{}", command.canonical);
            if !command.args_hint.is_empty() {
                label.push(' ');
                label.push_str(command.args_hint);
            }
            format!("{label} {}", command.description)
        })
        .collect::<Vec<_>>()
        .join("  |  ")
}

pub(super) fn command_allowed_during_request(command: &CommandDef) -> bool {
    command.allow_during_request
}

pub(super) fn is_operator_shell_input(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.starts_with('!') && !trimmed.trim_start_matches('!').trim().is_empty()
}

fn find_connection<'a>(
    connections: &'a [ConnectionDescriptor],
    connection_id: &ConnectionId,
) -> Option<&'a ConnectionDescriptor> {
    connections
        .iter()
        .find(|connection| &connection.id == connection_id)
}

fn find_connection_readiness<'a>(
    inspection: Option<&'a StatusInspection>,
    connection_id: &ConnectionId,
) -> Option<&'a ConnectionReadinessInspection> {
    inspection.and_then(|inspection| {
        inspection
            .connections
            .iter()
            .find(|connection| &connection.connection_id == connection_id)
    })
}

fn find_connection_model_inventory<'a>(
    inventories: &'a [ConnectionModelInventory],
    connection_id: &ConnectionId,
) -> Option<&'a ConnectionModelInventory> {
    inventories
        .iter()
        .find(|inventory| &inventory.connection_id == connection_id)
}

#[cfg(test)]
pub(super) fn session_readiness_summary(
    inspection: Option<&StatusInspection>,
    inventories: &[ConnectionModelInventory],
    connection_id: &ConnectionId,
    effective_model: &str,
) -> String {
    let Some(readiness) = find_connection_readiness(inspection, connection_id) else {
        return "readiness unknown".to_owned();
    };
    let Some(inventory) = find_connection_model_inventory(inventories, connection_id) else {
        return format!(
            "{}: {}",
            readiness.readiness_label(),
            readiness.probe_status
        );
    };

    let discovered_models = inventory
        .models
        .iter()
        .filter(|model| model.source == ConnectionModelSource::Discovered)
        .map(|model| model.model_id.as_str())
        .collect::<Vec<_>>();
    if discovered_models.is_empty() {
        return format!(
            "{}: {}",
            readiness.readiness_label(),
            readiness.probe_status
        );
    }

    let model_is_discovered = discovered_models.contains(&effective_model);
    let base_status = base_probe_status(&readiness.probe_status);
    if model_is_discovered {
        return format!("ready: {base_status}");
    }

    match readiness.readiness_state {
        bt_core::ConnectionReadinessState::Ready
        | bt_core::ConnectionReadinessState::ConfiguredModelUnavailable => {
            let discovered_source = inventory
                .discovered_source
                .as_deref()
                .unwrap_or(inventory.provider.as_str());
            format!(
                "configured model unavailable: {}, but current session model `{}` is not available on {}",
                base_status, effective_model, discovered_source
            )
        }
        _ => format!(
            "{}: {}",
            readiness.readiness_label(),
            readiness.probe_status
        ),
    }
}

#[cfg(test)]
fn base_probe_status(status: &str) -> &str {
    status
        .split(", but configured model `")
        .next()
        .unwrap_or(status)
}

fn connection_status_rank(readiness: Option<&ConnectionReadinessInspection>) -> u8 {
    match readiness {
        Some(readiness)
            if matches!(
                readiness.readiness_state,
                bt_core::ConnectionReadinessState::RuntimeUnsupported
            ) =>
        {
            3
        }
        Some(readiness)
            if matches!(
                readiness.readiness_state,
                bt_core::ConnectionReadinessState::MissingAuth
            ) =>
        {
            2
        }
        Some(readiness) if readiness.is_ready() => 0,
        Some(_) => 1,
        None => 4,
    }
}

fn connection_status_label(readiness: Option<&ConnectionReadinessInspection>) -> &'static str {
    match readiness {
        Some(readiness) => readiness.readiness_label(),
        None => "unknown",
    }
}

fn render_connection_menu_description(
    connection: &ConnectionDescriptor,
    readiness: Option<&ConnectionReadinessInspection>,
    current_connection: &ConnectionId,
) -> String {
    let current_suffix = if &connection.id == current_connection {
        " current"
    } else {
        ""
    };
    format!(
        "{} default {} {}{}",
        connection.provider,
        connection.default_model,
        connection_status_label(readiness),
        current_suffix
    )
}

pub(super) fn available_models_for_connection(
    connections: &[ConnectionDescriptor],
    connection_id: &ConnectionId,
    current_connection: &ConnectionId,
    current_model: Option<&str>,
    connection_models: &[ConnectionModelInventory],
) -> Vec<String> {
    let Some(connection) = find_connection(connections, connection_id) else {
        return Vec::new();
    };

    let mut models = Vec::new();
    if connection_id == current_connection
        && let Some(current_model) = current_model
        && !current_model.is_empty()
    {
        models.push(current_model.to_owned());
    }
    if let Some(inventory) = find_connection_model_inventory(connection_models, connection_id) {
        models.extend(inventory.models.iter().map(|model| model.model_id.clone()));
    } else {
        models.push(connection.default_model.clone());
        models.extend(connection.model_fallbacks.clone());
    }

    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();
    for model in models {
        if seen.insert(model.clone()) {
            deduped.push(model);
        }
    }
    deduped
}

fn model_menu_items_for_connection(
    connections: &[ConnectionDescriptor],
    connection_id: &ConnectionId,
    current_connection: &ConnectionId,
    current_model: Option<&str>,
    connection_models: &[ConnectionModelInventory],
) -> Vec<CommandMenuItem> {
    let Some(connection) = find_connection(connections, connection_id) else {
        return Vec::new();
    };

    let inventory = find_connection_model_inventory(connection_models, connection_id);
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |model: String, description: String, items: &mut Vec<CommandMenuItem>| {
        if seen.insert(model.clone()) {
            items.push(CommandMenuItem {
                insert_text: model.clone(),
                label: model,
                description,
            });
        }
    };

    if connection_id == current_connection
        && let Some(current_model) = current_model
        && !current_model.is_empty()
    {
        push(
            current_model.to_owned(),
            "Current session model".to_owned(),
            &mut items,
        );
    }

    if let Some(inventory) = inventory {
        for model in &inventory.models {
            let description = match model.source {
                ConnectionModelSource::Default => {
                    format!("Configured default for {}", connection.id)
                }
                ConnectionModelSource::Fallback => {
                    format!("Catalog alternative for {}", connection.id)
                }
                ConnectionModelSource::Discovered => inventory
                    .discovered_source
                    .as_deref()
                    .map(|source| format!("Discovered on {source}"))
                    .unwrap_or_else(|| "Discovered from the active connection".to_owned()),
            };
            push(model.model_id.clone(), description, &mut items);
        }
    } else {
        push(
            connection.default_model.clone(),
            format!("Configured default for {}", connection.id),
            &mut items,
        );
        for fallback in &connection.model_fallbacks {
            push(
                fallback.clone(),
                format!("Catalog alternative for {}", connection.id),
                &mut items,
            );
        }
    }

    items
}

fn connection_menu_items(
    connections: &[ConnectionDescriptor],
    readiness: Option<&StatusInspection>,
    current_connection: &ConnectionId,
    current_prefix: &str,
    trailing_space: bool,
) -> Vec<CommandMenuItem> {
    let mut candidates = connections
        .iter()
        .filter(|connection| connection.id.to_string().starts_with(current_prefix))
        .map(|connection| {
            let readiness = find_connection_readiness(readiness, &connection.id);
            (connection, readiness)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left, left_readiness), (right, right_readiness)| {
        let left_current = (&left.id != current_connection) as u8;
        let right_current = (&right.id != current_connection) as u8;
        left_current
            .cmp(&right_current)
            .then(
                connection_status_rank(*left_readiness)
                    .cmp(&connection_status_rank(*right_readiness)),
            )
            .then(left.id.to_string().cmp(&right.id.to_string()))
    });

    candidates
        .into_iter()
        .map(|(connection, readiness)| CommandMenuItem {
            insert_text: if trailing_space {
                format!("{} ", connection.id)
            } else {
                connection.id.to_string()
            },
            label: connection.id.to_string(),
            description: render_connection_menu_description(
                connection,
                readiness,
                current_connection,
            ),
        })
        .collect()
}

pub(super) fn completion_for_input(
    input: &str,
    current_connection: &ConnectionId,
    current_model: Option<&str>,
    connections: &[ConnectionDescriptor],
    connection_models: &[ConnectionModelInventory],
) -> Option<InputCompletion> {
    if !input.starts_with('/') {
        return None;
    }

    let replacement_start = if input.ends_with(char::is_whitespace) {
        input.len()
    } else {
        input
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1)
    };
    let replacement = replacement_start..input.len();
    let current_prefix = &input[replacement.clone()];
    let preceding = input[..replacement_start].trim_end();
    let tokens = if preceding.is_empty() {
        Vec::new()
    } else {
        preceding.split_whitespace().collect::<Vec<_>>()
    };

    let mut candidates = if tokens.is_empty() {
        let show_all_matching = current_prefix != "/";
        COMMAND_REGISTRY
            .iter()
            .filter(|command| {
                command.is_slash_enabled() && (show_all_matching || command.is_default_visible())
            })
            .map(|command| format!("/{}", command.canonical))
            .filter(|command| command.starts_with(current_prefix))
            .collect::<Vec<_>>()
    } else {
        let command = resolve_command(tokens[0]).ok()?;
        match command.canonical {
            "help" if tokens.len() == 1 => ["all".to_owned()]
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "connection" if tokens.len() == 1 => connections
                .iter()
                .map(|connection| connection.id.to_string())
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "model" if tokens.len() == 1 => available_models_for_connection(
                connections,
                current_connection,
                current_connection,
                current_model,
                connection_models,
            )
            .into_iter()
            .filter(|candidate| candidate.starts_with(current_prefix))
            .collect::<Vec<_>>(),
            "use" if tokens.len() == 1 => connections
                .iter()
                .map(|connection| connection.id.to_string())
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "use" if tokens.len() == 2 => {
                let target_connection = ConnectionId::new(tokens[1]);
                available_models_for_connection(
                    connections,
                    &target_connection,
                    current_connection,
                    current_model,
                    connection_models,
                )
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>()
            }
            "defaults" if tokens.len() == 1 => defaults_action_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "defaults" if tokens.len() == 2 && matches!(tokens[1], "connection" | "use") => {
                connections
                    .iter()
                    .map(|connection| connection.id.to_string())
                    .filter(|candidate| candidate.starts_with(current_prefix))
                    .collect::<Vec<_>>()
            }
            "defaults" if tokens.len() == 2 && tokens[1] == "model" => {
                available_models_for_connection(
                    connections,
                    current_connection,
                    current_connection,
                    current_model,
                    connection_models,
                )
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>()
            }
            "defaults" if tokens.len() == 3 && tokens[1] == "use" => {
                let target_connection = ConnectionId::new(tokens[2]);
                available_models_for_connection(
                    connections,
                    &target_connection,
                    current_connection,
                    current_model,
                    connection_models,
                )
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>()
            }
            "approve" | "deny" if tokens.len() == 1 => approval_scope_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "queue" if tokens.len() == 1 => queue_action_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "inspect" if tokens.len() == 1 => ["session".to_owned(), "tool".to_owned()]
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "export" if tokens.len() == 1 => export_format_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            "export" if tokens.len() == 2 && tokens[1] == "otlp" => ["push".to_owned()]
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        }
    };
    candidates.sort();
    candidates.dedup();
    (!candidates.is_empty()).then_some(InputCompletion {
        replacement,
        candidates,
    })
}

pub(super) fn command_menu_items(
    input: &str,
    current_connection: &ConnectionId,
    current_model: Option<&str>,
    connections: &[ConnectionDescriptor],
    readiness: Option<&StatusInspection>,
    connection_models: &[ConnectionModelInventory],
    sessions: &[SessionRecord],
    project_root: &str,
) -> Vec<CommandMenuItem> {
    if !input.starts_with('/') {
        return Vec::new();
    }

    let replacement_start = if input.ends_with(char::is_whitespace) {
        input.len()
    } else {
        input
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1)
    };
    let current_prefix = &input[replacement_start..];
    let preceding = input[..replacement_start].trim_end();
    let tokens = if preceding.is_empty() {
        Vec::new()
    } else {
        preceding.split_whitespace().collect::<Vec<_>>()
    };

    let mut items = if tokens.is_empty() {
        let show_all_matching = current_prefix != "/";
        COMMAND_REGISTRY
            .iter()
            .filter(|command| {
                command.is_slash_enabled()
                    && (show_all_matching || command.is_default_visible())
                    && format!("/{}", command.canonical).starts_with(current_prefix)
            })
            .map(|command| CommandMenuItem {
                insert_text: format!(
                    "/{}{}",
                    command.canonical,
                    if command.args_hint.is_empty() {
                        ""
                    } else {
                        " "
                    }
                ),
                label: command_usage(command),
                description: command.description.to_owned(),
            })
            .collect::<Vec<_>>()
    } else {
        let Ok(command) = resolve_command(tokens[0]) else {
            return Vec::new();
        };
        match command.canonical {
            "help" if tokens.len() == 1 => ["all".to_owned()]
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .map(|candidate| CommandMenuItem {
                    insert_text: candidate.clone(),
                    label: candidate,
                    description: "Show the advanced slash-command surface".to_owned(),
                })
                .collect::<Vec<_>>(),
            "resume" if tokens.len() == 1 => resume_candidates(sessions, project_root, None)
                .0
                .into_iter()
                .filter(|session| resume_selection_matches(session, current_prefix))
                .map(resume_menu_item)
                .collect::<Vec<_>>(),
            "connection" if tokens.len() == 1 => connection_menu_items(
                connections,
                readiness,
                current_connection,
                current_prefix,
                false,
            ),
            "model" if tokens.len() == 1 => model_menu_items_for_connection(
                connections,
                current_connection,
                current_connection,
                current_model,
                connection_models,
            )
            .into_iter()
            .filter(|candidate| candidate.label.starts_with(current_prefix))
            .collect::<Vec<_>>(),
            "use" if tokens.len() == 1 => connection_menu_items(
                connections,
                readiness,
                current_connection,
                current_prefix,
                true,
            ),
            "use" if tokens.len() == 2 => {
                let target_connection = ConnectionId::new(tokens[1]);
                model_menu_items_for_connection(
                    connections,
                    &target_connection,
                    current_connection,
                    current_model,
                    connection_models,
                )
                .into_iter()
                .filter(|candidate| candidate.label.starts_with(current_prefix))
                .collect::<Vec<_>>()
            }
            "defaults" if tokens.len() == 1 => defaults_action_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .map(|candidate| CommandMenuItem {
                    insert_text: format!("{candidate} "),
                    label: candidate.clone(),
                    description: match candidate.as_str() {
                        "connection" => {
                            "Persist the default connection for future sessions".to_owned()
                        }
                        "model" => "Persist the current connection's default model".to_owned(),
                        "use" => "Persist a connection and optional model together".to_owned(),
                        _ => "Update global defaults".to_owned(),
                    },
                })
                .collect::<Vec<_>>(),
            "defaults" if tokens.len() == 2 && matches!(tokens[1], "connection" | "use") => {
                connection_menu_items(
                    connections,
                    readiness,
                    current_connection,
                    current_prefix,
                    tokens[1] == "use",
                )
            }
            "defaults" if tokens.len() == 2 && tokens[1] == "model" => {
                model_menu_items_for_connection(
                    connections,
                    current_connection,
                    current_connection,
                    current_model,
                    connection_models,
                )
                .into_iter()
                .filter(|candidate| candidate.label.starts_with(current_prefix))
                .collect::<Vec<_>>()
            }
            "defaults" if tokens.len() == 3 && tokens[1] == "use" => {
                let target_connection = ConnectionId::new(tokens[2]);
                model_menu_items_for_connection(
                    connections,
                    &target_connection,
                    current_connection,
                    current_model,
                    connection_models,
                )
                .into_iter()
                .filter(|candidate| candidate.label.starts_with(current_prefix))
                .collect::<Vec<_>>()
            }
            "approve" | "deny" if tokens.len() == 1 => approval_scope_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .map(|candidate| CommandMenuItem {
                    insert_text: candidate.clone(),
                    label: candidate,
                    description: "Apply this approval scope".to_owned(),
                })
                .collect::<Vec<_>>(),
            "queue" if tokens.len() == 1 => queue_action_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .map(|candidate| CommandMenuItem {
                    insert_text: candidate.clone(),
                    label: candidate,
                    description: "Clear queued follow-up messages for this session".to_owned(),
                })
                .collect::<Vec<_>>(),
            "inspect" if tokens.len() == 1 => ["session".to_owned(), "tool".to_owned()]
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .map(|candidate| CommandMenuItem {
                    insert_text: candidate.clone(),
                    label: candidate.clone(),
                    description: match candidate.as_str() {
                        "session" => "Inspect the current session state".to_owned(),
                        "tool" => "Inspect a specific tool call by id".to_owned(),
                        _ => "Inspect structured session state".to_owned(),
                    },
                })
                .collect::<Vec<_>>(),
            "export" if tokens.len() == 1 => export_format_candidates()
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .map(|candidate| CommandMenuItem {
                    insert_text: candidate.clone(),
                    label: candidate,
                    description: "Export format preview".to_owned(),
                })
                .collect::<Vec<_>>(),
            "export" if tokens.len() == 2 && tokens[1] == "otlp" => ["push".to_owned()]
                .into_iter()
                .filter(|candidate| candidate.starts_with(current_prefix))
                .map(|candidate| CommandMenuItem {
                    insert_text: format!("{candidate} "),
                    label: candidate,
                    description: "Upload real OTLP protobuf to a collector endpoint".to_owned(),
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        }
    };

    let trim = input.trim_start();
    if !trim.starts_with("/resume")
        && !trim.starts_with("/use")
        && !trim.starts_with("/connection")
        && !trim.starts_with("/model")
        && !trim.starts_with("/defaults")
    {
        items.sort_by(|left, right| left.label.cmp(&right.label));
    }
    items.dedup_by(|left, right| left.label == right.label);
    items
}

fn resume_selection_matches(session: &SessionRecord, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }

    let normalized = prefix.to_ascii_lowercase();
    let session_id = session.session_id.to_string();
    session_id.starts_with(prefix)
        || short_id_string(&session_id).starts_with(prefix)
        || session_title(Some(session))
            .to_ascii_lowercase()
            .starts_with(&normalized)
}

fn resume_updated_at_label(updated_at: time::OffsetDateTime) -> String {
    updated_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| updated_at.to_string())
}

fn resume_menu_item(session: &SessionRecord) -> CommandMenuItem {
    let session_id = session.session_id.to_string();
    let short_id = short_id_string(&session_id);
    let model = session.model_id.as_deref().unwrap_or("default");
    CommandMenuItem {
        insert_text: session_id,
        label: format!("{} [{}]", session_title(Some(session)), short_id),
        description: format!(
            "{}  {} / {}  {}",
            resume_updated_at_label(session.updated_at),
            session.connection_id,
            model,
            truncate_path(session.project_root.as_str(), 52)
        ),
    }
}

pub(super) fn resolve_resume_selection<'a>(
    sessions: &'a [SessionRecord],
    project_root: &str,
    selection: &str,
) -> std::result::Result<&'a SessionRecord, String> {
    let (candidates, _) = resume_candidates(sessions, project_root, None);
    if candidates.is_empty() {
        return Err("No sessions are available to resume.".to_owned());
    }

    if let Ok(index) = selection.parse::<usize>()
        && let Some(session) = candidates.get(index.saturating_sub(1))
    {
        return Ok(session);
    }

    let normalized = selection.to_ascii_lowercase();
    candidates
        .into_iter()
        .find(|session| {
            let session_id = session.session_id.to_string();
            session_id == selection
                || short_id_string(&session_id) == selection
                || session_title(Some(session)).to_ascii_lowercase() == normalized
        })
        .ok_or_else(|| format!("Unknown resume selection `{selection}`."))
}

pub(super) fn shared_prefix(candidates: &[String]) -> Option<String> {
    let first = candidates.first()?.clone();
    let prefix = candidates.iter().skip(1).fold(first, |prefix, candidate| {
        prefix
            .chars()
            .zip(candidate.chars())
            .take_while(|(left, right)| left == right)
            .map(|(ch, _)| ch)
            .collect::<String>()
    });
    Some(prefix)
}

pub(super) fn completion_candidates_line(candidates: &[String]) -> String {
    let mut visible = candidates.iter().take(6).cloned().collect::<Vec<_>>();
    if candidates.len() > visible.len() {
        visible.push(format!("+{} more", candidates.len() - visible.len()));
    }
    format!("Suggestions: {}", visible.join(", "))
}

fn command_not_exposed_as_slash_message(command: &CommandDef) -> String {
    match command.canonical {
        "approve" | "deny" => {
            "Approvals are resolved from the approval chooser when a tool call pauses."
                .to_owned()
        }
        "queue" => {
            "Queue state is rendered inline in normal operation and no longer has a top-level slash command."
                .to_owned()
        }
        _ => format!(
            "`/{}` is not exposed in the current slash-command surface.",
            command.canonical
        ),
    }
}

pub(super) fn resolve_command(query: &str) -> std::result::Result<&'static CommandDef, String> {
    let normalized = query.trim().trim_start_matches('/').to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("Use /help to see available commands.".to_owned());
    }

    if let Some(exact) = COMMAND_REGISTRY.iter().find(|command| {
        command.canonical == normalized || command.aliases.iter().any(|alias| *alias == normalized)
    }) {
        if !exact.is_slash_enabled() {
            return Err(command_not_exposed_as_slash_message(exact));
        }
        return Ok(exact);
    }

    let mut matches = COMMAND_REGISTRY
        .iter()
        .filter(|command| {
            command.is_slash_enabled()
                && (command.canonical.starts_with(&normalized)
                    || command
                        .aliases
                        .iter()
                        .any(|alias| alias.starts_with(&normalized)))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|command| command.canonical);
    matches.dedup_by_key(|command| command.canonical);

    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!("Unknown command `/{normalized}`. Use /help.")),
        _ => Err(format!(
            "Ambiguous command `/{normalized}`. Matches: {}",
            matches
                .iter()
                .map(|command| format!("/{}", command.canonical))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub(super) fn command_help_output(show_all: bool) -> String {
    let mut lines = COMMAND_REGISTRY
        .iter()
        .filter(|command| command.is_slash_enabled() && (show_all || command.is_default_visible()))
        .map(|command| format!("{}\n  {}", command_usage(command), command.description))
        .collect::<Vec<_>>();

    if !show_all
        && COMMAND_REGISTRY
            .iter()
            .any(|command| matches!(command.surface, CommandSurface::SlashHidden))
    {
        lines.push("/help all\n  Show advanced slash commands".to_owned());
    }

    lines.push(
        "Contextual controls\n  approvals resolve through the approval chooser; queued follow-up state is rendered inline"
            .to_owned(),
    );
    lines.join("\n")
}

pub(super) fn parse_raw_command_args<'a>(
    args: &'a [&'a str],
) -> Result<(RawCommandMode, Option<&'a str>, Option<u32>, usize), String> {
    let (mode, remaining) = if matches!(args.first(), Some(value) if value.eq_ignore_ascii_case("diff"))
    {
        (RawCommandMode::Diff, &args[1..])
    } else {
        (RawCommandMode::Chunks, args)
    };

    let mut turn_id = None;
    let mut llm_call_ordinal = None;
    let mut limit = DEFAULT_RAW_CHUNK_LIMIT;
    let mut index = 0;

    while index < remaining.len() {
        let token = remaining[index];
        if token.eq_ignore_ascii_case("call") {
            let Some(next) = remaining.get(index + 1) else {
                return Err("Usage: /raw [diff] [turn-id] [call <n>] [limit]".to_owned());
            };
            let ordinal = next
                .parse::<u32>()
                .map_err(|error| format!("invalid raw llm call ordinal: {error}"))?;
            if ordinal == 0 {
                return Err("raw llm call ordinal must be >= 1".to_owned());
            }
            llm_call_ordinal = Some(ordinal);
            index += 2;
            continue;
        }
        if turn_id.is_none() && token.parse::<usize>().is_err() {
            turn_id = Some(token);
            index += 1;
            continue;
        }
        if limit == DEFAULT_RAW_CHUNK_LIMIT {
            limit = token
                .parse::<usize>()
                .map_err(|error| format!("invalid raw limit: {error}"))?;
            index += 1;
            continue;
        }
        return Err("Usage: /raw [diff] [turn-id] [call <n>] [limit]".to_owned());
    }

    Ok((mode, turn_id, llm_call_ordinal, limit))
}

pub(super) fn resolve_raw_turn_selection<'a>(
    turns: &'a [TurnInspection],
    current_branch_id: &BranchId,
    selection: Option<&str>,
) -> Result<&'a TurnInspection, String> {
    if turns.is_empty() {
        return Err("No turns recorded yet.".to_owned());
    }

    if let Some(selection) = selection {
        if selection.eq_ignore_ascii_case("latest") {
            return latest_raw_turn(turns, current_branch_id)
                .ok_or_else(|| "No turns recorded yet.".to_owned());
        }

        return turns
            .iter()
            .find(|turn| {
                turn.turn_id.to_string() == selection
                    || short_id_string(&turn.turn_id.to_string()) == selection
            })
            .ok_or_else(|| format!("Unknown turn `{selection}`."));
    }

    latest_raw_turn(turns, current_branch_id).ok_or_else(|| "No turns recorded yet.".to_owned())
}

pub(super) fn latest_raw_turn<'a>(
    turns: &'a [TurnInspection],
    current_branch_id: &BranchId,
) -> Option<&'a TurnInspection> {
    turns
        .iter()
        .rev()
        .find(|turn| &turn.branch_id == current_branch_id)
        .or_else(|| turns.last())
}
