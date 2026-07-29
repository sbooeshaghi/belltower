use super::*;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WrappedTextRow {
    pub(crate) start_byte: usize,
    pub(crate) text: String,
    pub(crate) end_byte: usize,
}

pub(crate) fn truncate_path(path: &str, limit: usize) -> String {
    if path.chars().count() <= limit {
        return path.to_owned();
    }

    let tail = path
        .chars()
        .rev()
        .take(limit.saturating_sub(3))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

pub(crate) fn display_project_root(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_owned();
    };
    let Some(home) = home.to_str() else {
        return path.to_owned();
    };

    if path == home {
        "~".to_owned()
    } else if let Some(rest) = path.strip_prefix(home) {
        if rest.starts_with('/') {
            format!("~{rest}")
        } else {
            path.to_owned()
        }
    } else {
        path.to_owned()
    }
}

pub(crate) fn previous_char_boundary(input: &str, index: usize) -> usize {
    if index == 0 {
        return 0;
    }
    let mut boundary = index.saturating_sub(1);
    while boundary > 0 && !input.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

pub(crate) fn next_char_boundary(input: &str, index: usize) -> usize {
    if index >= input.len() {
        return input.len();
    }
    let mut boundary = (index + 1).min(input.len());
    while boundary < input.len() && !input.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

pub(crate) fn previous_word_boundary(input: &str, index: usize) -> usize {
    let mut boundary = index;
    while boundary > 0 {
        let previous = previous_char_boundary(input, boundary);
        let ch = input[previous..boundary].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        boundary = previous;
    }
    while boundary > 0 {
        let previous = previous_char_boundary(input, boundary);
        let ch = input[previous..boundary].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        boundary = previous;
    }
    boundary
}

pub(crate) fn next_word_boundary(input: &str, index: usize) -> usize {
    let mut boundary = index;
    while boundary < input.len() {
        let next = next_char_boundary(input, boundary);
        let ch = input[boundary..next].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        boundary = next;
    }
    while boundary < input.len() {
        let next = next_char_boundary(input, boundary);
        let ch = input[boundary..next].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        boundary = next;
    }
    boundary
}

fn char_display_width(ch: char, column: usize) -> usize {
    match ch {
        '\t' => 4usize.saturating_sub(column % 4).max(1),
        _ => UnicodeWidthChar::width(ch).unwrap_or(0),
    }
}

pub(crate) fn display_width(raw: &str) -> usize {
    let mut max_width = 0usize;
    let mut line_width = 0usize;

    for ch in raw.chars() {
        if ch == '\n' {
            max_width = max_width.max(line_width);
            line_width = 0;
            continue;
        }
        line_width += char_display_width(ch, line_width);
    }

    max_width.max(line_width)
}

pub(crate) fn take_prefix_by_display_width(raw: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }

    let mut prefix = String::new();
    let mut width = 0usize;

    for ch in raw.chars() {
        if ch == '\n' {
            break;
        }

        let ch_width = char_display_width(ch, width);
        if ch_width > 0 && width + ch_width > limit {
            break;
        }
        prefix.push(ch);
        width += ch_width;
    }

    prefix
}

pub(crate) fn pad_to_display_width(raw: &str, width: usize) -> String {
    let raw_width = display_width(raw);
    if raw_width >= width {
        return raw.to_owned();
    }

    let mut padded = raw.to_owned();
    padded.push_str(&" ".repeat(width - raw_width));
    padded
}

pub(crate) fn wrap_plain_text(raw: &str, width: usize) -> Vec<String> {
    wrap_plain_text_with_end_indices(raw, width)
        .into_iter()
        .map(|row| row.text)
        .collect()
}

pub(crate) fn wrap_composer_input(raw: &str, width: usize) -> Vec<String> {
    wrap_composer_input_with_end_indices(raw, width)
        .into_iter()
        .map(|row| row.text)
        .collect()
}

pub(crate) fn wrap_plain_text_with_end_indices(raw: &str, width: usize) -> Vec<WrappedTextRow> {
    wrap_plain_text_with_end_indices_impl(raw, width, true)
}

pub(crate) fn wrap_composer_input_with_end_indices(raw: &str, width: usize) -> Vec<WrappedTextRow> {
    wrap_plain_text_with_end_indices_impl(raw, width, false)
}

fn wrap_plain_text_with_end_indices_impl(
    raw: &str,
    width: usize,
    trim_trailing: bool,
) -> Vec<WrappedTextRow> {
    let width = width.max(1);
    let mut wrapped = Vec::new();

    if raw.is_empty() {
        wrapped.push(WrappedTextRow {
            start_byte: 0,
            text: String::new(),
            end_byte: 0,
        });
        return wrapped;
    }

    let mut offset = 0usize;
    for segment in raw.split_inclusive('\n') {
        let has_newline = segment.ends_with('\n');
        let content_len = segment.len() - usize::from(has_newline);
        let line_start = offset;
        let line_end = offset + content_len;
        wrap_logical_line(
            raw,
            line_start,
            line_end,
            has_newline,
            width,
            trim_trailing,
            &mut wrapped,
        );
        offset += segment.len();
    }

    if raw.ends_with('\n') {
        wrapped.push(WrappedTextRow {
            start_byte: raw.len(),
            text: String::new(),
            end_byte: raw.len(),
        });
    }

    wrapped
}

#[cfg(test)]
pub(crate) fn wrapped_cursor_position(input: &str, cursor: usize, width: usize) -> (u16, u16) {
    wrapped_cursor_position_with_rows(input, cursor, width, wrap_plain_text_with_end_indices)
}

pub(crate) fn wrapped_composer_cursor_position(
    input: &str,
    cursor: usize,
    width: usize,
) -> (u16, u16) {
    wrapped_cursor_position_with_rows(input, cursor, width, wrap_composer_input_with_end_indices)
}

fn wrapped_cursor_position_with_rows(
    input: &str,
    cursor: usize,
    width: usize,
    wrap_rows: impl Fn(&str, usize) -> Vec<WrappedTextRow>,
) -> (u16, u16) {
    let width = width.max(1);
    let before_cursor = &input[..cursor];
    let rows = wrap_rows(before_cursor, width);
    let row = rows.len().saturating_sub(1);
    let col = rows.last().map(|row| display_width(&row.text)).unwrap_or(0);
    (
        col.min(usize::from(u16::MAX)) as u16,
        row.min(usize::from(u16::MAX)) as u16,
    )
}

fn wrap_logical_line(
    raw: &str,
    line_start: usize,
    line_end: usize,
    has_newline: bool,
    width: usize,
    trim_trailing: bool,
    wrapped: &mut Vec<WrappedTextRow>,
) {
    if line_start == line_end {
        wrapped.push(WrappedTextRow {
            start_byte: line_start,
            text: String::new(),
            end_byte: if has_newline { line_end + 1 } else { line_end },
        });
        return;
    }

    let mut row_start = line_start;
    while row_start < line_end {
        let row_start_before_trim = row_start;
        if row_start_before_trim > line_start {
            while row_start < line_end {
                let Some(ch) = raw[row_start..line_end].chars().next() else {
                    break;
                };
                if !ch.is_whitespace() {
                    break;
                }
                row_start += ch.len_utf8();
            }
        }
        if row_start >= line_end {
            break;
        }

        let mut cursor = row_start;
        let mut row_width = 0usize;
        let mut last_space_break: Option<(usize, usize)> = None;
        let mut emitted = false;

        while cursor < line_end {
            let ch = raw[cursor..line_end].chars().next().unwrap_or_default();
            let next = cursor + ch.len_utf8();
            let ch_width = char_display_width(ch, row_width);
            if ch_width > 0 && row_width > 0 && row_width + ch_width > width {
                let break_at = if ch.is_whitespace() {
                    Some((next, cursor))
                } else {
                    last_space_break
                };
                if let Some((next_start, text_end)) = break_at
                    && text_end > row_start
                {
                    wrapped.push(WrappedTextRow {
                        start_byte: row_start,
                        text: raw[row_start..text_end].to_owned(),
                        end_byte: next_start,
                    });
                    row_start = next_start;
                } else {
                    wrapped.push(WrappedTextRow {
                        start_byte: row_start,
                        text: raw[row_start..cursor].to_owned(),
                        end_byte: cursor,
                    });
                    row_start = cursor;
                }
                emitted = true;
                break;
            }

            row_width += ch_width;
            if ch.is_whitespace() {
                last_space_break = Some((next, cursor));
            }
            cursor = next;
        }

        if !emitted {
            let end_byte = if has_newline { line_end + 1 } else { line_end };
            let text = if trim_trailing {
                raw[row_start..line_end].trim_end().to_owned()
            } else {
                raw[row_start..line_end].to_owned()
            };
            wrapped.push(WrappedTextRow {
                start_byte: row_start,
                text,
                end_byte,
            });
            break;
        }
    }
}

pub(crate) fn next_stream_retry_delay(attempt: u8) -> Duration {
    match attempt {
        0 => Duration::from_secs(1),
        1 => Duration::from_secs(2),
        2 => Duration::from_secs(4),
        _ => EVENT_STREAM_RETRY_MAX_DELAY,
    }
}

pub(crate) fn short_id_string(raw: &str) -> String {
    raw.chars().take(8).collect()
}

pub(crate) fn parse_tool_mode(raw: &str) -> Result<SessionToolMode, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "standard" => Ok(SessionToolMode::Standard),
        "extended" => Ok(SessionToolMode::Extended),
        _ => Err("unknown tool mode".to_owned()),
    }
}

pub(crate) fn render_tool_mode(mode: SessionToolMode) -> &'static str {
    match mode {
        SessionToolMode::Standard => "standard",
        SessionToolMode::Extended => "extended",
    }
}

pub(crate) fn truncate_detail(raw: &str) -> String {
    let limit = 28usize;
    let mut chars = raw.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_plain_text_prefers_word_boundaries() {
        assert_eq!(
            wrap_plain_text("one two three", 7),
            vec!["one two".to_owned(), "three".to_owned()]
        );
        assert_eq!(
            wrap_plain_text("hello world", 5),
            vec!["hello".to_owned(), "world".to_owned()]
        );
    }

    #[test]
    fn wrap_plain_text_breaks_long_unspaced_tokens() {
        assert_eq!(
            wrap_plain_text("abcdefgh", 3),
            vec!["abc".to_owned(), "def".to_owned(), "gh".to_owned()]
        );
    }

    #[test]
    fn wrap_plain_text_preserves_explicit_blank_lines() {
        assert_eq!(
            wrap_plain_text("\n\nhello", 80),
            vec!["".to_owned(), "".to_owned(), "hello".to_owned()]
        );
        assert_eq!(
            wrap_plain_text("hello\n\n", 80),
            vec!["hello".to_owned(), "".to_owned(), "".to_owned()]
        );
    }

    #[test]
    fn wrapped_cursor_position_tracks_explicit_blank_lines() {
        assert_eq!(wrapped_cursor_position("\n\nhello", 2, 80), (0, 2));
        assert_eq!(wrapped_cursor_position("\n\nhello", 7, 80), (5, 2));
    }

    #[test]
    fn composer_wrapping_preserves_trailing_spaces() {
        assert_eq!(wrap_plain_text("hello ", 80), vec!["hello".to_owned()]);
        assert_eq!(wrap_composer_input("hello ", 80), vec!["hello ".to_owned()]);
        assert_eq!(wrapped_composer_cursor_position("hello ", 6, 80), (6, 0));
        assert_eq!(wrapped_composer_cursor_position("hello  ", 7, 80), (7, 0));
    }
}
