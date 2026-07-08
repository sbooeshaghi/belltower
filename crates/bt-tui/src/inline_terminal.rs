use std::fmt;
use std::io;
use std::io::Write;

use super::*;
use crossterm::cursor::MoveDown;
use crossterm::cursor::MoveTo;
use crossterm::cursor::MoveToColumn;
use crossterm::cursor::RestorePosition;
use crossterm::cursor::SavePosition;
use crossterm::queue;
use crossterm::style::Attribute;
use crossterm::style::Print;
use crossterm::style::ResetColor;
use crossterm::style::SetAttribute;
use crossterm::terminal::ClearType;
use ratatui::backend::Backend;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;
use ratatui::style::Color;
use url::Url;

pub(super) struct TerminalGuard;

const BOTTOM_SURFACE_BACKGROUND: Color = Color::Rgb(28, 28, 28);

impl TerminalGuard {
    pub(super) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        )?;
        if let Err(error) = execute!(stdout, EnableBracketedPaste) {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags
        );
        let _ = disable_raw_mode();
    }
}

pub(super) fn create_inline_terminal(
    viewport_height: u16,
) -> io::Result<custom_terminal::Terminal<CrosstermBackend<io::Stdout>>> {
    custom_terminal::Terminal::with_inline_viewport(
        CrosstermBackend::new(io::stdout()),
        viewport_height.max(1),
    )
}

pub(super) fn desired_inline_viewport_height(
    app: &mut ChatApp,
    view_width: u16,
    screen_height: u16,
) -> u16 {
    let shell_height = bottom_shell_height(app, view_width.max(1));
    let hard_max = screen_height.max(1);
    let preferred_max = screen_height.saturating_sub(MIN_SCROLLBACK_HISTORY_HEIGHT);
    let min_live_height = u16::from(app.active_turn_is_live());
    let max_viewport_height = preferred_max
        .max(
            shell_height
                .saturating_add(min_live_height)
                .max(MIN_INLINE_VIEWPORT_HEIGHT),
        )
        .min(hard_max);
    let live_height_budget = max_viewport_height.saturating_sub(shell_height);
    let live_transcript_height = if live_height_budget == 0 {
        0
    } else {
        app.active_view_height(view_width.max(1))
            .min(live_height_budget)
    };

    shell_height
        .saturating_add(live_transcript_height)
        .clamp(MIN_INLINE_VIEWPORT_HEIGHT, max_viewport_height.max(1))
}

pub(super) fn hard_clear_terminal<B: Backend + Write>(
    terminal: &mut custom_terminal::Terminal<B>,
) -> io::Result<()> {
    terminal.clear_scrollback_and_visible_screen_ansi()?;
    terminal.clear()?;
    Ok(())
}

pub(super) fn clear_visible_terminal<B: Backend + Write>(
    terminal: &mut custom_terminal::Terminal<B>,
) -> io::Result<()> {
    terminal.clear_visible_screen()
}

pub(super) fn process_terminal_actions<B: Backend + Write>(
    terminal: &mut custom_terminal::Terminal<B>,
    app: &mut ChatApp,
) -> io::Result<()> {
    if app.pending_hard_clear {
        hard_clear_terminal(terminal)?;
        app.pending_hard_clear = false;
    }
    if app.pending_visible_clear {
        clear_visible_terminal(terminal)?;
        app.pending_visible_clear = false;
    }
    if let Some(lines) = app.pending_scrollback_reset_lines.take() {
        crate::insert_history::insert_history_lines(terminal, lines)?;
    } else if let Some(text) = app.pending_scrollback_reset_text.take() {
        insert_scrollback_text(terminal, app.transcript_output_width, &text)?;
    }
    Ok(())
}

pub(super) fn sync_transcript_scrollback<B: Backend + Write>(
    terminal: &mut custom_terminal::Terminal<B>,
    app: &mut ChatApp,
) -> io::Result<()> {
    let Some(lines) = app.take_new_history_lines() else {
        return Ok(());
    };
    crate::insert_history::insert_history_lines(terminal, lines)
}

fn insert_scrollback_text<B: Backend + Write>(
    terminal: &mut custom_terminal::Terminal<B>,
    transcript_output_width: u16,
    text: &str,
) -> io::Result<()> {
    let screen_size = terminal.size()?;
    let mut area = terminal.viewport_area();
    let wrap_width = usize::from(transcript_output_width.max(1));
    let rendered_lines = prepare_scrollback_lines(text, wrap_width);
    let wrapped_rows = rendered_lines
        .iter()
        .map(|line| line.physical_rows)
        .sum::<u16>();
    let mut should_update_area = false;

    if area.bottom() < screen_size.height {
        let scroll_amount = wrapped_rows.min(screen_size.height - area.bottom());
        if scroll_amount > 0 {
            shift_viewport_down(terminal, scroll_amount, area.top(), screen_size.height)?;
            area.y = area.y.saturating_add(scroll_amount);
            should_update_area = true;
        }
    }

    if area.top() == 0 {
        return Ok(());
    }

    let last_cursor = {
        let backend = terminal.backend_mut();
        backend.get_cursor_position().unwrap_or(Position {
            x: 0,
            y: area.bottom().saturating_sub(1),
        })
    };
    let history_bottom = area.top().saturating_sub(1);

    {
        let writer = terminal.backend_mut();
        queue!(writer, SetAttribute(Attribute::Reset), ResetColor)?;
        queue!(writer, SetScrollRegion(1..area.top()))?;
        queue!(writer, MoveTo(0, history_bottom))?;
        for line in &rendered_lines {
            queue!(
                writer,
                SetAttribute(Attribute::Reset),
                ResetColor,
                Print("\r\n")
            )?;
            write_scrollback_line(writer, line)?;
        }
        queue!(
            writer,
            SetAttribute(Attribute::Reset),
            ResetColor,
            ResetScrollRegion,
            MoveTo(last_cursor.x, last_cursor.y)
        )?;
        std::io::Write::flush(writer)?;
    }

    if should_update_area {
        terminal.set_viewport_area(area);
        terminal.invalidate_viewport();
    }
    if wrapped_rows > 0 {
        terminal.note_history_rows_inserted(wrapped_rows);
    }

    Ok(())
}

fn shift_viewport_down<B: Backend + Write>(
    terminal: &mut custom_terminal::Terminal<B>,
    lines: u16,
    viewport_top: u16,
    screen_height: u16,
) -> io::Result<()> {
    if lines == 0 {
        return Ok(());
    }

    let writer = terminal.backend_mut();
    let top_1based = viewport_top.saturating_add(1);
    queue!(writer, SetScrollRegion(top_1based..screen_height))?;
    queue!(writer, MoveTo(0, viewport_top))?;
    for _ in 0..lines {
        queue!(writer, Print("\x1bM"))?;
    }
    queue!(writer, ResetScrollRegion)?;
    std::io::Write::flush(writer)
}

fn write_scrollback_line(writer: &mut impl Write, line: &ScrollbackLine) -> io::Result<()> {
    if line.physical_rows > 1 {
        queue!(writer, SavePosition)?;
        for _ in 1..line.physical_rows {
            queue!(writer, MoveDown(1), MoveToColumn(0))?;
            queue!(writer, crossterm::terminal::Clear(ClearType::UntilNewLine))?;
        }
        queue!(writer, RestorePosition)?;
    }
    queue!(
        writer,
        crossterm::terminal::Clear(ClearType::UntilNewLine),
        Print(&line.text)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScrollbackLine {
    text: String,
    physical_rows: u16,
}

fn prepare_scrollback_lines(raw: &str, wrap_width: usize) -> Vec<ScrollbackLine> {
    let wrap_width = wrap_width.max(1);
    let mut prepared = Vec::new();

    for raw_line in raw.split('\n') {
        if raw_line.is_empty() {
            prepared.push(ScrollbackLine {
                text: String::new(),
                physical_rows: 1,
            });
            continue;
        }

        let physical_rows = display_width(raw_line).max(1).div_ceil(wrap_width);
        if should_preserve_terminal_wrapped_line(raw_line) {
            prepared.push(ScrollbackLine {
                text: raw_line.to_owned(),
                physical_rows: physical_rows.min(usize::from(u16::MAX)) as u16,
            });
        } else {
            prepared.extend(
                wrap_plain_text(raw_line, wrap_width)
                    .into_iter()
                    .map(|line| ScrollbackLine {
                        text: line,
                        physical_rows: 1,
                    }),
            );
        }
    }

    if prepared.is_empty() {
        prepared.push(ScrollbackLine {
            text: String::new(),
            physical_rows: 1,
        });
    }

    prepared
}

fn should_preserve_terminal_wrapped_line(raw_line: &str) -> bool {
    let trimmed = raw_line.trim();
    !trimmed.is_empty() && !trimmed.chars().any(char::is_whitespace) && Url::parse(trimmed).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SetScrollRegion(pub std::ops::Range<u16>);

impl crossterm::Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("use ANSI");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResetScrollRegion;

impl crossterm::Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("use ANSI");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

pub(super) fn draw_chat(frame: &mut custom_terminal::Frame<'_>, app: &mut ChatApp) {
    let area = frame.area();
    app.set_input_view_width(composer_wrap_width(area.width));
    app.set_bottom_panel_view_width(area.width);
    let bottom_surface = render_bottom_surface(app);
    let shell_aux_lines = shell_aux_lines(app, area.width);
    let shell_aux_height = shell_aux_height(app, area.width);
    let composer_height = composer_body_height(app);
    let popup_height = popup_or_footer_height(app);
    let shell_gap_height = shell_top_gap_height();
    let shell_height = shell_gap_height
        .saturating_add(shell_aux_height)
        .saturating_add(composer_height)
        .saturating_add(popup_height);
    let live_height_budget = area.height.saturating_sub(shell_height);
    let mut live_transcript_lines = app.active_view_lines(area.width);
    if live_height_budget > 0 {
        let rendered_height = Paragraph::new(Text::from(live_transcript_lines.clone()))
            .wrap(Wrap { trim: false })
            .line_count(area.width.max(1));
        if rendered_height > usize::from(live_height_budget) {
            let start = live_transcript_lines
                .len()
                .saturating_sub(usize::from(live_height_budget));
            live_transcript_lines.drain(..start);
        }
    } else {
        live_transcript_lines.clear();
    }
    let live_transcript_height = live_transcript_lines.len().min(usize::from(u16::MAX)) as u16;
    let chunks = chat_layout_chunks(
        area,
        live_transcript_height.min(live_height_budget),
        shell_gap_height,
        shell_aux_height,
        composer_height,
        popup_height,
    );
    let transcript_area = chunks[0];
    let shell_aux_area = chunks[2];
    let input_area = chunks[3];
    let bottom_area = chunks[4];
    app.set_message_view_size(transcript_area.width, transcript_area.height);
    app.set_input_view_width(composer_wrap_width(input_area.width));
    app.set_bottom_panel_view_width(bottom_area.width);
    if live_transcript_height > 0 {
        let live_area = Rect::new(
            transcript_area.x,
            transcript_area.y,
            transcript_area.width,
            live_transcript_height.min(transcript_area.height),
        );
        frame.render_widget(Paragraph::new(Text::from(live_transcript_lines)), live_area);
    }

    if shell_aux_area.height > 0 {
        let widget = Paragraph::new(Text::from(shell_aux_lines))
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(BOTTOM_SURFACE_BACKGROUND));
        frame.render_widget(widget, shell_aux_area);
    }

    if input_area.height > 0 {
        let (_, input_scroll_row, _) = app.visible_input_lines();
        let input_text_area = composer_text_area_rect(input_area);
        let input_background = Paragraph::new(Text::default()).style(user_surface_style());
        frame.render_widget(input_background, input_area);
        let input = Paragraph::new(render_input_text(app))
            .wrap(Wrap { trim: false })
            .style(user_surface_style());
        frame.render_widget(input, input_text_area);
        let (cursor_col, cursor_row) = app.input_cursor_position();
        let prefix_width = display_width(COMPOSER_PREFIX).min(usize::from(u16::MAX)) as u16;
        let max_cursor_col = input_text_area.width.saturating_sub(prefix_width + 1);
        let max_cursor_row = input_text_area.height.saturating_sub(1);
        frame.set_cursor_position((
            input_text_area.x + prefix_width + cursor_col.min(max_cursor_col),
            input_text_area.y
                + cursor_row
                    .saturating_sub(input_scroll_row)
                    .min(max_cursor_row),
        ));
    }

    if bottom_area.height == 0 {
        return;
    }

    let widget = Paragraph::new(Text::from(bottom_surface.lines))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(BOTTOM_SURFACE_BACKGROUND));
    frame.render_widget(widget, bottom_area);
    if let Some((cursor_col, cursor_row)) = bottom_surface.cursor {
        frame.set_cursor_position((
            bottom_area.x + cursor_col.min(bottom_area.width.saturating_sub(1)),
            bottom_area.y + cursor_row.min(bottom_area.height.saturating_sub(1)),
        ));
    }
}

pub(super) fn render_input_text(app: &ChatApp) -> Text<'static> {
    let (visible_input_lines, scroll_row, _) = app.visible_input_lines();
    let lines = if app.composer.input.is_empty() {
        vec![Line::from(vec![
            Span::raw(COMPOSER_PREFIX),
            Span::styled(
                INPUT_PLACEHOLDER.to_owned(),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ])]
    } else {
        visible_input_lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let prefix = if scroll_row == 0 && index == 0 {
                    COMPOSER_PREFIX
                } else {
                    COMPOSER_CONTINUATION_PREFIX
                };
                Line::from(vec![Span::raw(prefix), Span::raw(line)])
            })
            .collect::<Vec<_>>()
    };
    Text::from(lines)
}

pub(super) fn chat_layout_chunks(
    area: Rect,
    live_transcript_height: u16,
    shell_gap_height: u16,
    shell_aux_height: u16,
    composer_height: u16,
    popup_height: u16,
) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(live_transcript_height),
            Constraint::Length(shell_gap_height),
            Constraint::Length(shell_aux_height),
            Constraint::Length(composer_height),
            Constraint::Length(popup_height),
            Constraint::Min(0),
        ])
        .split(area)
        .iter()
        .copied()
        .take(5)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::Backend;
    use ratatui::backend::ClearType;
    use ratatui::backend::TestBackend;
    use ratatui::backend::WindowSize;
    use ratatui::layout::Size;
    use std::io;
    use std::io::Write;

    #[derive(Debug, Clone)]
    struct TestWriteBackend {
        inner: TestBackend,
        writes: Vec<u8>,
    }

    impl TestWriteBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                inner: TestBackend::new(width, height),
                writes: Vec::new(),
            }
        }

        fn ansi_output(&self) -> String {
            String::from_utf8_lossy(&self.writes).into_owned()
        }
    }

    impl Write for TestWriteBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for TestWriteBackend {
        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            Backend::draw(&mut self.inner, content)
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            Backend::hide_cursor(&mut self.inner)
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            Backend::show_cursor(&mut self.inner)
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Backend::get_cursor_position(&mut self.inner)
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            Backend::set_cursor_position(&mut self.inner, position)
        }

        fn clear(&mut self) -> io::Result<()> {
            Backend::clear(&mut self.inner)
        }

        fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
            Backend::clear_region(&mut self.inner, clear_type)
        }

        fn size(&self) -> io::Result<Size> {
            Backend::size(&self.inner)
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            Backend::window_size(&mut self.inner)
        }

        fn flush(&mut self) -> io::Result<()> {
            Backend::flush(&mut self.inner)
        }
    }

    #[test]
    fn scrollback_insertion_with_space_below_viewport_uses_reverse_index_shift() {
        let mut backend = TestWriteBackend::new(20, 10);
        backend
            .set_cursor_position(Position::new(0, 5))
            .expect("set cursor");
        let mut terminal =
            custom_terminal::Terminal::with_inline_viewport(backend, 3).expect("terminal");

        insert_scrollback_text(&mut terminal, 20, "history line").expect("insert scrollback");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 6, 20, 3));

        let output = terminal.backend_mut().ansi_output();
        assert!(output.contains("\u{1b}[6;10r"));
        assert!(output.contains("\u{1b}M"));
    }

    #[test]
    fn history_insertion_consumes_visible_gap_before_scrolling_remainder() {
        let mut backend = TestWriteBackend::new(20, 10);
        backend
            .set_cursor_position(Position::new(0, 5))
            .expect("set cursor");
        let mut terminal =
            custom_terminal::Terminal::with_inline_viewport(backend, 3).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 7, 20, 3));

        crate::insert_history::insert_history_lines(
            &mut terminal,
            vec![
                Line::from("first"),
                Line::from("second"),
                Line::from("third"),
            ],
        )
        .expect("insert history");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 7, 20, 3));
        assert_eq!(terminal.visible_history_rows(), 7);
        let output = terminal.backend_mut().ansi_output();
        assert!(output.contains("first"));
        assert!(output.contains("second"));
        assert!(output.contains("third"));
        assert!(output.contains("\u{1b}[6;1H"));
        assert!(output.contains("\u{1b}[7;1H"));
        assert!(
            !output.contains("\u{1b}M"),
            "history should consume the visible gap before using the existing scroll region"
        );
    }

    #[test]
    fn history_insertion_uses_codex_reverse_index_shift_before_viewport() {
        let mut backend = TestWriteBackend::new(20, 10);
        backend
            .set_cursor_position(Position::new(0, 5))
            .expect("set cursor");
        let mut terminal =
            custom_terminal::Terminal::with_inline_viewport(backend, 3).expect("terminal");

        crate::insert_history::insert_history_lines(
            &mut terminal,
            vec![Line::from("first"), Line::from("second")],
        )
        .expect("insert history");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 7, 20, 3));
        assert_eq!(terminal.visible_history_rows(), 7);
        let output = terminal.backend_mut().ansi_output();
        assert!(output.contains("\u{1b}M"));
        assert!(
            output.contains("\u{1b}[6;1H"),
            "history should append at the old viewport boundary after shifting the viewport down"
        );
        assert!(output.contains("first"));
        assert!(output.contains("second"));
    }

    #[test]
    fn chat_layout_stacks_live_transcript_above_bottom_shell() {
        let chunks = chat_layout_chunks(Rect::new(0, 10, 80, 12), 3, 1, 2, 2, 1);

        assert_eq!(chunks[0], Rect::new(0, 10, 80, 3));
        assert_eq!(chunks[1], Rect::new(0, 13, 80, 1));
        assert_eq!(chunks[2], Rect::new(0, 14, 80, 2));
        assert_eq!(chunks[3], Rect::new(0, 16, 80, 2));
        assert_eq!(chunks[4], Rect::new(0, 18, 80, 1));
    }

    #[test]
    fn chat_layout_keeps_aux_rows_above_composer_and_footer() {
        let chunks = chat_layout_chunks(Rect::new(0, 13, 80, 6), 0, 1, 1, 3, 1);

        assert_eq!(chunks[1], Rect::new(0, 13, 80, 1));
        assert_eq!(chunks[2], Rect::new(0, 14, 80, 1));
        assert_eq!(chunks[3], Rect::new(0, 15, 80, 3));
        assert_eq!(chunks[4], Rect::new(0, 18, 80, 1));
    }
}
