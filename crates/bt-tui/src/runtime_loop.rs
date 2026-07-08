use super::*;
use crossterm::SynchronizedUpdate;
use ratatui::backend::Backend;
use std::io::stdout;

const ACTIVE_TURN_POLL_INTERVAL: Duration = Duration::from_millis(16);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChatOpenMode {
    New,
    ResumePicker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChatExit {
    Quit,
    Detached,
}

pub(super) async fn run_chat(
    client: BelltowerClient,
    args: ChatArgs,
    mode: ChatOpenMode,
) -> Result<ChatExit, Box<dyn Error>> {
    let mut trace = StartupTrace::from_env("bt-tui");
    trace.mark("tui.start");
    trace.mark("session.open.start");
    let selection = open_chat_session(&client, &args, mode).await?;
    trace.mark("session.open.done");

    trace.mark("app.construct.start");
    let mut app = ChatApp::new(
        client,
        selection.session_id,
        selection.branch_id,
        selection.project_root,
        selection.connection_id,
    );
    app.assistant_commit_batch_size = Some(1);
    trace.mark("app.construct.done");

    trace.mark("terminal.enter.start");
    let _guard = TerminalGuard::enter()?;
    let (initial_width, initial_height) = crossterm::terminal::size()?;
    app.set_transcript_output_width(initial_width);
    app.set_input_view_width(composer_wrap_width(initial_width));
    app.set_bottom_panel_view_width(initial_width);
    let mut terminal = create_inline_terminal(desired_inline_viewport_height(
        &mut app,
        initial_width,
        initial_height,
    ))?;
    terminal.clear()?;
    app.pending_scrollback_reset_lines = Some(app.compose_scrollback_reset_lines());
    app.pending_hard_clear = false;
    app.pending_visible_clear = false;
    trace.mark("terminal.enter.done");

    trace.mark("first_paint.start");
    draw_chat_frame(&mut terminal, &mut app)?;
    trace.mark("first_paint.done");

    trace.mark("post_paint.metadata_refresh.start");
    app.refresh().await?;
    trace.mark("post_paint.metadata_refresh.done");
    trace.mark("post_paint.sse_connect.start");
    app.ensure_event_stream().await?;
    trace.mark("post_paint.sse_connect.done");

    loop {
        if let Err(error) = app.poll_background().await {
            app.show_error(format!("background update failed: {error}"));
        }
        app.run_stream_commit_tick();
        app.flush_pending_escape_prefix_if_expired(Instant::now());
        if app.should_quit {
            break;
        }
        draw_chat_frame(&mut terminal, &mut app)?;
        if app.should_quit {
            break;
        }

        if event::poll(poll_interval(&app))? {
            handle_terminal_event(&mut terminal, &mut app, event::read()?).await?;
        } else {
            app.flush_pending_escape_prefix_if_expired(Instant::now());
            if !app.should_quit {
                handle_idle_refresh(&mut app).await?;
            }
        }
    }

    Ok(if app.detach_requested {
        ChatExit::Detached
    } else {
        ChatExit::Quit
    })
}

fn draw_chat_frame(
    terminal: &mut custom_terminal::Terminal<impl Backend + Write>,
    app: &mut ChatApp,
) -> Result<(), Box<dyn Error>> {
    let terminal_size = terminal.size()?;
    app.set_transcript_output_width(terminal_size.width);
    app.set_input_view_width(composer_wrap_width(terminal_size.width));
    app.set_bottom_panel_view_width(terminal_size.width);
    stdout().sync_update(|_| {
        process_terminal_actions(terminal, app)?;
        let draw_height =
            desired_inline_viewport_height(app, terminal_size.width, terminal_size.height);
        terminal.prepare_viewport(draw_height)?;
        sync_transcript_scrollback(terminal, app)?;
        terminal.draw(|frame| draw_chat(frame, app))
    })??;
    Ok(())
}

fn poll_interval(app: &ChatApp) -> Duration {
    let base = if app.active_turn_is_live() {
        ACTIVE_TURN_POLL_INTERVAL
    } else {
        IDLE_POLL_INTERVAL
    };
    app.pending_escape_prefix_poll_delay(Instant::now())
        .map_or(base, |delay| delay.min(base))
}

async fn handle_terminal_event(
    terminal: &mut custom_terminal::Terminal<impl Backend + Write>,
    app: &mut ChatApp,
    event: TerminalEvent,
) -> Result<(), Box<dyn Error>> {
    match event {
        TerminalEvent::Key(key)
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
        {
            if let Some(action) = app.handle_key(key) {
                dispatch_chat_action(app, action).await;
            }
        }
        TerminalEvent::Paste(text) => {
            app.handle_paste(&text);
        }
        TerminalEvent::Resize(width, _) => {
            terminal.autoresize()?;
            terminal.invalidate_viewport();
            let resized_width = terminal.size()?.width.max(width);
            app.prepare_scrollback_reflow_for_resize(resized_width);
        }
        _ => {}
    }

    Ok(())
}

async fn dispatch_chat_action(app: &mut ChatApp, action: ChatAction) {
    match action {
        ChatAction::Send => {
            if let Err(error) = app.send_input().await {
                app.show_error(format!("send failed: {error}"));
            }
        }
        ChatAction::Refresh => {
            if let Err(error) = app.refresh().await {
                app.show_error(format!("refresh failed: {error}"));
            }
        }
        ChatAction::Cancel => {
            if let Err(error) = app.cancel_active_turn(None).await {
                app.show_error(format!("cancel failed: {error}"));
            }
        }
        ChatAction::ApproveLastPendingOnce => {
            if let Err(error) = app
                .start_resolve_last_pending(true, ApprovalScope::Once, None)
                .await
            {
                app.show_error(format!("approval failed: {error}"));
            }
        }
        ChatAction::ApproveLastPendingSession => {
            if let Err(error) = app
                .start_resolve_last_pending(true, ApprovalScope::Session, None)
                .await
            {
                app.show_error(format!("approval failed: {error}"));
            }
        }
        ChatAction::ApproveLastPendingAlways => {
            if let Err(error) = app
                .start_resolve_last_pending(true, ApprovalScope::Always, None)
                .await
            {
                app.show_error(format!("approval failed: {error}"));
            }
        }
        ChatAction::DenyLastPending => {
            if let Err(error) = app
                .start_resolve_last_pending(false, ApprovalScope::Once, None)
                .await
            {
                app.show_error(format!("denial failed: {error}"));
            }
        }
        ChatAction::PreviousBranch => {
            if let Err(error) = app.cycle_branch(-1).await {
                app.show_error(format!("branch switch failed: {error}"));
            }
        }
        ChatAction::NextBranch => {
            if let Err(error) = app.cycle_branch(1).await {
                app.show_error(format!("branch switch failed: {error}"));
            }
        }
        ChatAction::PreviousSession => {
            if let Err(error) = app.cycle_session(-1).await {
                app.show_error(format!("session switch failed: {error}"));
            }
        }
        ChatAction::NextSession => {
            if let Err(error) = app.cycle_session(1).await {
                app.show_error(format!("session switch failed: {error}"));
            }
        }
        ChatAction::NewSession => {
            if let Err(error) = app.create_session(None).await {
                app.show_error(format!("session creation failed: {error}"));
            }
        }
        ChatAction::CreateBranch => {
            if let Err(error) = app.create_branch(None).await {
                app.show_error(format!("branch creation failed: {error}"));
            }
        }
    }
}

async fn handle_idle_refresh(app: &mut ChatApp) -> Result<(), Box<dyn Error>> {
    if app.pending_session_load.is_none()
        && !app.has_pending_request()
        && app.last_metadata_refresh.elapsed() >= METADATA_REFRESH_INTERVAL
    {
        if let Err(error) = app.refresh_metadata().await {
            app.show_error(format!("refresh failed: {error}"));
        }
    } else if app.pending_session_load.is_none()
        && !app.has_pending_request()
        && app.stream_task.is_none()
        && app.last_refresh.elapsed() >= FALLBACK_REFRESH_INTERVAL
        && let Err(error) = app.refresh().await
    {
        app.show_error(format!("refresh failed: {error}"));
    }

    Ok(())
}
