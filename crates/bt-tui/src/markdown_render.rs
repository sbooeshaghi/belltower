use std::path::Path;

use ratatui::text::{Line, Text};

use crate::text_utils::{display_width, wrap_plain_text};

#[cfg(test)]
pub(crate) fn render_markdown_text(input: &str) -> Text<'static> {
    render_markdown_text_with_width(input, None)
}

#[cfg(test)]
pub(crate) fn render_markdown_text_with_width(input: &str, width: Option<usize>) -> Text<'static> {
    render_markdown_text_with_width_and_cwd(input, width, None)
}

pub(crate) fn render_markdown_text_with_width_and_cwd(
    input: &str,
    width: Option<usize>,
    _cwd: Option<&Path>,
) -> Text<'static> {
    let lines = render_markdown_lines(input, width);
    if lines.is_empty() {
        Text::default()
    } else {
        Text::from(lines)
    }
}

fn render_markdown_lines(input: &str, width: Option<usize>) -> Vec<Line<'static>> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let wrap_width = width.filter(|width| *width > 0);
    let mut rendered = Vec::new();
    let mut in_fenced_code_block = false;

    for raw_line in normalized.lines() {
        if is_fence_line(raw_line) {
            in_fenced_code_block = !in_fenced_code_block;
            continue;
        }

        if in_fenced_code_block {
            rendered.push(Line::from(raw_line.to_owned()));
            continue;
        }

        if raw_line.trim().is_empty() {
            push_blank_line(&mut rendered);
            continue;
        }

        if is_rule_line(raw_line) {
            rendered.push(Line::from("———"));
            continue;
        }

        if let Some(heading) = parse_heading(raw_line) {
            push_wrapped_prefixed_lines(&mut rendered, "", "", heading, wrap_width);
            continue;
        }

        if let Some((prefix, continuation, body)) = parse_block_line(raw_line) {
            push_wrapped_prefixed_lines(&mut rendered, &prefix, &continuation, &body, wrap_width);
            continue;
        }

        push_wrapped_prefixed_lines(&mut rendered, "", "", raw_line.trim(), wrap_width);
    }

    while rendered.last().is_some_and(is_blank_line_spaces_only) {
        rendered.pop();
    }
    rendered
}

fn push_wrapped_prefixed_lines(
    rendered: &mut Vec<Line<'static>>,
    first_prefix: &str,
    continuation_prefix: &str,
    body: &str,
    width: Option<usize>,
) {
    if body.is_empty() {
        rendered.push(Line::from(first_prefix.trim_end().to_owned()));
        return;
    }

    let Some(width) = width else {
        rendered.push(Line::from(format!("{first_prefix}{body}")));
        return;
    };

    let first_width = width.saturating_sub(display_width(first_prefix)).max(1);
    let continuation_width = width
        .saturating_sub(display_width(continuation_prefix))
        .max(1);
    let mut first_visual_line = true;

    for raw_line in body.split('\n') {
        let wrapped = if raw_line.is_empty() {
            vec![String::new()]
        } else if first_visual_line {
            wrap_plain_text(raw_line, first_width)
        } else {
            wrap_plain_text(raw_line, continuation_width)
        };

        for segment in wrapped {
            let prefix = if first_visual_line {
                first_prefix
            } else {
                continuation_prefix
            };
            if segment.is_empty() {
                rendered.push(Line::from(prefix.trim_end().to_owned()));
            } else {
                rendered.push(Line::from(format!("{prefix}{segment}")));
            }
            first_visual_line = false;
        }
    }
}

fn push_blank_line(rendered: &mut Vec<Line<'static>>) {
    if rendered.last().is_some_and(is_blank_line_spaces_only) {
        return;
    }
    rendered.push(Line::default());
}

fn parse_heading(raw_line: &str) -> Option<&str> {
    let trimmed = raw_line.trim_start();
    let depth = trimmed.chars().take_while(|ch| *ch == '#').count();
    if depth == 0 {
        return None;
    }
    trimmed[depth..]
        .strip_prefix(' ')
        .map(str::trim)
        .filter(|body| !body.is_empty())
}

fn parse_block_line(raw_line: &str) -> Option<(String, String, String)> {
    parse_blockquote_line(raw_line)
        .or_else(|| parse_unordered_list_line(raw_line))
        .or_else(|| parse_ordered_list_line(raw_line))
}

fn parse_blockquote_line(raw_line: &str) -> Option<(String, String, String)> {
    let trimmed = raw_line.trim_start();
    let depth = trimmed.chars().take_while(|ch| *ch == '>').count();
    if depth == 0 {
        return None;
    }

    let body = trimmed[depth..].trim_start();
    let quote_prefix = format!("{}> ", "  ".repeat(depth.saturating_sub(1)));
    let continuation = format!("{}  ", "  ".repeat(depth.saturating_sub(1)));
    Some((quote_prefix, continuation, body.to_owned()))
}

fn parse_unordered_list_line(raw_line: &str) -> Option<(String, String, String)> {
    let trimmed = raw_line.trim_start_matches(' ');
    let indent_width = raw_line.len().saturating_sub(trimmed.len());
    let body = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;
    let indent = " ".repeat(indent_width);
    Some((
        format!("{indent}- "),
        format!("{indent}  "),
        body.to_owned(),
    ))
}

fn parse_ordered_list_line(raw_line: &str) -> Option<(String, String, String)> {
    let trimmed = raw_line.trim_start_matches(' ');
    let indent_width = raw_line.len().saturating_sub(trimmed.len());
    let digits_len = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }

    let marker = trimmed.get(..digits_len)?;
    let remainder = trimmed.get(digits_len..)?;
    let body = remainder.strip_prefix(". ")?;
    let indent = " ".repeat(indent_width);
    let prefix = format!("{indent}{marker}. ");
    let continuation = " ".repeat(display_width(&prefix));
    Some((prefix, continuation, body.to_owned()))
}

fn is_fence_line(raw_line: &str) -> bool {
    raw_line.trim_start().starts_with("```")
}

fn is_rule_line(raw_line: &str) -> bool {
    matches!(raw_line.trim(), "---" | "***" | "___")
}

fn is_blank_line_spaces_only(line: &Line<'_>) -> bool {
    if line.spans.is_empty() {
        return true;
    }
    line.spans
        .iter()
        .all(|span| span.content.is_empty() || span.content.chars().all(|ch| ch == ' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_to_strings(text: Text<'static>) -> Vec<String> {
        text.lines.iter().map(Line::to_string).collect()
    }

    #[test]
    fn collapses_repeated_blank_lines_between_paragraphs() {
        let rendered = render_markdown_text("Paragraph 1\n\n\n\nParagraph 2");
        assert_eq!(
            lines_to_strings(rendered),
            vec!["Paragraph 1", "", "Paragraph 2"]
        );
    }

    #[test]
    fn keeps_ordered_list_marker_with_body() {
        let rendered = render_markdown_text("1. Tight item\n");
        assert_eq!(lines_to_strings(rendered), vec!["1. Tight item"]);
    }

    #[test]
    fn preserves_code_block_content_without_fence_markers() {
        let rendered = render_markdown_text("```rust\nfn main() {}\n\nprintln!(\"x\");\n```\n");
        assert_eq!(
            lines_to_strings(rendered),
            vec!["fn main() {}", "", "println!(\"x\");"]
        );
    }

    #[test]
    fn preserves_soft_breaks_as_distinct_lines() {
        let rendered = render_markdown_text("first line\nsecond line");
        assert_eq!(
            lines_to_strings(rendered),
            vec!["first line", "second line"]
        );
    }
}
