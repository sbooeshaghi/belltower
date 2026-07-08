use std::path::Path;

use ratatui::text::Line;

pub(crate) fn append_markdown(
    markdown_source: &str,
    width: Option<usize>,
    cwd: Option<&Path>,
    lines: &mut Vec<Line<'static>>,
) {
    let rendered = crate::markdown_render::render_markdown_text_with_width_and_cwd(
        markdown_source,
        width,
        cwd,
    );
    lines.extend(rendered.lines);
}

pub(crate) fn render_markdown_to_string(
    markdown_source: &str,
    width: Option<usize>,
    cwd: Option<&Path>,
) -> String {
    let rendered = crate::markdown_render::render_markdown_text_with_width_and_cwd(
        markdown_source,
        width,
        cwd,
    );
    rendered
        .lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_to_strings(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(Line::to_string).collect()
    }

    #[test]
    fn append_markdown_collapses_repeated_blank_lines_between_paragraphs() {
        let mut out = Vec::new();
        append_markdown("first\n\n\n\nsecond\n", Some(80), None, &mut out);
        assert_eq!(lines_to_strings(&out), vec!["first", "", "second"]);
    }

    #[test]
    fn append_markdown_keeps_ordered_list_line_unsplit() {
        let mut out = Vec::new();
        append_markdown("1. Tight item\n", Some(80), None, &mut out);
        assert_eq!(lines_to_strings(&out), vec!["1. Tight item"]);
    }
}
