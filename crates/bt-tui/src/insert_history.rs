use std::fmt;
use std::io;
use std::io::Write;

#[path = "wrapping.rs"]
mod wrapping;

use crossterm::cursor::{MoveDown, MoveTo, MoveToColumn, RestorePosition, SavePosition};
use crossterm::queue;
use crossterm::style::Color as CrosstermColor;
use crossterm::style::{Colors, Print, ResetColor, SetAttribute, SetColors};
use ratatui::backend::Backend;
use ratatui::layout::Position;
use ratatui::style::Style;
use ratatui::text::Line;

use crate::custom_terminal;
use wrapping::adaptive_wrap_line;
use wrapping::line_contains_url_like;
use wrapping::line_has_mixed_url_and_non_url_tokens;

pub(super) fn insert_history_lines<B>(
    terminal: &mut custom_terminal::Terminal<B>,
    lines: Vec<Line<'static>>,
) -> io::Result<()>
where
    B: Backend + Write,
{
    if lines.is_empty() {
        return Ok(());
    }

    let screen_size = terminal.size()?;
    let mut area = terminal.viewport_area();
    let wrap_width = area.width.max(1) as usize;
    let mut wrapped = Vec::new();
    let mut wrapped_rows = 0usize;
    for line in &lines {
        let line_wrapped =
            if line_contains_url_like(line) && !line_has_mixed_url_and_non_url_tokens(line) {
                vec![line.clone()]
            } else {
                adaptive_wrap_line(line, wrap_width)
            };

        wrapped_rows += line_wrapped
            .iter()
            .map(|wrapped_line| history_line_physical_rows(wrapped_line, wrap_width) as usize)
            .sum::<usize>();
        wrapped.extend(line_wrapped);
    }
    let mut wrapped_rows = wrapped_rows as u16;
    if let Some((consumed_lines, consumed_rows)) =
        fill_visible_history_gap_prefix(terminal, &wrapped, wrap_width)?
    {
        terminal.note_history_rows_inserted(consumed_rows);
        if consumed_lines >= wrapped.len() {
            return Ok(());
        }
        wrapped.drain(..consumed_lines);
        wrapped_rows = wrapped_rows.saturating_sub(consumed_rows);
        area = terminal.viewport_area();
    }

    let mut should_update_area = false;

    if area.top() == 0 {
        return Ok(());
    }

    let last_cursor_pos = terminal
        .backend_mut()
        .get_cursor_position()
        .unwrap_or(Position {
            x: 0,
            y: area.bottom().saturating_sub(1),
        });
    let cursor_top = if area.bottom() < screen_size.height {
        let scroll_amount = wrapped_rows.min(screen_size.height - area.bottom());
        if scroll_amount > 0 {
            let top_1based = area.top() + 1;
            let writer = terminal.backend_mut();
            queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
            queue!(writer, MoveTo(0, area.top()))?;
            for _ in 0..scroll_amount {
                queue!(writer, Print("\x1bM"))?;
            }
            queue!(writer, ResetScrollRegion)?;

            let cursor_top = area.top().saturating_sub(1);
            area.y = area.y.saturating_add(scroll_amount);
            should_update_area = true;
            cursor_top
        } else {
            area.top().saturating_sub(1)
        }
    } else {
        area.top().saturating_sub(1)
    };

    {
        let writer = terminal.backend_mut();
        queue!(writer, SetScrollRegion(1..area.top()))?;
        queue!(writer, MoveTo(0, cursor_top))?;
        for line in &wrapped {
            queue!(writer, Print("\r\n"))?;
            write_history_line(writer, line, wrap_width)?;
        }
        queue!(writer, ResetScrollRegion)?;
    }

    if should_update_area {
        terminal.set_viewport_area(area);
        terminal.invalidate_viewport();
    }
    if wrapped_rows > 0 {
        terminal.note_history_rows_inserted(wrapped_rows);
    }
    terminal.set_cursor_position(last_cursor_pos)?;
    Ok(())
}

fn fill_visible_history_gap_prefix<B>(
    terminal: &mut custom_terminal::Terminal<B>,
    wrapped: &[Line<'static>],
    wrap_width: usize,
) -> io::Result<Option<(usize, u16)>>
where
    B: Backend + Write,
{
    let area = terminal.viewport_area();
    let visible_gap_rows = area.top().saturating_sub(terminal.visible_history_rows());
    if visible_gap_rows == 0 || wrapped.is_empty() {
        return Ok(None);
    }

    let mut consumed_lines = 0usize;
    let mut consumed_rows = 0u16;
    for line in wrapped {
        let line_rows = history_line_physical_rows(line, wrap_width);
        if consumed_rows.saturating_add(line_rows) > visible_gap_rows {
            break;
        }
        consumed_lines += 1;
        consumed_rows = consumed_rows.saturating_add(line_rows);
    }

    if consumed_lines == 0 {
        return Ok(None);
    }

    let last_cursor_pos = terminal
        .backend_mut()
        .get_cursor_position()
        .unwrap_or(Position {
            x: 0,
            y: area.bottom().saturating_sub(1),
        });
    let mut row = terminal.visible_history_rows();

    {
        let writer = terminal.backend_mut();
        for line in wrapped.iter().take(consumed_lines) {
            queue!(writer, MoveTo(0, row))?;
            write_history_line(writer, line, wrap_width)?;
            row = row.saturating_add(history_line_physical_rows(line, wrap_width));
        }
    }

    terminal.set_cursor_position(last_cursor_pos)?;
    Ok(Some((consumed_lines, consumed_rows)))
}

fn history_line_display_width(line: &Line<'static>) -> usize {
    if line.spans.is_empty() {
        return 0;
    }
    line.spans
        .iter()
        .map(|span| crate::custom_terminal::display_width(span.content.as_ref()))
        .sum()
}

fn history_line_physical_rows(line: &Line<'static>, wrap_width: usize) -> u16 {
    history_line_display_width(line)
        .max(1)
        .div_ceil(wrap_width)
        .min(usize::from(u16::MAX)) as u16
}

fn write_history_line<W: Write>(
    writer: &mut W,
    line: &Line<'static>,
    wrap_width: usize,
) -> io::Result<()> {
    let physical_rows = history_line_physical_rows(line, wrap_width);
    if physical_rows > 1 {
        queue!(writer, SavePosition)?;
        for _ in 1..physical_rows {
            queue!(writer, MoveDown(1), MoveToColumn(0))?;
            queue!(
                writer,
                crossterm::terminal::Clear(crossterm::terminal::ClearType::UntilNewLine)
            )?;
        }
        queue!(writer, RestorePosition)?;
    }
    queue!(
        writer,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::UntilNewLine)
    )?;
    for span in &line.spans {
        let style = span.style.patch(line.style);
        apply_style(writer, style)?;
        queue!(writer, Print(span.content.as_ref()))?;
    }
    queue!(
        writer,
        SetAttribute(crossterm::style::Attribute::Reset),
        ResetColor
    )?;
    Ok(())
}

fn apply_style(writer: &mut impl Write, style: Style) -> io::Result<()> {
    queue!(writer, SetAttribute(crossterm::style::Attribute::Reset))?;
    queue!(
        writer,
        SetColors(Colors::new(
            CrosstermColor::from(style.fg.unwrap_or(ratatui::style::Color::Reset)),
            CrosstermColor::from(style.bg.unwrap_or(ratatui::style::Color::Reset))
        ))
    )?;
    if style.add_modifier.contains(ratatui::style::Modifier::BOLD) {
        queue!(writer, SetAttribute(crossterm::style::Attribute::Bold))?;
    }
    if style.add_modifier.contains(ratatui::style::Modifier::DIM) {
        queue!(writer, SetAttribute(crossterm::style::Attribute::Dim))?;
    }
    if style
        .add_modifier
        .contains(ratatui::style::Modifier::ITALIC)
    {
        queue!(writer, SetAttribute(crossterm::style::Attribute::Italic))?;
    }
    if style
        .add_modifier
        .contains(ratatui::style::Modifier::UNDERLINED)
    {
        queue!(
            writer,
            SetAttribute(crossterm::style::Attribute::Underlined)
        )?;
    }
    if style
        .add_modifier
        .contains(ratatui::style::Modifier::REVERSED)
    {
        queue!(writer, SetAttribute(crossterm::style::Attribute::Reverse))?;
    }
    Ok(())
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
