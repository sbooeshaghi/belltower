pub const MAX_TOOL_RESULT_LINES: usize = 2_000;
pub const MAX_TOOL_RESULT_BYTES: usize = 51_200;

#[must_use]
pub fn truncate_text(input: &str, max_lines: usize, max_bytes: usize) -> String {
    let bytes = input.as_bytes();
    if bytes.len() <= max_bytes && input.lines().count() <= max_lines {
        return input.to_owned();
    }

    let lines = input.lines().collect::<Vec<_>>();
    let keep_each_side = (max_lines / 2).max(1);
    let head = lines
        .iter()
        .take(keep_each_side)
        .copied()
        .collect::<Vec<_>>();
    let tail = lines
        .iter()
        .rev()
        .take(keep_each_side)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    let omitted_lines = lines.len().saturating_sub(head.len() + tail.len());
    let mut output = String::new();
    output.push_str(&head.join("\n"));
    output.push_str("\n---\n");
    output.push_str(&format!(
        "[... {} lines truncated, {} bytes total ...]",
        omitted_lines,
        bytes.len()
    ));
    output.push_str("\n---\n");
    output.push_str(&tail.join("\n"));

    if output.len() > max_bytes {
        output.truncate(max_bytes);
    }

    output
}
