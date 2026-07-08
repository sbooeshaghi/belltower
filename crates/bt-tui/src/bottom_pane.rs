use super::*;
use crate::message_render::render_wrapped_prefixed_entry;
use crate::transcript_helpers::message_text_content;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BottomSurfaceKind {
    Footer,
    CommandMenu,
    Question,
    Approval,
}

pub(super) struct BottomSurfaceView {
    #[cfg(test)]
    pub(super) title: String,
    pub(super) kind: BottomSurfaceKind,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) cursor: Option<(u16, u16)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FooterProps {
    session_context: String,
    cwd: String,
    detail: Option<String>,
}

pub(super) fn footer_height(app: &ChatApp) -> u16 {
    render_footer_lines(app).len().min(usize::from(u16::MAX)) as u16
}

fn footer_is_visible(app: &ChatApp) -> bool {
    app.composer.input.is_empty()
}

fn bottom_panel_content_height(app: &ChatApp) -> u16 {
    let surface = render_bottom_surface_content(app);
    match surface.kind {
        BottomSurfaceKind::Footer => 0,
        BottomSurfaceKind::CommandMenu => command_menu_panel_height(app),
        BottomSurfaceKind::Question => measured_surface_height(
            &surface.lines,
            app.bottom_panel_view_width(),
            BOTTOM_PANEL_ACTIVE_HEIGHT,
        )
        .max(3),
        BottomSurfaceKind::Approval => surface
            .lines
            .len()
            .max(1)
            .min(usize::from(BOTTOM_PANEL_ACTIVE_HEIGHT))
            as u16,
    }
}

pub(super) fn popup_or_footer_height(app: &ChatApp) -> u16 {
    let content_height = bottom_panel_content_height(app);
    if content_height > 0 {
        content_height
    } else if footer_is_visible(app) {
        footer_height(app)
    } else {
        0
    }
}

pub(super) fn composer_body_height(app: &ChatApp) -> u16 {
    if app.question_panel_is_active() {
        return 0;
    }

    app.input_panel_height()
        .saturating_add(COMPOSER_TOP_PADDING)
        .saturating_add(COMPOSER_BOTTOM_PADDING)
        .saturating_add(COMPOSER_GROWTH_HEADROOM)
        .max(MIN_COMPOSER_BODY_HEIGHT)
}

pub(super) const fn shell_top_gap_height() -> u16 {
    0
}

pub(super) fn shell_aux_lines(app: &ChatApp, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let status_lines = if app.question_panel_is_active() {
        Vec::new()
    } else {
        pending_chat_placeholder_lines(app, width)
    };
    if !status_lines.is_empty() {
        lines.extend(status_lines);
    }

    let queued_preview = queued_follow_up_preview_lines(app, width);
    if !queued_preview.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.extend(queued_preview);
    }

    lines
}

fn render_task_status_lines(app: &ChatApp, width: u16) -> Vec<Line<'static>> {
    let Some(task_status) = app.task_status.as_ref() else {
        return Vec::new();
    };
    let started_at = app
        .pending_send_started_at
        .or(app.pending_approval_started_at)
        .or(app.active_turn.started_at);

    let mut header = format!("• {}", task_status.header);
    if let Some(started_at) = started_at {
        if task_status.show_interrupt_hint {
            header.push_str(&format!(
                " ({} • esc to interrupt)",
                elapsed_label(started_at)
            ));
        } else {
            header.push_str(&format!(" ({})", elapsed_label(started_at)));
        }
    }

    let mut lines = vec![Line::from(header)];
    if let Some(detail) = task_status.detail.as_deref() {
        let wrap_width = usize::from(width.max(1));
        let detail_lines = render_wrapped_prefixed_entry("  └ ", "    ", detail, width);
        lines.extend(detail_lines.lines().map(|line| {
            let text = if display_width(line) > wrap_width {
                truncate_inline_text(line, wrap_width)
            } else {
                line.to_owned()
            };
            Line::from(text)
        }));
    }
    lines
}

pub(super) fn shell_aux_height(app: &ChatApp, width: u16) -> u16 {
    let lines = shell_aux_lines(app, width);
    if lines.is_empty() {
        0
    } else {
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .line_count(width.max(1))
            .try_into()
            .unwrap_or(0)
    }
}

pub(super) fn bottom_shell_height(app: &ChatApp, width: u16) -> u16 {
    shell_top_gap_height()
        .saturating_add(shell_aux_height(app, width))
        .saturating_add(composer_body_height(app))
        .saturating_add(popup_or_footer_height(app))
}

pub(super) fn command_menu_panel_height(app: &ChatApp) -> u16 {
    let items = app.current_command_menu_items();
    if items.is_empty() {
        2
    } else {
        items.len().min(usize::from(BOTTOM_PANEL_ACTIVE_HEIGHT)) as u16
    }
}

pub(crate) fn command_menu_visible_row_count() -> usize {
    usize::from(BOTTOM_PANEL_ACTIVE_HEIGHT.saturating_sub(2)).max(1)
}

fn measured_surface_height(lines: &[Line<'static>], width: u16, max_height: u16) -> u16 {
    Paragraph::new(Text::from(lines.to_vec()))
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
        .try_into()
        .unwrap_or(max_height)
        .clamp(1, max_height.max(1))
}

pub(super) fn render_bottom_surface(app: &ChatApp) -> BottomSurfaceView {
    render_bottom_surface_content(app)
}

#[cfg(test)]
pub(super) fn render_bottom_panel(app: &ChatApp) -> BottomSurfaceView {
    render_bottom_surface(app)
}

fn render_bottom_surface_content(app: &ChatApp) -> BottomSurfaceView {
    if app.approval_menu_is_active() {
        let pending = app
            .pending_approvals
            .first()
            .expect("pending approval tool");
        let detail = app
            .pending_tool_detail(&pending.call_id)
            .map(|detail| format!(" {detail}"))
            .unwrap_or_default();
        let mut lines = vec![Line::from(format!(
            "approval: {} {}{}",
            pending.tool_name,
            short_id_string(&pending.call_id.to_string()),
            detail
        ))];
        lines.extend(
            ApprovalMenuChoice::all()
                .iter()
                .enumerate()
                .map(|(index, choice)| {
                    let style = if index == app.bottom_shell.approval_menu_selection {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(
                        render_approval_menu_entry(
                            *choice,
                            index == app.bottom_shell.approval_menu_selection,
                            usize::from(app.bottom_panel_view_width()),
                        ),
                        style,
                    ))
                })
                .collect::<Vec<_>>(),
        );
        BottomSurfaceView {
            #[cfg(test)]
            title: "Approval".to_owned(),
            kind: BottomSurfaceKind::Approval,
            lines,
            cursor: None,
        }
    } else if app.should_show_command_menu() {
        let base_title = slash_panel_title(&app.composer.input);
        let items = app.current_command_menu_items();
        let lines = if items.is_empty() {
            if base_title == "Resume" {
                vec![Line::from("No sessions available to resume.")]
            } else {
                vec![
                    Line::from("No matching commands."),
                    Line::from(command_catalog_line()),
                ]
            }
        } else {
            let visible_row_count = command_menu_visible_row_count();
            let max_start = items.len().saturating_sub(visible_row_count);
            let start = app.bottom_shell.command_menu_scroll_top.min(max_start);
            let visible_items = items
                .iter()
                .skip(start)
                .take(visible_row_count)
                .collect::<Vec<_>>();
            let label_width = command_menu_label_width(
                &visible_items,
                usize::from(app.bottom_panel_view_width()),
            );
            items
                .into_iter()
                .enumerate()
                .skip(start)
                .take(visible_row_count)
                .map(|(index, item)| {
                    let style = if index == app.bottom_shell.command_menu_selection {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(
                        render_command_menu_entry(
                            &item,
                            index == app.bottom_shell.command_menu_selection,
                            usize::from(app.bottom_panel_view_width()),
                            label_width,
                        ),
                        style,
                    ))
                })
                .collect::<Vec<_>>()
        };
        BottomSurfaceView {
            #[cfg(test)]
            title: base_title.to_owned(),
            kind: BottomSurfaceKind::CommandMenu,
            lines,
            cursor: None,
        }
    } else if app.question_panel_is_active() {
        let pending = app.pending_inputs.first().expect("pending question");
        let width = app.bottom_panel_view_width();
        let (lines, cursor) = render_question_surface(app, pending, width);
        BottomSurfaceView {
            #[cfg(test)]
            title: "Question".to_owned(),
            kind: BottomSurfaceKind::Question,
            lines,
            cursor,
        }
    } else {
        BottomSurfaceView {
            #[cfg(test)]
            title: "Footer".to_owned(),
            kind: BottomSurfaceKind::Footer,
            lines: if footer_is_visible(app) {
                render_footer_lines(app)
            } else {
                Vec::new()
            },
            cursor: None,
        }
    }
}

fn render_question_surface(
    app: &ChatApp,
    pending: &PendingInputInspection,
    width: u16,
) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
    let mut lines = vec![Line::from(vec![
        Span::styled("Question 1/1", Style::default().add_modifier(Modifier::DIM)),
        Span::styled(
            " (1 unanswered)",
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])];

    for line in render_wrapped_prefixed_entry("", "", &pending.prompt, width).lines() {
        lines.push(Line::from(Span::styled(
            line.to_owned(),
            Style::default().fg(Color::Cyan),
        )));
    }

    lines.push(Line::from(""));

    if !pending.choices.is_empty() {
        let selected = app.bottom_shell.question_choice_selection;
        let label_width = question_choice_label_width(&pending.choices);
        for (index, choice) in pending.choices.iter().enumerate() {
            lines.push(Line::from(render_question_choice_line(
                index,
                selected == index,
                label_width,
                choice,
                usize::from(width),
            )));
        }
        if !app.composer.input.is_empty() {
            lines.push(Line::from(""));
            let answer_start = lines.len().min(usize::from(u16::MAX)) as u16;
            let (visible_input_lines, input_scroll_row, _) = app.visible_input_lines();
            lines.extend(
                visible_input_lines
                    .into_iter()
                    .enumerate()
                    .map(|(index, line)| {
                        let prefix = if input_scroll_row == 0 && index == 0 {
                            COMPOSER_PREFIX
                        } else {
                            COMPOSER_CONTINUATION_PREFIX
                        };
                        Line::from(vec![Span::raw(prefix), Span::raw(line)])
                    }),
            );
            lines.push(Line::from(""));
            lines.push(question_hint_line(true));
            let (cursor_col, cursor_row) = app.input_cursor_position();
            let prefix_width = display_width(COMPOSER_PREFIX).min(usize::from(u16::MAX)) as u16;
            let cursor = Some((
                prefix_width.saturating_add(cursor_col),
                answer_start.saturating_add(cursor_row.saturating_sub(input_scroll_row)),
            ));
            return (lines, cursor);
        }
        lines.push(Line::from(""));
        lines.push(question_hint_line(true));
        return (lines, None);
    }

    let answer_start = lines.len().min(usize::from(u16::MAX)) as u16;
    let (visible_input_lines, input_scroll_row, _) = app.visible_input_lines();
    if app.composer.input.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(COMPOSER_PREFIX),
            Span::styled(
                "Type your answer (optional)".to_owned(),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]));
    } else {
        lines.extend(
            visible_input_lines
                .into_iter()
                .enumerate()
                .map(|(index, line)| {
                    let prefix = if input_scroll_row == 0 && index == 0 {
                        COMPOSER_PREFIX
                    } else {
                        COMPOSER_CONTINUATION_PREFIX
                    };
                    Line::from(vec![Span::raw(prefix), Span::raw(line)])
                }),
        );
    }
    lines.push(Line::from(""));
    lines.push(question_hint_line(false));

    let (cursor_col, cursor_row) = app.input_cursor_position();
    let prefix_width = display_width(COMPOSER_PREFIX).min(usize::from(u16::MAX)) as u16;
    let cursor = Some((
        prefix_width.saturating_add(cursor_col),
        answer_start.saturating_add(cursor_row.saturating_sub(input_scroll_row)),
    ));
    (lines, cursor)
}

fn render_question_choice_line(
    index: usize,
    selected: bool,
    label_width: usize,
    choice: &str,
    width: usize,
) -> Vec<Span<'static>> {
    let prefix = if selected {
        COMPOSER_PREFIX
    } else {
        COMPOSER_CONTINUATION_PREFIX
    };
    let label = format!("{}.", index + 1);
    let available = width.saturating_sub(display_width(prefix)).max(1);
    let text_width = available.saturating_sub(label_width).saturating_sub(2);
    let choice = truncate_inline_text(choice, text_width.max(1));
    let mut label_style = Style::default();
    let mut choice_style = Style::default();
    if selected {
        label_style = label_style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
        choice_style = choice_style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
    }
    vec![
        Span::raw(prefix.to_owned()),
        Span::styled(pad_to_display_width(&label, label_width), label_style),
        Span::raw("  "),
        Span::styled(choice, choice_style),
    ]
}

fn question_choice_label_width(choices: &[String]) -> usize {
    let max_index_width = choices.len().max(1).to_string().len().saturating_add(1);
    max_index_width.max(2)
}

fn question_hint_line(has_options: bool) -> Line<'static> {
    let mut spans = if has_options {
        vec![
            Span::styled(
                "up/down to choose",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" | "),
        ]
    } else {
        Vec::new()
    };
    spans.extend([
        Span::styled(
            "enter to submit answer",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled(
            "esc to interrupt",
            Style::default().add_modifier(Modifier::DIM),
        ),
    ]);
    Line::from(spans)
}

pub(super) fn render_footer_lines(app: &ChatApp) -> Vec<Line<'static>> {
    render_footer_lines_from_props(&footer_props(app), app.bottom_panel_view_width())
}

pub(super) fn queued_follow_up_preview_lines(app: &ChatApp, width: u16) -> Vec<Line<'static>> {
    let mut previews = Vec::new();
    if let Some(queue) = app.queue_inspection.as_ref() {
        for queued in &queue.queued_messages {
            if let Some(text) = message_text_content(&queued.message, Role::User)
                && !text.is_empty()
            {
                previews.push(text);
            }
        }
    }
    for pending in &app.pending_message_submissions {
        if !previews.iter().any(|preview| preview == &pending.preview) {
            previews.push(pending.preview.clone());
        }
    }

    if previews.is_empty() {
        return Vec::new();
    }

    let wrap_width = usize::from(width.saturating_sub(4).max(1));
    let mut lines = vec![Line::from("• Queued follow-up messages")];
    for preview in previews.iter().take(3) {
        let wrapped = wrap_plain_text(preview, wrap_width);
        for (index, line) in wrapped.into_iter().take(3).enumerate() {
            let prefix = if index == 0 { "  ↳ " } else { "    " };
            lines.push(Line::from(format!("{prefix}{line}")));
        }
    }
    if previews.len() > 3 {
        lines.push(Line::from("    ..."));
    }
    lines
}

fn footer_props(app: &ChatApp) -> FooterProps {
    let session_context = format!(
        "Session: {} (Branch: {}) | {} ({})",
        short_id_string(&app.session_id.to_string()),
        short_id_string(&app.branch_id.to_string()),
        app.connection_id,
        app.effective_model(),
    );
    let cwd = display_project_root(&app.project_root);
    let queue_detail = app.queue_inspection.as_ref().and_then(|queue| {
        (!queue.queued_messages.is_empty()
            || !queue.pending_approvals.is_empty()
            || !queue.pending_inputs.is_empty()
            || queue.cancel_requested
            || queue.pending_steer_count > 0)
            .then(|| {
                format!(
                    "queue messages={} approvals={} inputs={} cancel_requested={} pending_steer={}",
                    queue.queued_messages.len(),
                    queue.pending_approvals.len(),
                    queue.pending_inputs.len(),
                    queue.cancel_requested,
                    queue.pending_steer_count
                )
            })
    });
    let detail = app
        .active_notice()
        .map(|notice| format!("note {}", notice))
        .or(queue_detail);
    FooterProps {
        session_context,
        cwd,
        detail,
    }
}

fn render_footer_lines_from_props(props: &FooterProps, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width).max(1);
    let inline_context = format!("{} | {}", props.session_context, props.cwd);
    let context_lines = if display_width(&inline_context) <= width {
        vec![Line::from(Span::styled(
            inline_context,
            Style::default().add_modifier(Modifier::DIM),
        ))]
    } else {
        vec![
            Line::from(Span::styled(
                truncate_inline_text(&props.session_context, width),
                Style::default().add_modifier(Modifier::DIM),
            )),
            Line::from(Span::styled(
                truncate_path(&props.cwd, width),
                Style::default().add_modifier(Modifier::DIM),
            )),
        ]
    };
    match props.detail.as_deref() {
        Some(detail) => {
            let mut lines = vec![Line::from(truncate_inline_text(detail, width))];
            lines.extend(context_lines);
            lines
        }
        None => context_lines,
    }
}

pub(super) fn render_startup_banner(app: &ChatApp, tmux_warning: Option<&str>) -> String {
    let mut lines = vec![
        format!(
            "Welcome to Belltower v{} ({})",
            env!("CARGO_PKG_VERSION"),
            display_project_root(&app.project_root)
        ),
        String::new(),
        format!(
            "* Session: {} Branch: {} (/resume or create a /new one)",
            short_id_string(&app.session_id.to_string()),
            short_id_string(&app.branch_id.to_string()),
        ),
        format!(
            "* Connection: {} Model: {} (switch with /use)",
            app.connection_id,
            app.effective_model(),
        ),
        "* Type /help for a list of commands".to_owned(),
    ];

    if let Some(tmux_warning) = tmux_warning {
        lines.push(String::new());
        lines.push(tmux_warning.to_owned());
    }

    lines.join("\n")
}

fn slash_panel_title(input: &str) -> &'static str {
    let head = input
        .trim_start()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default();
    match resolve_command(head) {
        Ok(command) if command.canonical == "resume" => "Resume",
        _ => "Commands",
    }
}

fn command_menu_label_width(items: &[&CommandMenuItem], width: usize) -> usize {
    let content_width = width.saturating_sub(2);
    if content_width <= 12 {
        return content_width;
    }
    let natural = items
        .iter()
        .map(|item| display_width(&item.label))
        .max()
        .unwrap_or(0);
    let bounded = natural.clamp(12, 28);
    bounded.min((content_width / 3).max(12))
}

fn render_approval_menu_entry(choice: ApprovalMenuChoice, selected: bool, width: usize) -> String {
    let prefix = if selected {
        COMPOSER_PREFIX
    } else {
        COMPOSER_CONTINUATION_PREFIX
    };
    let content_width = width.saturating_sub(display_width(prefix)).max(1);
    let label_width = 21usize.min(content_width).max(1);
    if content_width <= label_width.saturating_add(6) {
        return format!(
            "{prefix}{}",
            truncate_inline_text(choice.label(), content_width)
        );
    }

    let separator = "  ";
    let description_width = content_width
        .saturating_sub(label_width)
        .saturating_sub(display_width(separator));
    let label = truncate_inline_text(choice.label(), label_width);
    let padded_label = pad_to_display_width(&label, label_width);
    let description = truncate_inline_text(choice.description(), description_width);
    format!("{prefix}{padded_label}{separator}{description}")
}

fn render_command_menu_entry(
    item: &CommandMenuItem,
    selected: bool,
    width: usize,
    label_width: usize,
) -> String {
    let prefix = if selected {
        COMPOSER_PREFIX
    } else {
        COMPOSER_CONTINUATION_PREFIX
    };
    let content_width = width.saturating_sub(display_width(prefix)).max(1);
    if item.description.is_empty() || content_width <= label_width.saturating_add(8) {
        return format!(
            "{prefix}{}",
            truncate_inline_text(&item.label, content_width)
        );
    }

    let separator = "  ";
    let description_width = content_width
        .saturating_sub(label_width)
        .saturating_sub(display_width(separator));

    let label = truncate_inline_text(&item.label, label_width);
    let padded_label = pad_to_display_width(&label, label_width);
    let description = truncate_inline_text(&item.description, description_width);

    format!("{prefix}{padded_label}{separator}{description}")
}

fn truncate_inline_text(raw: &str, limit: usize) -> String {
    let count = display_width(raw);
    if count <= limit {
        return raw.to_owned();
    }
    if limit <= 3 {
        return ".".repeat(limit);
    }
    let keep = limit - 3;
    let head = take_prefix_by_display_width(raw, keep);
    format!("{head}...")
}

fn elapsed_label(started_at: Instant) -> String {
    let elapsed = started_at.elapsed().as_secs_f32();
    if elapsed >= 10.0 {
        format!("{elapsed:.0}s")
    } else {
        format!("{elapsed:.1}s")
    }
}

pub(super) fn pending_chat_placeholder(app: &ChatApp) -> Option<String> {
    let turn_is_in_flight = app.has_pending_request()
        || app.active_turn.live
        || !app.active_turn.cells.is_empty()
        || !app.active_stream_tail_lines().is_empty()
        || app.runtime_busy_for_controls();
    if !turn_is_in_flight {
        return None;
    }

    if let Some(task_status) = app.task_status.as_ref() {
        return Some(format!("• {}", task_status.header));
    }

    if let Some(command) = app.pending_shell_command.as_deref() {
        let suffix = app
            .pending_send_started_at
            .or(app.pending_approval_started_at)
            .or(app.active_turn.started_at)
            .map(|started_at| format!(" ({} • esc to interrupt)", elapsed_label(started_at)))
            .unwrap_or_default();
        Some(format!("• Running !{}{}", truncate_detail(command), suffix))
    } else {
        let label = match app
            .queue_inspection
            .as_ref()
            .map(|inspection| inspection.runtime_state)
        {
            Some(SessionRuntimeState::WaitingOnInput) => "Waiting on input".to_owned(),
            Some(SessionRuntimeState::WaitingOnApproval) => "Waiting on approval".to_owned(),
            Some(SessionRuntimeState::CancelRequested) => "Cancellation requested".to_owned(),
            _ if !app.pending_inputs.is_empty() => "Waiting on input".to_owned(),
            _ if !app.pending_approvals.is_empty()
                || app.pending_approval.is_some()
                || app.pending_approval_started_at.is_some() =>
            {
                "Waiting on approval".to_owned()
            }
            _ => "Working".to_owned(),
        };

        if let Some(started_at) = app
            .pending_send_started_at
            .or(app.pending_approval_started_at)
            .or(app.active_turn.started_at)
        {
            Some(format!(
                "• {label} ({} • esc to interrupt)",
                elapsed_label(started_at)
            ))
        } else {
            Some(format!("• {label}"))
        }
    }
}

fn pending_chat_placeholder_lines(app: &ChatApp, width: u16) -> Vec<Line<'static>> {
    let task_status_lines = render_task_status_lines(app, width);
    if !task_status_lines.is_empty() {
        return task_status_lines;
    }
    pending_chat_placeholder(app)
        .map(|line| vec![Line::from(line)])
        .unwrap_or_default()
}
