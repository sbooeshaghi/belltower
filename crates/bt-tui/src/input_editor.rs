use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundKeyAction {
    Escape,
    CancelOrQuit,
    Submit,
    Complete,
    HistoryUp,
    HistoryDown,
    Backspace,
    DeleteChar,
    DeleteWordRight,
    MoveCursorLeft,
    MoveCursorRight,
    Refresh,
    PreviousSession,
    NextSession,
    NewSession,
    CreateBranch,
    MoveCursorStart,
    MoveCursorEnd,
    KillAfterCursor,
    KillBeforeCursor,
    DeleteWordLeft,
    KillWordLeft,
    MoveWordLeft,
    MoveWordRight,
    InsertNewline,
    PreviousBranch,
    NextBranch,
    Yank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeyBinding {
    code: KeyCode,
    modifiers: KeyModifiers,
    action: BoundKeyAction,
}

const KEY_BINDINGS: &[KeyBinding] = &[
    KeyBinding {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::Escape,
    },
    KeyBinding {
        code: KeyCode::Char('c'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::CancelOrQuit,
    },
    KeyBinding {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::Submit,
    },
    KeyBinding {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::Complete,
    },
    KeyBinding {
        code: KeyCode::Up,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::HistoryUp,
    },
    KeyBinding {
        code: KeyCode::Down,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::HistoryDown,
    },
    KeyBinding {
        code: KeyCode::Backspace,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::Backspace,
    },
    KeyBinding {
        code: KeyCode::Backspace,
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::DeleteWordLeft,
    },
    KeyBinding {
        code: KeyCode::Char('\u{7f}'),
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::DeleteWordLeft,
    },
    KeyBinding {
        code: KeyCode::Char('\u{8}'),
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::DeleteWordLeft,
    },
    KeyBinding {
        code: KeyCode::Delete,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::DeleteChar,
    },
    KeyBinding {
        code: KeyCode::Delete,
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::DeleteWordRight,
    },
    KeyBinding {
        code: KeyCode::Left,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::MoveCursorLeft,
    },
    KeyBinding {
        code: KeyCode::Left,
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::MoveWordLeft,
    },
    KeyBinding {
        code: KeyCode::Right,
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::MoveCursorRight,
    },
    KeyBinding {
        code: KeyCode::Right,
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::MoveWordRight,
    },
    KeyBinding {
        code: KeyCode::Char('l'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::Refresh,
    },
    KeyBinding {
        code: KeyCode::Char('p'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::PreviousSession,
    },
    KeyBinding {
        code: KeyCode::Char('n'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::NextSession,
    },
    KeyBinding {
        code: KeyCode::Char('t'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::NewSession,
    },
    KeyBinding {
        code: KeyCode::Char('b'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::CreateBranch,
    },
    KeyBinding {
        code: KeyCode::Char('a'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::MoveCursorStart,
    },
    KeyBinding {
        code: KeyCode::Char('e'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::MoveCursorEnd,
    },
    KeyBinding {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::KillAfterCursor,
    },
    KeyBinding {
        code: KeyCode::Char('u'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::KillBeforeCursor,
    },
    KeyBinding {
        code: KeyCode::Char('w'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::KillWordLeft,
    },
    KeyBinding {
        code: KeyCode::Char('y'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::Yank,
    },
    KeyBinding {
        code: KeyCode::Char('b'),
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::MoveWordLeft,
    },
    KeyBinding {
        code: KeyCode::Char('f'),
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::MoveWordRight,
    },
    KeyBinding {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::SHIFT,
        action: BoundKeyAction::InsertNewline,
    },
    KeyBinding {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::ALT,
        action: BoundKeyAction::InsertNewline,
    },
    KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::CONTROL,
        action: BoundKeyAction::InsertNewline,
    },
    KeyBinding {
        code: KeyCode::Char('['),
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::PreviousBranch,
    },
    KeyBinding {
        code: KeyCode::Char(']'),
        modifiers: KeyModifiers::NONE,
        action: BoundKeyAction::NextBranch,
    },
];

fn line_start_for_cursor(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end_for_cursor(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map_or(text.len(), |offset| cursor + offset)
}

fn byte_offset_for_display_column(line: &str, target_column: usize) -> usize {
    let mut offset = 0;
    for (index, ch) in line.char_indices() {
        let next = index + ch.len_utf8();
        if display_width(&line[..next]) > target_column {
            break;
        }
        offset = next;
        if display_width(&line[..next]) == target_column {
            break;
        }
    }
    offset
}

fn composer_row_text_end(row: &WrappedTextRow) -> usize {
    row.start_byte + row.text.len()
}

fn composer_visual_row_index(rows: &[WrappedTextRow], cursor: usize) -> usize {
    rows.partition_point(|row| row.start_byte <= cursor)
        .saturating_sub(1)
        .min(rows.len().saturating_sub(1))
}

fn word_modifier_pressed(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(
        KeyModifiers::ALT | KeyModifiers::META | KeyModifiers::SUPER | KeyModifiers::HYPER,
    )
}

fn modified_word_edit_action(key: KeyEvent) -> Option<BoundKeyAction> {
    if !word_modifier_pressed(key.modifiers) {
        return None;
    }

    match key.code {
        KeyCode::Backspace | KeyCode::Char('\u{7f}') | KeyCode::Char('\u{8}') => {
            Some(BoundKeyAction::DeleteWordLeft)
        }
        KeyCode::Delete => Some(BoundKeyAction::DeleteWordRight),
        KeyCode::Left | KeyCode::Char('b') => Some(BoundKeyAction::MoveWordLeft),
        KeyCode::Right | KeyCode::Char('f') => Some(BoundKeyAction::MoveWordRight),
        _ => None,
    }
}

impl ChatApp {
    fn bound_action_for_key(&self, key: KeyEvent) -> Option<BoundKeyAction> {
        if let Some(action) = modified_word_edit_action(key) {
            return Some(action);
        }
        KEY_BINDINGS
            .iter()
            .find(|binding| binding.code == key.code && binding.modifiers == key.modifiers)
            .map(|binding| binding.action)
    }

    fn is_plain_escape_key(key: KeyEvent) -> bool {
        key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE
    }

    fn escape_prefixed_action_for_key(&self, key: KeyEvent) -> Option<BoundKeyAction> {
        if key.modifiers != KeyModifiers::NONE {
            return None;
        }
        match key.code {
            KeyCode::Backspace | KeyCode::Char('\u{7f}') | KeyCode::Char('\u{8}') => {
                Some(BoundKeyAction::DeleteWordLeft)
            }
            _ => None,
        }
    }

    pub(super) fn pending_escape_prefix_poll_delay(&self, now: Instant) -> Option<Duration> {
        let started_at = self.pending_escape_prefix_at?;
        Some(ESCAPE_PREFIX_TIMEOUT.saturating_sub(now.saturating_duration_since(started_at)))
    }

    pub(super) fn flush_pending_escape_prefix_if_expired(&mut self, now: Instant) {
        let Some(started_at) = self.pending_escape_prefix_at else {
            return;
        };
        if now.saturating_duration_since(started_at) < ESCAPE_PREFIX_TIMEOUT {
            return;
        }
        self.pending_escape_prefix_at = None;
        self.handle_escape();
    }

    fn apply_bound_key_action(&mut self, action: BoundKeyAction) -> Option<ChatAction> {
        match action {
            BoundKeyAction::Escape => {
                self.handle_escape();
                None
            }
            BoundKeyAction::CancelOrQuit => {
                if self.turn_is_busy() {
                    Some(ChatAction::Cancel)
                } else {
                    self.should_quit = true;
                    None
                }
            }
            BoundKeyAction::Submit => self.submit_from_composer(),
            BoundKeyAction::Complete => {
                self.apply_input_completion();
                None
            }
            BoundKeyAction::HistoryUp => {
                self.navigate_up_in_composer();
                None
            }
            BoundKeyAction::HistoryDown => {
                self.navigate_down_in_composer();
                None
            }
            BoundKeyAction::Backspace => {
                self.backspace_input_char();
                None
            }
            BoundKeyAction::DeleteChar => {
                self.delete_input_char();
                None
            }
            BoundKeyAction::DeleteWordRight => {
                self.delete_input_word_right();
                None
            }
            BoundKeyAction::MoveCursorLeft => {
                self.move_input_cursor_left();
                None
            }
            BoundKeyAction::MoveCursorRight => {
                self.move_input_cursor_right();
                None
            }
            BoundKeyAction::Refresh => Some(ChatAction::Refresh),
            BoundKeyAction::PreviousSession => Some(ChatAction::PreviousSession),
            BoundKeyAction::NextSession => Some(ChatAction::NextSession),
            BoundKeyAction::NewSession => Some(ChatAction::NewSession),
            BoundKeyAction::CreateBranch => Some(ChatAction::CreateBranch),
            BoundKeyAction::MoveCursorStart => {
                self.move_input_cursor_start();
                None
            }
            BoundKeyAction::MoveCursorEnd => {
                self.move_input_cursor_end();
                None
            }
            BoundKeyAction::KillAfterCursor => {
                self.kill_input_after_cursor();
                None
            }
            BoundKeyAction::KillBeforeCursor => {
                self.kill_input_before_cursor();
                None
            }
            BoundKeyAction::DeleteWordLeft => {
                self.delete_input_word_left();
                None
            }
            BoundKeyAction::KillWordLeft => {
                self.kill_input_word_left();
                None
            }
            BoundKeyAction::MoveWordLeft => {
                self.move_input_cursor_word_left();
                None
            }
            BoundKeyAction::MoveWordRight => {
                self.move_input_cursor_word_right();
                None
            }
            BoundKeyAction::InsertNewline => {
                self.insert_input_newline();
                None
            }
            BoundKeyAction::PreviousBranch => Some(ChatAction::PreviousBranch),
            BoundKeyAction::NextBranch => Some(ChatAction::NextBranch),
            BoundKeyAction::Yank => {
                self.yank_input();
                None
            }
        }
    }

    fn measured_input_panel_height(&self) -> u16 {
        self.input_line_count()
            .clamp(INPUT_PANEL_MIN_HEIGHT, INPUT_PANEL_MAX_HEIGHT)
    }

    pub(super) fn move_input_cursor_left(&mut self) {
        self.composer.cursor = previous_char_boundary(&self.composer.input, self.composer.cursor);
        self.composer.preferred_column = None;
    }

    pub(super) fn move_input_cursor_right(&mut self) {
        self.composer.cursor = next_char_boundary(&self.composer.input, self.composer.cursor);
        self.composer.preferred_column = None;
    }

    pub(super) fn move_input_cursor_word_left(&mut self) {
        self.composer.cursor = previous_word_boundary(&self.composer.input, self.composer.cursor);
        self.composer.preferred_column = None;
    }

    pub(super) fn move_input_cursor_word_right(&mut self) {
        self.composer.cursor = next_word_boundary(&self.composer.input, self.composer.cursor);
        self.composer.preferred_column = None;
    }

    pub(super) fn move_input_cursor_start(&mut self) {
        self.composer.cursor = 0;
        self.composer.preferred_column = None;
    }

    pub(super) fn move_input_cursor_end(&mut self) {
        self.composer.cursor = self.composer.input.len();
        self.composer.preferred_column = None;
    }

    pub(super) fn move_input_cursor_vertical(&mut self, direction: i32) -> bool {
        if self.composer.input.is_empty() {
            return false;
        }

        let width = usize::from(self.input_view_width().max(1));
        let rows = wrap_composer_input_with_end_indices(&self.composer.input, width);
        let cursor = self.composer.cursor.min(self.composer.input.len());
        if rows.len() > 1 {
            let current_index = composer_visual_row_index(&rows, cursor);
            let current_row = &rows[current_index];
            let target_column = self.composer.preferred_column.unwrap_or_else(|| {
                let cursor_end =
                    cursor.clamp(current_row.start_byte, composer_row_text_end(current_row));
                display_width(&self.composer.input[current_row.start_byte..cursor_end])
            });

            let target_index = if direction < 0 {
                if current_index == 0 {
                    if cursor == 0 {
                        return false;
                    }
                    self.composer.cursor = 0;
                    self.composer.preferred_column = None;
                    return true;
                }
                current_index - 1
            } else {
                if current_index + 1 >= rows.len() {
                    if cursor == self.composer.input.len() {
                        return false;
                    }
                    self.composer.cursor = self.composer.input.len();
                    self.composer.preferred_column = None;
                    return true;
                }
                current_index + 1
            };

            self.composer.preferred_column.get_or_insert(target_column);
            let target_row = &rows[target_index];
            let target_end = composer_row_text_end(target_row);
            let target_line = &self.composer.input[target_row.start_byte..target_end];
            let target_offset = byte_offset_for_display_column(target_line, target_column);
            self.composer.cursor = target_row.start_byte + target_offset;
            return true;
        }

        let cursor = self.composer.cursor.min(self.composer.input.len());
        let line_start = line_start_for_cursor(&self.composer.input, cursor);
        let line_end = line_end_for_cursor(&self.composer.input, cursor);
        let target_column = self
            .composer
            .preferred_column
            .unwrap_or_else(|| display_width(&self.composer.input[line_start..cursor]));

        let Some((target_start, target_end)) = (if direction < 0 {
            if line_start == 0 {
                None
            } else {
                let previous_end = line_start - 1;
                let previous_start = self.composer.input[..previous_end]
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
                Some((previous_start, previous_end))
            }
        } else if line_end >= self.composer.input.len() {
            None
        } else {
            let next_start = line_end + 1;
            let next_end = self.composer.input[next_start..]
                .find('\n')
                .map_or(self.composer.input.len(), |offset| next_start + offset);
            Some((next_start, next_end))
        }) else {
            return false;
        };

        self.composer.preferred_column.get_or_insert(target_column);
        let target_line = &self.composer.input[target_start..target_end];
        let target_offset = byte_offset_for_display_column(target_line, target_column);
        self.composer.cursor = target_start + target_offset;
        true
    }

    pub(super) fn clear_input_history_browse(&mut self) {
        self.composer.history_browse_index = None;
        self.composer.history_browse_draft = None;
    }

    pub(super) fn clear_composer_preferred_column(&mut self) {
        self.composer.preferred_column = None;
    }

    pub(super) fn sent_user_input_history(&self) -> Vec<String> {
        self.messages
            .iter()
            .filter(|message| message.role == Role::User)
            .filter_map(user_message_history_entry)
            .collect()
    }

    pub(super) fn browse_input_history(&mut self, direction: i32) {
        let history = self.sent_user_input_history();
        if history.is_empty() {
            return;
        }

        let max_index = history.len().saturating_sub(1);
        let next_index = match self.composer.history_browse_index {
            Some(current) => {
                if direction < 0 {
                    Some((current + 1).min(max_index))
                } else {
                    current.checked_sub(1)
                }
            }
            None if direction < 0 => {
                self.composer.history_browse_draft = Some(self.composer.input.clone());
                Some(0)
            }
            None => None,
        };

        match next_index {
            Some(index) => {
                let history_index = max_index.saturating_sub(index);
                self.composer.input = history[history_index].clone();
                self.composer.cursor = self.composer.input.len();
                self.clear_composer_preferred_column();
                self.composer.history_browse_index = Some(index);
                self.clamp_command_menu_selection();
            }
            None => {
                if let Some(draft) = self.composer.history_browse_draft.take() {
                    self.composer.input = draft;
                    self.composer.cursor = self.composer.input.len();
                    self.clear_composer_preferred_column();
                }
                self.composer.history_browse_index = None;
                self.clamp_command_menu_selection();
            }
        }
    }

    pub(super) fn insert_input_newline(&mut self) {
        self.insert_input_text("\n");
    }

    pub(super) fn insert_input_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.clear_input_history_browse();
        self.composer.input.insert_str(self.composer.cursor, text);
        self.composer.cursor += text.len();
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    pub(super) fn insert_input_char(&mut self, ch: char) {
        let mut buffer = [0; 4];
        self.insert_input_text(ch.encode_utf8(&mut buffer));
    }

    pub(super) fn handle_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        self.insert_input_text(&normalized);
    }

    pub(super) fn backspace_input_char(&mut self) {
        if self.composer.cursor == 0 {
            return;
        }
        self.clear_input_history_browse();
        let start = previous_char_boundary(&self.composer.input, self.composer.cursor);
        self.composer
            .input
            .replace_range(start..self.composer.cursor, "");
        self.composer.cursor = start;
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    pub(super) fn delete_input_char(&mut self) {
        if self.composer.cursor >= self.composer.input.len() {
            return;
        }
        self.clear_input_history_browse();
        let end = next_char_boundary(&self.composer.input, self.composer.cursor);
        self.composer
            .input
            .replace_range(self.composer.cursor..end, "");
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    pub(super) fn delete_input_word_left(&mut self) {
        if self.composer.cursor == 0 {
            return;
        }
        self.clear_input_history_browse();
        let start = previous_word_boundary(&self.composer.input, self.composer.cursor);
        self.composer
            .input
            .replace_range(start..self.composer.cursor, "");
        self.composer.cursor = start;
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    pub(super) fn kill_input_word_left(&mut self) {
        if self.composer.cursor == 0 {
            return;
        }
        self.clear_input_history_browse();
        let start = previous_word_boundary(&self.composer.input, self.composer.cursor);
        self.kill_input_range(start..self.composer.cursor);
        self.composer.cursor = start;
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    pub(super) fn delete_input_word_right(&mut self) {
        if self.composer.cursor >= self.composer.input.len() {
            return;
        }
        self.clear_input_history_browse();
        let end = next_word_boundary(&self.composer.input, self.composer.cursor);
        self.composer
            .input
            .replace_range(self.composer.cursor..end, "");
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    pub(super) fn kill_input_after_cursor(&mut self) {
        self.clear_input_history_browse();
        let cursor = self.composer.cursor;
        let line_end = line_end_for_cursor(&self.composer.input, cursor);
        let range = if cursor == line_end {
            if line_end < self.composer.input.len() {
                Some(cursor..line_end + 1)
            } else {
                None
            }
        } else {
            Some(cursor..line_end)
        };
        if let Some(range) = range {
            self.kill_input_range(range);
        }
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    pub(super) fn kill_input_before_cursor(&mut self) {
        if self.composer.cursor == 0 {
            return;
        }
        self.clear_input_history_browse();
        let cursor = self.composer.cursor;
        let line_start = line_start_for_cursor(&self.composer.input, cursor);
        let new_cursor = if cursor == line_start {
            line_start.saturating_sub(1)
        } else {
            line_start
        };
        let range = if cursor == line_start {
            if line_start > 0 {
                Some(line_start - 1..line_start)
            } else {
                None
            }
        } else {
            Some(line_start..cursor)
        };
        if let Some(range) = range {
            self.kill_input_range(range);
        }
        self.composer.cursor = new_cursor;
        self.clear_composer_preferred_column();
        self.clamp_command_menu_selection();
    }

    fn kill_input_range(&mut self, range: std::ops::Range<usize>) {
        if range.start >= range.end {
            return;
        }
        let removed = self.composer.input[range.clone()].to_owned();
        if removed.is_empty() {
            return;
        }
        self.composer.kill_buffer = removed;
        self.composer.input.replace_range(range, "");
        self.clear_composer_preferred_column();
    }

    fn yank_input(&mut self) {
        if self.composer.kill_buffer.is_empty() {
            return;
        }
        let text = self.composer.kill_buffer.clone();
        self.insert_input_text(&text);
    }

    pub(super) fn input_line_count(&self) -> u16 {
        let (_, cursor_row) = self.input_cursor_position();
        self.input_visual_lines()
            .len()
            .max(usize::from(cursor_row) + 1)
            .min(usize::from(u16::MAX)) as u16
    }

    pub(super) fn input_panel_height(&self) -> u16 {
        self.measured_input_panel_height()
    }

    pub(super) fn input_cursor_position(&self) -> (u16, u16) {
        let width = usize::from(self.input_view_width().max(1));
        wrapped_composer_cursor_position(&self.composer.input, self.composer.cursor, width)
    }

    pub(super) fn input_visual_lines(&self) -> Vec<String> {
        wrap_composer_input(
            &self.composer.input,
            usize::from(self.input_view_width().max(1)),
        )
    }

    pub(super) fn visible_input_lines(&self) -> (Vec<String>, u16, u16) {
        let mut lines = self.input_visual_lines();
        let (_, cursor_row) = self.input_cursor_position();
        while lines.len() <= usize::from(cursor_row) {
            lines.push(String::new());
        }
        let content_height = self.input_panel_height().max(1);
        let scroll_row = cursor_row.saturating_sub(content_height.saturating_sub(1));
        let start = usize::from(scroll_row);
        let end = (start + usize::from(content_height)).min(lines.len());
        let visible = if start < end {
            lines[start..end].to_vec()
        } else {
            vec![String::new()]
        };
        (visible, scroll_row, content_height)
    }

    pub(super) fn question_panel_is_active(&self) -> bool {
        !self.has_pending_request()
            && !self.composer.input.trim_start().starts_with('/')
            && !self.pending_inputs.is_empty()
    }

    pub(super) fn approval_menu_is_active(&self) -> bool {
        self.pending_approval.is_none() && !self.pending_approvals.is_empty()
    }

    pub(super) fn clamp_approval_menu_selection(&mut self) {
        let len = ApprovalMenuChoice::all().len();
        if self.pending_approvals.is_empty() || len == 0 {
            self.bottom_shell.approval_menu_selection = 0;
        } else if self.bottom_shell.approval_menu_selection >= len {
            self.bottom_shell.approval_menu_selection = len - 1;
        }
    }

    pub(super) fn clamp_question_choice_selection(&mut self) {
        let len = self
            .pending_inputs
            .first()
            .map(|pending| pending.choices.len())
            .unwrap_or(0);
        if len == 0 {
            self.bottom_shell.question_choice_selection = 0;
        } else if self.bottom_shell.question_choice_selection >= len {
            self.bottom_shell.question_choice_selection = len - 1;
        }
    }

    pub(super) fn select_previous_question_choice(&mut self) {
        if !self.question_panel_is_active() {
            return;
        }
        let len = self
            .pending_inputs
            .first()
            .map(|pending| pending.choices.len())
            .unwrap_or(0);
        if len == 0 {
            return;
        }
        self.bottom_shell.question_choice_selection = self
            .bottom_shell
            .question_choice_selection
            .saturating_sub(1);
    }

    pub(super) fn select_next_question_choice(&mut self) {
        if !self.question_panel_is_active() {
            return;
        }
        let len = self
            .pending_inputs
            .first()
            .map(|pending| pending.choices.len())
            .unwrap_or(0);
        if len == 0 {
            return;
        }
        self.bottom_shell.question_choice_selection =
            (self.bottom_shell.question_choice_selection + 1).min(len - 1);
    }

    pub(super) fn selected_question_choice(&self) -> Option<String> {
        if !self.question_panel_is_active() {
            return None;
        }
        self.pending_inputs.first().and_then(|pending| {
            pending
                .choices
                .get(self.bottom_shell.question_choice_selection)
                .cloned()
        })
    }

    pub(super) fn select_previous_approval_choice(&mut self) {
        let len = ApprovalMenuChoice::all().len();
        if !self.approval_menu_is_active() || len == 0 {
            return;
        }
        self.bottom_shell.approval_menu_selection =
            self.bottom_shell.approval_menu_selection.saturating_sub(1);
    }

    pub(super) fn select_next_approval_choice(&mut self) {
        let len = ApprovalMenuChoice::all().len();
        if !self.approval_menu_is_active() || len == 0 {
            return;
        }
        self.bottom_shell.approval_menu_selection =
            (self.bottom_shell.approval_menu_selection + 1).min(len - 1);
    }

    pub(super) fn selected_approval_choice(&self) -> Option<ApprovalMenuChoice> {
        self.approval_menu_is_active()
            .then(|| {
                ApprovalMenuChoice::all()
                    .get(self.bottom_shell.approval_menu_selection)
                    .copied()
            })
            .flatten()
    }

    pub(super) fn current_command_menu_items(&self) -> Vec<CommandMenuItem> {
        command_menu_items(
            &self.composer.input,
            &self.connection_id,
            self.connection_model.as_deref(),
            &self.connections,
            self.connection_status.as_ref(),
            &self.connection_models,
            &self.sessions,
            &self.project_root,
        )
    }

    pub(super) fn should_show_command_menu(&self) -> bool {
        if !self.composer.input.trim_start().starts_with('/') {
            return false;
        }

        let trimmed = self.composer.input.trim_start();
        if trimmed == "/inspect" {
            return false;
        }

        let items = self.current_command_menu_items();
        if items.is_empty() {
            return false;
        }

        if trimmed.ends_with(char::is_whitespace) || items.len() > 1 {
            return true;
        }

        let replacement_start = if trimmed.ends_with(char::is_whitespace) {
            trimmed.len()
        } else {
            trimmed
                .rfind(char::is_whitespace)
                .map_or(0, |index| index + 1)
        };
        let current_prefix = &trimmed[replacement_start..];
        items.iter().any(|item| item.insert_text != current_prefix)
    }

    pub(super) fn clamp_command_menu_selection(&mut self) {
        let len = self.current_command_menu_items().len();
        if len == 0 {
            self.bottom_shell.command_menu_selection = 0;
            self.bottom_shell.command_menu_scroll_top = 0;
        } else if self.bottom_shell.command_menu_selection >= len {
            self.bottom_shell.command_menu_selection = len - 1;
        }
        let visible_rows = crate::bottom_pane::command_menu_visible_row_count();
        let max_scroll = len.saturating_sub(visible_rows);
        self.bottom_shell.command_menu_scroll_top =
            self.bottom_shell.command_menu_scroll_top.min(max_scroll);
        if len == 0 {
            return;
        }
        let selection = self.bottom_shell.command_menu_selection;
        if selection < self.bottom_shell.command_menu_scroll_top {
            self.bottom_shell.command_menu_scroll_top = selection;
        } else {
            let bottom = self
                .bottom_shell
                .command_menu_scroll_top
                .saturating_add(visible_rows.saturating_sub(1));
            if selection > bottom {
                self.bottom_shell.command_menu_scroll_top =
                    selection.saturating_add(1).saturating_sub(visible_rows);
            }
        }
    }

    pub(super) fn select_previous_command_menu_item(&mut self) {
        let len = self.current_command_menu_items().len();
        if len == 0 {
            return;
        }
        if self.bottom_shell.command_menu_selection == 0 {
            self.bottom_shell.command_menu_selection = len - 1;
        } else {
            self.bottom_shell.command_menu_selection -= 1;
        }
        self.clamp_command_menu_selection();
    }

    pub(super) fn select_next_command_menu_item(&mut self) {
        let len = self.current_command_menu_items().len();
        if len == 0 {
            return;
        }
        self.bottom_shell.command_menu_selection =
            (self.bottom_shell.command_menu_selection + 1) % len;
        self.clamp_command_menu_selection();
    }

    pub(super) fn apply_selected_command_menu_item(&mut self) -> bool {
        let items = self.current_command_menu_items();
        let Some(selected) = items.get(self.bottom_shell.command_menu_selection).cloned() else {
            return false;
        };

        let replacement_start = if self.composer.input.ends_with(char::is_whitespace) {
            self.composer.input.len()
        } else {
            self.composer
                .input
                .rfind(char::is_whitespace)
                .map_or(0, |index| index + 1)
        };
        if self.composer.input[replacement_start..] == selected.insert_text {
            return false;
        }
        self.composer
            .input
            .replace_range(replacement_start.., &selected.insert_text);
        self.composer.cursor = self.composer.input.len();
        self.clear_composer_preferred_column();
        self.clear_input_history_browse();
        self.clamp_command_menu_selection();
        true
    }

    pub(super) fn apply_input_completion(&mut self) {
        if self.apply_selected_command_menu_item() {
            return;
        }

        let Some(completion) = completion_for_input(
            &self.composer.input,
            &self.connection_id,
            self.connection_model.as_deref(),
            &self.connections,
            &self.connection_models,
        ) else {
            return;
        };

        let prefix = self.composer.input[completion.replacement.clone()].to_owned();
        if completion.candidates.len() == 1 {
            self.composer
                .input
                .replace_range(completion.replacement, &completion.candidates[0]);
            self.composer.cursor = self.composer.input.len();
            self.clear_composer_preferred_column();
            self.clear_input_history_browse();
            self.clamp_command_menu_selection();
            return;
        }

        if let Some(shared_prefix) = shared_prefix(&completion.candidates)
            && shared_prefix.len() > prefix.len()
        {
            self.composer
                .input
                .replace_range(completion.replacement, &shared_prefix);
            self.composer.cursor = self.composer.input.len();
            self.clear_composer_preferred_column();
            self.clear_input_history_browse();
            self.clamp_command_menu_selection();
        }
        self.show_notice(completion_candidates_line(&completion.candidates));
    }

    fn handle_escape(&mut self) {
        if self.turn_is_busy() {
            self.show_notice(
                "Turn still active. Use /detach to leave it running or Ctrl-C to cancel.",
            );
        } else {
            self.should_quit = true;
        }
    }

    fn submit_from_composer(&mut self) -> Option<ChatAction> {
        if let Some(choice) = self.selected_approval_choice() {
            Some(choice.to_chat_action())
        } else if self.composer.input.trim_start().starts_with('/') {
            if self.apply_selected_command_menu_item() {
                None
            } else {
                Some(ChatAction::Send)
            }
        } else {
            Some(ChatAction::Send)
        }
    }

    fn navigate_up_in_composer(&mut self) {
        if self.approval_menu_is_active() {
            self.select_previous_approval_choice();
        } else if self.should_show_command_menu() && !self.current_command_menu_items().is_empty() {
            self.select_previous_command_menu_item();
        } else if self.composer.input.is_empty()
            && self.question_panel_is_active()
            && self
                .pending_inputs
                .first()
                .is_some_and(|pending| !pending.choices.is_empty())
        {
            self.select_previous_question_choice();
        } else if self.composer.history_browse_index.is_none()
            && self.move_input_cursor_vertical(-1)
        {
            // Multiline drafts behave like Codex's textarea: Up first moves the
            // caret inside the draft, then falls back to shell-style history at
            // the first line.
        } else {
            self.browse_input_history(-1);
        }
    }

    fn navigate_down_in_composer(&mut self) {
        if self.approval_menu_is_active() {
            self.select_next_approval_choice();
        } else if self.should_show_command_menu() && !self.current_command_menu_items().is_empty() {
            self.select_next_command_menu_item();
        } else if self.composer.input.is_empty()
            && self.question_panel_is_active()
            && self
                .pending_inputs
                .first()
                .is_some_and(|pending| !pending.choices.is_empty())
        {
            self.select_next_question_choice();
        } else if self.composer.history_browse_index.is_none() && self.move_input_cursor_vertical(1)
        {
            // See navigate_up_in_composer: Down stays inside multiline drafts
            // until the caret reaches the final line.
        } else {
            self.browse_input_history(1);
        }
    }

    pub(super) async fn send_input(&mut self) -> Result<(), Box<dyn Error>> {
        if self.pending_session_load.is_some() {
            self.show_notice("Wait for the session switch to finish.");
            return Ok(());
        }

        let text = self
            .composer
            .input
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        if text.trim().is_empty() {
            if let Some(choice) = self.selected_question_choice() {
                if self.pending_send.is_some() {
                    self.show_notice("Wait for the current question answer to finish.");
                    return Ok(());
                }
                self.start_answer_pending_input(serde_json::Value::String(choice));
            }
            return Ok(());
        }

        if text.trim_start().starts_with('/') {
            let command_head = text
                .trim_start()
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or_default();
            if self.turn_is_busy() {
                let allow_during_request = resolve_command(command_head)
                    .map(command_allowed_during_request)
                    .unwrap_or(false);
                if !allow_during_request {
                    self.show_notice("Wait for the current request before running that command.");
                    return Ok(());
                }
            }
            self.composer.input.clear();
            self.composer.cursor = 0;
            self.clear_composer_preferred_column();
            self.clear_input_history_browse();
            return self.handle_command(&text).await;
        }

        self.composer.input.clear();
        self.composer.cursor = 0;
        self.clear_composer_preferred_column();
        self.clear_input_history_browse();
        if !self.pending_inputs.is_empty() {
            if self.pending_send.is_some() {
                self.show_notice("Wait for the current question answer to finish.");
                return Ok(());
            }
            self.start_answer_pending_input(serde_json::Value::String(text));
            return Ok(());
        }
        if is_operator_shell_input(&text) {
            if self.turn_is_busy() || self.pending_shell_command.is_some() {
                self.show_notice(
                    "Operator shell commands do not enter the session queue. Wait for the current request to finish.",
                );
                return Ok(());
            }
            self.start_shell_command(text);
        } else if self.pending_shell_command.is_some() {
            self.show_notice("Wait for the current operator shell command to finish.");
            return Ok(());
        } else if self.pending_send.is_some()
            || self.pending_approval.is_some()
            || self.turn_is_busy()
            || self.active_turn.live
            || self.cancel_requested
        {
            self.start_background_message_submission(text);
        } else {
            self.start_send_text(text);
        }
        Ok(())
    }

    pub(super) fn start_answer_pending_input(&mut self, response: serde_json::Value) {
        if self.pending_send.is_some() {
            self.show_notice("Wait for the current question answer to finish.");
            return;
        }
        let Some(pending) = self.pending_inputs.first().cloned() else {
            self.show_notice("No pending question.");
            return;
        };
        self.mark_live_turn_start();
        self.active_turn.live = true;
        self.set_task_status(
            "Working",
            Some(format!(
                "answering question {}",
                short_id_string(&pending.call_id.to_string())
            )),
        );
        let client = self.client.clone();
        let session_id = self.session_id;
        self.pending_send = Some(tokio::spawn(async move {
            client
                .answer_tool(
                    session_id,
                    &AnswerToolRequest {
                        call_id: pending.call_id,
                        response,
                    },
                )
                .await
                .map(|_| PendingSendCompletion::Ack)
                .map_err(|error| error.to_string())
        }));
        self.pending_send_started_at = Some(Instant::now());
    }

    pub(super) fn start_send_text(&mut self, text: String) {
        self.pending_shell_command = None;
        self.mark_live_turn_start();
        self.active_turn.live = true;
        self.active_turn.optimistic_user_text = Some(text.clone());
        self.set_task_status(
            "Working",
            Some(format!(
                "{} ({})",
                self.connection_id,
                self.effective_model()
            )),
        );
        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        self.pending_send = Some(tokio::spawn(async move {
            client
                .send_message(
                    session_id,
                    &SendMessageRequest {
                        branch_id,
                        message: Message::text(Role::User, text),
                    },
                )
                .await
                .map(PendingSendCompletion::MessageSubmitted)
                .map_err(|error| error.to_string())
        }));
        self.pending_send_started_at = Some(Instant::now());
    }

    pub(super) fn start_background_message_submission(&mut self, text: String) {
        let preview = truncate_detail(&text);
        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        self.pending_message_submissions
            .push_back(PendingMessageSubmission {
                preview,
                handle: tokio::spawn(async move {
                    client
                        .send_message(
                            session_id,
                            &SendMessageRequest {
                                branch_id,
                                message: Message::text(Role::User, text),
                            },
                        )
                        .await
                        .map_err(|error| error.to_string())
                }),
            });
    }

    pub(super) fn start_shell_command(&mut self, raw_input: String) {
        self.mark_live_turn_start();
        let command = raw_input.trim_start_matches('!').trim().to_owned();
        self.set_task_status("Running", Some(format!("!{}", command)));
        self.pending_shell_command = Some(command.clone());
        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        self.pending_send = Some(tokio::spawn(async move {
            client
                .run_shell_command(
                    session_id,
                    &RunShellCommandRequest {
                        branch_id,
                        raw_input,
                        command,
                        timeout_seconds: Some(120),
                    },
                )
                .await
                .map(|_| PendingSendCompletion::Ack)
                .map_err(|error| error.to_string())
        }));
        self.pending_send_started_at = Some(Instant::now());
    }

    fn handle_key_without_escape_prefix(&mut self, key: KeyEvent) -> Option<ChatAction> {
        if let Some(bound) = self.bound_action_for_key(key) {
            return self.apply_bound_key_action(bound);
        }
        match key.code {
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => None,
            KeyCode::Char(ch) => {
                self.insert_input_char(ch);
                None
            }
            _ => None,
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Option<ChatAction> {
        let now = Instant::now();
        self.flush_pending_escape_prefix_if_expired(now);
        if self.should_quit {
            return None;
        }

        if self.pending_escape_prefix_at.take().is_some() {
            if let Some(bound) = self.escape_prefixed_action_for_key(key) {
                return self.apply_bound_key_action(bound);
            }
            self.handle_escape();
            if self.should_quit {
                return None;
            }
            return self.handle_key_without_escape_prefix(key);
        }

        if Self::is_plain_escape_key(key) {
            self.pending_escape_prefix_at = Some(now);
            return None;
        }

        self.handle_key_without_escape_prefix(key)
    }
}
