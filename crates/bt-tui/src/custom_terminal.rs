use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Color as CrosstermColor;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::ResetColor;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::terminal::Clear;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;
pub(crate) fn display_width(s: &str) -> usize {
    if !s.contains('\x1B') {
        return UnicodeWidthStr::width(s);
    }

    let mut visible = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1B' && chars.clone().next() == Some(']') {
            chars.next();
            for c in chars.by_ref() {
                if c == '\x07' {
                    break;
                }
            }
            continue;
        }
        visible.push(ch);
    }
    UnicodeWidthStr::width(visible.as_str())
}

pub(super) struct Frame<'a> {
    cursor_position: Option<Position>,
    viewport_area: Rect,
    buffer: &'a mut Buffer,
}

impl Frame<'_> {
    pub(super) const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub(super) fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    pub(super) fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }
}

pub(super) struct Terminal<B>
where
    B: Backend + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    hidden_cursor: bool,
    viewport_area: Rect,
    last_known_screen_size: Size,
    last_known_cursor_pos: Position,
    visible_history_rows: u16,
}

impl<B> Terminal<B>
where
    B: Backend + Write,
{
    pub(super) fn with_inline_viewport(mut backend: B, viewport_height: u16) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend
            .get_cursor_position()
            .unwrap_or(Position { x: 0, y: 0 });
        let height = viewport_height.max(1).min(screen_size.height.max(1));
        let area = Rect::new(0, cursor_pos.y, screen_size.width, height);
        Ok(Self {
            backend,
            buffers: [Buffer::empty(area), Buffer::empty(area)],
            current: 0,
            hidden_cursor: false,
            viewport_area: area,
            last_known_screen_size: screen_size,
            last_known_cursor_pos: cursor_pos,
            visible_history_rows: area.top(),
        })
    }

    pub(super) fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub(super) fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    pub(super) fn autoresize(&mut self) -> io::Result<()> {
        let size = self.size()?;
        if size != self.last_known_screen_size {
            let current_area = self.viewport_area;
            let height = current_area.height.min(size.height.max(1)).max(1);
            let max_y = size.height.saturating_sub(height);
            let next_y = match self.backend.get_cursor_position() {
                Ok(cursor_pos) => {
                    let delta = i32::from(cursor_pos.y) - i32::from(self.last_known_cursor_pos.y);
                    (i32::from(current_area.y) + delta).clamp(0, i32::from(max_y)) as u16
                }
                Err(_) => current_area.y.min(max_y),
            };
            let next_area = Rect::new(0, next_y, size.width, height);
            if next_area != current_area {
                self.set_viewport_area(next_area);
            }
            self.last_known_screen_size = size;
        }
        Ok(())
    }

    pub(super) fn set_viewport_area(&mut self, area: Rect) {
        self.current_buffer_mut().resize(area);
        self.previous_buffer_mut().resize(area);
        self.viewport_area = area;
        self.visible_history_rows = self.visible_history_rows.min(area.top());
    }

    pub(super) fn prepare_viewport(&mut self, height: u16) -> io::Result<()> {
        self.autoresize()?;
        let size = self.last_known_screen_size;
        let mut area = self.viewport_area;
        area.height = height.max(1).min(size.height.max(1));
        area.width = size.width;

        if area.bottom() > size.height {
            let scroll_by = area.bottom().saturating_sub(size.height);
            if scroll_by > 0 && area.top() > 0 {
                scroll_region_up(&mut self.backend, 0..area.top(), scroll_by)?;
            }
            area.y = size.height.saturating_sub(area.height);
        }

        if area != self.viewport_area {
            self.clear()?;
            self.set_viewport_area(area);
        }
        Ok(())
    }

    pub(super) fn draw<F>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.autoresize()?;
        self.current_buffer_mut().reset();

        let mut frame = Frame {
            cursor_position: None,
            viewport_area: self.viewport_area,
            buffer: self.current_buffer_mut(),
        };
        render_callback(&mut frame);
        let cursor_position = frame.cursor_position;

        self.flush()?;

        match cursor_position {
            Some(position) => {
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
            None => self.hide_cursor()?,
        }

        self.swap_buffers();
        Backend::flush(&mut self.backend)?;
        Ok(())
    }

    pub(super) fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.backend
            .set_cursor_position(self.viewport_area.as_position())?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    pub(super) fn clear_visible_screen(&mut self) -> io::Result<()> {
        let home = Position { x: 0, y: 0 };
        self.set_cursor_position(home)?;
        self.backend.clear_region(ClearType::All)?;
        self.set_cursor_position(home)?;
        Backend::flush(&mut self.backend)?;
        self.visible_history_rows = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    pub(super) fn clear_scrollback_and_visible_screen_ansi(&mut self) -> io::Result<()> {
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        Backend::flush(&mut self.backend)?;
        self.last_known_cursor_pos = Position { x: 0, y: 0 };
        self.visible_history_rows = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    pub(super) fn invalidate_viewport(&mut self) {
        self.previous_buffer_mut().reset();
    }

    pub(super) fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    pub(super) fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    pub(super) fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    pub(super) fn viewport_area(&self) -> Rect {
        self.viewport_area
    }

    pub(super) fn visible_history_rows(&self) -> u16 {
        self.visible_history_rows
    }

    pub(super) fn note_history_rows_inserted(&mut self, inserted_rows: u16) {
        self.visible_history_rows = self
            .visible_history_rows
            .saturating_add(inserted_rows)
            .min(self.viewport_area.top());
    }

    fn current_buffer(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    fn swap_buffers(&mut self) {
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
    }

    fn flush(&mut self) -> io::Result<()> {
        let updates = diff_buffers(self.previous_buffer(), self.current_buffer());
        if let Some((x, y)) = updates.iter().rev().find_map(|command| match command {
            DrawCommand::Put { x, y, .. } => Some((*x, *y)),
            DrawCommand::ClearToEnd { .. } => None,
        }) {
            self.last_known_cursor_pos = Position { x, y };
        }
        draw_commands(&mut self.backend, updates.into_iter())
    }
}

impl<B> Drop for Terminal<B>
where
    B: Backend + Write,
{
    fn drop(&mut self) {
        let _ = self.show_cursor();
    }
}

enum DrawCommand {
    Put { x: u16, y: u16, cell: Cell },
    ClearToEnd { x: u16, y: u16, bg: Color },
}

fn diff_buffers(previous: &Buffer, current: &Buffer) -> Vec<DrawCommand> {
    let previous_buffer = &previous.content;
    let next_buffer = &current.content;

    let mut updates = vec![];
    for y in 0..previous.area.height {
        let row_start = y as usize * previous.area.width as usize;
        let row_end = row_start + previous.area.width as usize;
        let previous_row = &previous_buffer[row_start..row_end];
        let current_row = &next_buffer[row_start..row_end];
        if previous_row == current_row {
            continue;
        }

        let bg = current_row
            .last()
            .map(|cell| cell.bg)
            .unwrap_or(Color::Reset);

        let mut last_nonblank_column = None;
        let mut column = 0usize;
        while column < current_row.len() {
            let cell = &current_row[column];
            let width = display_width(cell.symbol());
            if cell.symbol() != " " || cell.bg != bg || cell.modifier != Modifier::empty() {
                last_nonblank_column = Some(column + width.saturating_sub(1));
            }
            column += width.max(1);
        }

        let (x, y) = previous.pos_of(row_start);
        updates.push(DrawCommand::ClearToEnd { x, y, bg });

        if let Some(limit) = last_nonblank_column.map(|column| column as u16) {
            for (offset, cell) in current_row.iter().enumerate() {
                if cell.skip {
                    continue;
                }
                let (x, y) = previous.pos_of(row_start + offset);
                if x <= limit {
                    updates.push(DrawCommand::Put {
                        x,
                        y,
                        cell: cell.clone(),
                    });
                }
            }
        }
    }

    updates
}

fn draw_commands<I>(writer: &mut impl Write, commands: I) -> io::Result<()>
where
    I: Iterator<Item = DrawCommand>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_pos: Option<Position> = None;

    for command in commands {
        let (x, y) = match command {
            DrawCommand::Put { x, y, .. } => (x, y),
            DrawCommand::ClearToEnd { x, y, .. } => (x, y),
        };
        if !matches!(last_pos, Some(p) if x == p.x + 1 && y == p.y) {
            queue!(writer, MoveTo(x, y))?;
        }
        last_pos = Some(Position { x, y });

        match command {
            DrawCommand::Put { cell, .. } => {
                if cell.modifier != modifier {
                    queue!(writer, SetAttribute(crossterm::style::Attribute::Reset))?;
                    modifier = cell.modifier;
                    fg = Color::Reset;
                    bg = Color::Reset;
                    if modifier.contains(Modifier::BOLD) {
                        queue!(writer, SetAttribute(crossterm::style::Attribute::Bold))?;
                    }
                    if modifier.contains(Modifier::DIM) {
                        queue!(writer, SetAttribute(crossterm::style::Attribute::Dim))?;
                    }
                    if modifier.contains(Modifier::ITALIC) {
                        queue!(writer, SetAttribute(crossterm::style::Attribute::Italic))?;
                    }
                    if modifier.contains(Modifier::UNDERLINED) {
                        queue!(
                            writer,
                            SetAttribute(crossterm::style::Attribute::Underlined)
                        )?;
                    }
                    if modifier.contains(Modifier::REVERSED) {
                        queue!(writer, SetAttribute(crossterm::style::Attribute::Reverse))?;
                    }
                }
                if cell.fg != fg || cell.bg != bg {
                    queue!(
                        writer,
                        SetColors(Colors::new(
                            CrosstermColor::from(cell.fg),
                            CrosstermColor::from(cell.bg)
                        ))
                    )?;
                    fg = cell.fg;
                    bg = cell.bg;
                }
                queue!(writer, Print(cell.symbol()))?;
            }
            DrawCommand::ClearToEnd { bg: clear_bg, .. } => {
                queue!(writer, SetAttribute(crossterm::style::Attribute::Reset))?;
                modifier = Modifier::empty();
                queue!(writer, SetBackgroundColor(CrosstermColor::from(clear_bg)))?;
                bg = clear_bg;
                queue!(writer, Clear(crossterm::terminal::ClearType::UntilNewLine))?;
            }
        }
    }

    queue!(
        writer,
        SetAttribute(crossterm::style::Attribute::Reset),
        ResetColor
    )?;
    Ok(())
}

fn scroll_region_up(
    writer: &mut impl Write,
    region: std::ops::Range<u16>,
    scroll_by: u16,
) -> io::Result<()> {
    if scroll_by == 0 || region.start >= region.end {
        return Ok(());
    }

    let bottom = region.end.saturating_sub(1);
    queue!(
        writer,
        SetScrollRegion(region.start.saturating_add(1)..region.end)
    )?;
    queue!(writer, MoveTo(0, bottom))?;
    for _ in 0..scroll_by {
        queue!(writer, Print("\r\n"))?;
    }
    queue!(writer, ResetScrollRegion)?;
    std::io::Write::flush(writer)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SetScrollRegion(pub std::ops::Range<u16>);

impl crossterm::Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
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
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::Backend;
    use ratatui::backend::ClearType;
    use ratatui::backend::TestBackend;
    use ratatui::backend::WindowSize;
    use ratatui::style::Style;
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

        fn resize(&mut self, width: u16, height: u16) {
            self.inner.resize(width, height);
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
            I: Iterator<Item = (u16, u16, &'a Cell)>,
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

    fn apply_updates(area: Rect, previous: &Buffer, updates: &[DrawCommand]) -> Vec<String> {
        let mut screen = (0..area.height)
            .map(|y| {
                let row_start = y as usize * area.width as usize;
                previous.content[row_start..row_start + area.width as usize]
                    .iter()
                    .map(|cell| cell.symbol().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        for command in updates {
            match command {
                DrawCommand::Put { x, y, cell } => {
                    screen[*y as usize][*x as usize] = cell.symbol().to_owned();
                }
                DrawCommand::ClearToEnd { x, y, .. } => {
                    for cell in screen[*y as usize]
                        .iter_mut()
                        .take(usize::from(area.width))
                        .skip(usize::from(*x))
                    {
                        *cell = " ".to_owned();
                    }
                }
            }
        }

        screen
            .into_iter()
            .map(|row| row.into_iter().collect::<String>())
            .collect()
    }

    fn buffer_lines(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                let row_start = y as usize * buffer.area.width as usize;
                buffer.content[row_start..row_start + buffer.area.width as usize]
                    .iter()
                    .map(|cell| cell.symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn diff_buffers_clears_fully_blank_rows_from_column_zero() {
        let area = Rect::new(0, 0, 8, 2);
        let mut previous = Buffer::empty(area);
        let current = Buffer::empty(area);

        previous.set_string(0, 1, "Goodbye!", Style::default());

        let updates = diff_buffers(&previous, &current);

        assert!(
            updates
                .iter()
                .any(|command| { matches!(command, DrawCommand::ClearToEnd { x: 0, y: 1, .. }) })
        );
    }

    #[test]
    fn diff_buffers_overwrite_short_input_after_placeholder_prefix() {
        let area = Rect::new(0, 0, 24, 1);
        let mut previous = Buffer::empty(area);
        let mut current = Buffer::empty(area);

        previous.set_string(0, 0, "› Type to chat", Style::default());
        current.set_string(0, 0, "› draft", Style::default());

        let updates = diff_buffers(&previous, &current);
        let applied = apply_updates(area, &previous, &updates);

        assert_eq!(applied, buffer_lines(&current));
    }

    #[test]
    fn diff_buffers_restore_placeholder_after_short_input() {
        let area = Rect::new(0, 0, 24, 1);
        let mut previous = Buffer::empty(area);
        let mut current = Buffer::empty(area);

        previous.set_string(0, 0, "› draft", Style::default());
        current.set_string(0, 0, "› Type to chat", Style::default());

        let updates = diff_buffers(&previous, &current);
        let applied = apply_updates(area, &previous, &updates);

        assert_eq!(applied, buffer_lines(&current));
    }

    #[test]
    fn diff_buffers_handle_multiline_composer_growth_without_stale_prefix_chars() {
        let area = Rect::new(0, 0, 32, 4);
        let mut previous = Buffer::empty(area);
        let mut current = Buffer::empty(area);

        previous.set_string(0, 0, "› Type to chat. /help", Style::default());
        previous.set_string(0, 1, "Session: demo | local (model)", Style::default());
        previous.set_string(0, 2, "/tmp/belltower", Style::default());

        current.set_string(0, 0, "› draft", Style::default());
        current.set_string(0, 1, "  x", Style::default());

        let updates = diff_buffers(&previous, &current);
        let applied = apply_updates(area, &previous, &updates);

        assert_eq!(applied, buffer_lines(&current));
    }

    #[test]
    fn diff_buffers_clear_rows_when_bottom_shell_content_shifts_down() {
        let area = Rect::new(0, 0, 32, 5);
        let mut previous = Buffer::empty(area);
        let mut current = Buffer::empty(area);

        previous.set_string(0, 1, "› Type to chat. /help", Style::default());
        previous.set_string(0, 2, "Session: demo | local (model)", Style::default());
        previous.set_string(0, 3, "/tmp/belltower", Style::default());

        current.set_string(0, 2, "› draft", Style::default());
        current.set_string(0, 3, "  x", Style::default());

        let updates = diff_buffers(&previous, &current);
        let applied = apply_updates(area, &previous, &updates);

        assert_eq!(applied, buffer_lines(&current));
    }

    #[test]
    fn prepare_viewport_preserves_viewport_after_screen_growth_when_height_is_unchanged() {
        let mut backend = TestWriteBackend::new(20, 10);
        backend
            .set_cursor_position(Position::new(0, 6))
            .expect("set cursor");
        let mut terminal = Terminal::with_inline_viewport(backend, 4).expect("inline terminal");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 6, 20, 4));

        terminal.backend_mut().resize(20, 12);
        terminal.autoresize().expect("autoresize");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 6, 20, 4));

        terminal.prepare_viewport(4).expect("prepare viewport");
        assert_eq!(terminal.viewport_area(), Rect::new(0, 6, 20, 4));
    }

    #[test]
    fn autoresize_tracks_cursor_delta_when_terminal_resize_moves_viewport() {
        let mut backend = TestWriteBackend::new(20, 10);
        backend
            .set_cursor_position(Position::new(0, 6))
            .expect("set cursor");
        let mut terminal = Terminal::with_inline_viewport(backend, 4).expect("inline terminal");

        terminal
            .set_cursor_position(Position::new(0, 9))
            .expect("set cursor");
        terminal.backend_mut().resize(20, 12);
        terminal
            .backend_mut()
            .set_cursor_position(Position::new(0, 11))
            .expect("backend cursor");

        terminal.autoresize().expect("autoresize");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 8, 20, 4));
    }

    #[test]
    fn prepare_viewport_keeps_viewport_top_stable_when_height_shrinks() {
        let mut backend = TestWriteBackend::new(20, 10);
        backend
            .set_cursor_position(Position::new(0, 6))
            .expect("set cursor");
        let mut terminal = Terminal::with_inline_viewport(backend, 4).expect("inline terminal");

        terminal.prepare_viewport(2).expect("prepare viewport");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 6, 20, 2));
    }

    #[test]
    fn prepare_viewport_growth_uses_available_space_below_before_scrolling_history() {
        let mut backend = TestWriteBackend::new(20, 12);
        backend
            .set_cursor_position(Position::new(0, 6))
            .expect("set cursor");
        let mut terminal = Terminal::with_inline_viewport(backend, 2).expect("inline terminal");

        terminal.prepare_viewport(4).expect("prepare viewport");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 6, 20, 4));
    }

    #[test]
    fn prepare_viewport_growth_scrolls_history_above_viewport_instead_of_erasing_it() {
        let mut backend = TestWriteBackend {
            inner: TestBackend::with_lines([
                "row0    ", "row1    ", "row2    ", "row3    ", "row4    ", "row5    ",
            ]),
            writes: Vec::new(),
        };
        backend
            .set_cursor_position(Position::new(0, 3))
            .expect("set cursor");
        let mut terminal = Terminal::with_inline_viewport(backend, 2).expect("inline terminal");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 3, 8, 2));

        terminal.prepare_viewport(4).expect("prepare viewport");

        assert_eq!(terminal.viewport_area(), Rect::new(0, 2, 8, 4));
        let ansi = String::from_utf8_lossy(&terminal.backend.writes);
        assert!(
            ansi.contains("\x1b[1;3r"),
            "expected viewport growth to emit a scroll-region shift before reanchoring"
        );
    }

    #[test]
    fn visible_history_rows_track_inserted_lines_and_reset_on_clear() {
        let mut backend = TestWriteBackend::new(20, 10);
        backend
            .set_cursor_position(Position::new(0, 6))
            .expect("set cursor");
        let mut terminal = Terminal::with_inline_viewport(backend, 4).expect("inline terminal");

        assert_eq!(terminal.visible_history_rows(), 6);

        terminal.note_history_rows_inserted(2);
        assert_eq!(terminal.visible_history_rows(), 6);

        terminal.note_history_rows_inserted(20);
        assert_eq!(
            terminal.visible_history_rows(),
            terminal.viewport_area().top()
        );

        terminal
            .clear_visible_screen()
            .expect("clear should reset history rows");
        assert_eq!(terminal.visible_history_rows(), 0);
    }
}
