use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use std::ops::Range;
use unicode_width::UnicodeWidthChar;
use url::Url;

pub(crate) fn adaptive_wrap_line(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    if text.is_empty() {
        return vec![styled_line(String::new(), line.style)];
    }

    if line_contains_url_like(line) && !line_has_mixed_url_and_non_url_tokens(line) {
        return vec![line_range_with_style(line, 0..text.len())];
    }

    wrap_text_ranges_preserving_urls(&text, width)
        .into_iter()
        .map(|range| line_range_with_style(line, range))
        .collect()
}

pub(crate) fn line_contains_url_like(line: &Line<'_>) -> bool {
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    text_contains_url_like(&text)
}

pub(crate) fn line_has_mixed_url_and_non_url_tokens(line: &Line<'_>) -> bool {
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    text_has_mixed_url_and_non_url_tokens(&text)
}

fn wrap_text_ranges_preserving_urls(text: &str, width: usize) -> Vec<Range<usize>> {
    let protected_ranges = protected_url_ranges(text);
    let mut wrapped = Vec::new();
    let mut row_start = 0usize;
    let mut row_width = 0usize;

    for (byte_index, ch) in text.char_indices() {
        if ch == '\n' {
            wrapped.push(row_start..byte_index);
            row_start = byte_index + ch.len_utf8();
            row_width = 0;
            continue;
        }

        let ch_width = char_display_width(ch);
        let in_protected_range = protected_ranges
            .iter()
            .any(|range| range.start <= byte_index && byte_index < range.end);

        if row_width > 0 && row_width + ch_width > width && !in_protected_range {
            wrapped.push(row_start..byte_index);
            row_start = byte_index;
            row_width = 0;
        }

        row_width += ch_width;
    }

    if row_start < text.len() {
        wrapped.push(row_start..text.len());
    } else if text.is_empty() || text.ends_with('\n') {
        wrapped.push(text.len()..text.len());
    }

    wrapped
}

fn protected_url_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut token_start = None;

    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                push_url_like_range(text, start, idx, &mut ranges);
            }
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }

    if let Some(start) = token_start {
        push_url_like_range(text, start, text.len(), &mut ranges);
    }

    ranges
}

fn push_url_like_range(text: &str, start: usize, end: usize, ranges: &mut Vec<Range<usize>>) {
    let token = &text[start..end];
    let Some((inner_start, inner_end)) = stripped_token_range(token) else {
        return;
    };
    let candidate = &token[inner_start..inner_end];
    if is_url_like_token(candidate) {
        ranges.push((start + inner_start)..(start + inner_end));
    }
}

fn stripped_token_range(token: &str) -> Option<(usize, usize)> {
    let mut start = 0usize;
    let mut end = token.len();

    while start < end {
        let ch = token[start..].chars().next()?;
        if !is_surrounding_punctuation(ch) {
            break;
        }
        start += ch.len_utf8();
    }

    while end > start {
        let ch = token[..end].chars().next_back()?;
        if !is_surrounding_punctuation(ch) {
            break;
        }
        end -= ch.len_utf8();
    }

    (start < end).then_some((start, end))
}

fn is_surrounding_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | '.' | ';' | ':' | '!' | '\'' | '"'
    )
}

fn text_contains_url_like(text: &str) -> bool {
    text.split_ascii_whitespace().any(is_url_like_token)
}

fn text_has_mixed_url_and_non_url_tokens(text: &str) -> bool {
    let mut saw_url = false;
    let mut saw_non_url = false;

    for token in text.split_ascii_whitespace() {
        if is_url_like_token(token) {
            saw_url = true;
        } else if is_substantive_non_url_token(token) {
            saw_non_url = true;
        }

        if saw_url && saw_non_url {
            return true;
        }
    }

    false
}

fn is_substantive_non_url_token(token: &str) -> bool {
    let token = token.trim_matches(is_surrounding_punctuation);
    !token.is_empty() && token.chars().any(|ch| ch.is_alphanumeric()) && !is_url_like_token(token)
}

fn is_url_like_token(token: &str) -> bool {
    let token = token.trim_matches(is_surrounding_punctuation);
    if token.is_empty() {
        return false;
    }

    if token.contains("://") {
        return Url::parse(token).is_ok();
    }

    if let Some(stripped) = token.strip_prefix("www.") {
        return looks_like_domain_host(stripped);
    }

    if token.starts_with("localhost") {
        return true;
    }

    looks_like_ipv4_host(token) || looks_like_domain_host(token)
}

fn looks_like_domain_host(token: &str) -> bool {
    let host = token.split_once('/').map_or(token, |(host, _)| host);
    let host = host.split_once('?').map_or(host, |(host, _)| host);
    let host = host.split_once('#').map_or(host, |(host, _)| host);
    let host = host.split_once(':').map_or(host, |(host, _)| host);

    host.contains('.')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
        && host.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn looks_like_ipv4_host(token: &str) -> bool {
    let host = token.split_once('/').map_or(token, |(host, _)| host);
    let host = host.split_once('?').map_or(host, |(host, _)| host);
    let host = host.split_once('#').map_or(host, |(host, _)| host);
    let host = host.split_once(':').map_or(host, |(host, _)| host);

    let mut count = 0usize;
    for octet in host.split('.') {
        count += 1;
        if count > 4
            || octet.is_empty()
            || octet.len() > 3
            || !octet.chars().all(|ch| ch.is_ascii_digit())
        {
            return false;
        }
        if octet.parse::<u8>().is_err() {
            return false;
        }
    }
    count == 4
}

fn char_display_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

fn styled_line(content: String, style: Style) -> Line<'static> {
    Line::from(vec![Span::styled(content, style)])
}

fn line_range_with_style(line: &Line<'_>, range: Range<usize>) -> Line<'static> {
    let mut spans = Vec::new();
    let mut span_start = 0usize;

    for span in &line.spans {
        let content = span.content.as_ref();
        let span_end = span_start + content.len();
        let start = range.start.max(span_start);
        let end = range.end.min(span_end);
        if start < end {
            spans.push(Span::styled(
                content[start - span_start..end - span_start].to_owned(),
                span.style,
            ));
        }
        span_start = span_end;
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    let mut wrapped = Line::from(spans);
    wrapped.style = line.style;
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn keeps_url_only_lines_intact() {
        let line = Line::from("https://example.com/very/long/path");

        let wrapped = adaptive_wrap_line(&line, 8);

        assert_eq!(
            rendered_text(&wrapped),
            vec!["https://example.com/very/long/path"]
        );
    }

    #[test]
    fn preserves_url_tokens_inside_mixed_lines() {
        let line = Line::from("prefix https://example.com/very/long/path suffix");

        let wrapped = adaptive_wrap_line(&line, 12);
        let texts = rendered_text(&wrapped);

        assert!(texts.len() > 1);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("https://example.com/very/long/path"))
        );
    }

    #[test]
    fn wraps_long_non_url_tokens() {
        let line = Line::from("prefix superlongunbrokenspan suffix");

        let wrapped = adaptive_wrap_line(&line, 10);
        let texts = rendered_text(&wrapped);

        assert!(texts.len() > 2);
        assert_eq!(texts.join(""), "prefix superlongunbrokenspan suffix");
    }

    #[test]
    fn preserves_span_styles_while_wrapping() {
        let user_style = Style::default().bg(ratatui::style::Color::Rgb(58, 58, 58));
        let accent_style = Style::default().fg(ratatui::style::Color::Cyan);
        let line = Line::from(vec![
            Span::styled("› hello ", user_style),
            Span::styled("world", accent_style),
        ]);

        let wrapped = adaptive_wrap_line(&line, 8);
        let texts = rendered_text(&wrapped);

        assert_eq!(texts, vec!["› hello ", "world"]);
        assert_eq!(wrapped[0].spans[0].style.bg, user_style.bg);
        assert_eq!(wrapped[1].spans[0].style.fg, accent_style.fg);
    }
}
