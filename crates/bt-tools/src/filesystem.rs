use crate::{
    mutation::with_file_mutation_queue,
    text::{MAX_TOOL_RESULT_BYTES, MAX_TOOL_RESULT_LINES, truncate_text},
};
use bt_core::{
    ApprovalRequirement, BelltowerError, Result, ToolContext, ToolDisplayGroup, ToolExecutionMode,
    ToolExecutor, ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
};
use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{Value, json};
use std::fs;
use std::future::Future;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path};
use std::pin::Pin;

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

pub struct ReadTool;
pub struct WriteTool;
pub struct EditTool;
pub struct ListTool;
pub struct SearchTool;

impl ToolExecutor for ReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".to_owned(),
            description: "Read file contents with optional line ranges.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["path", "call_id"],
                "properties": {
                    "path": {"type": "string"},
                    "call_id": {"type": "string"},
                    "start_line": {"type": "integer"},
                    "end_line": {"type": "integer"}
                }
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec!["file".to_owned(), "read".to_owned()],
                display_group: ToolDisplayGroup::Codebase,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let path = required_string(&arguments, "path")?;
            let call_id = required_string(&arguments, "call_id")?;
            let start_line = arguments.get("start_line").and_then(Value::as_u64);
            let end_line = arguments.get("end_line").and_then(Value::as_u64);
            let resolved = resolve_within_root(&context.project_root, &path)?;
            let bytes = fs::read(&resolved)?;
            let output = if is_binary(&bytes) {
                json!({
                    "path": resolved.to_string(),
                    "binary": true,
                    "size_bytes": bytes.len(),
                })
            } else {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let sliced = slice_lines(&text, start_line, end_line);
                json!({
                    "path": resolved.to_string(),
                    "binary": false,
                    "content": truncate_text(&sliced, MAX_TOOL_RESULT_LINES, MAX_TOOL_RESULT_BYTES),
                })
            };
            Ok(tool_result(call_id, "read", false, output))
        })
    }
}

impl ToolExecutor for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".to_owned(),
            description: "Write a file relative to the project root.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["path", "content", "call_id"],
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "call_id": {"type": "string"},
                    "create_parent": {"type": "boolean"}
                }
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Moderate,
                is_read_only: false,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::WaitForCompletion,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec!["file".to_owned(), "write".to_owned()],
                display_group: ToolDisplayGroup::Codebase,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::FirstUsePerSession
    }

    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let path = required_string(&arguments, "path")?;
            let content = required_string(&arguments, "content")?;
            let call_id = required_string(&arguments, "call_id")?;
            let create_parent = arguments
                .get("create_parent")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let resolved = resolve_within_root(&context.project_root, &path)?;
            let output = with_file_mutation_queue(&resolved, async {
                if create_parent && let Some(parent) = resolved.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&resolved, content)?;
                Ok::<_, BelltowerError>(json!({"path": resolved.to_string(), "written": true}))
            })
            .await?;
            Ok(tool_result(call_id, "write", false, output))
        })
    }
}

impl ToolExecutor for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_owned(),
            description: "Apply an exact string replacement in a file.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["path", "search", "replace", "call_id"],
                "properties": {
                    "path": {"type": "string"},
                    "search": {"type": "string"},
                    "replace": {"type": "string"},
                    "call_id": {"type": "string"}
                }
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Moderate,
                is_read_only: false,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::WaitForCompletion,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec!["file".to_owned(), "edit".to_owned(), "replace".to_owned()],
                display_group: ToolDisplayGroup::Codebase,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::FirstUsePerSession
    }

    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let path = required_string(&arguments, "path")?;
            let search = required_string(&arguments, "search")?;
            let replace = required_string(&arguments, "replace")?;
            let call_id = required_string(&arguments, "call_id")?;
            let resolved = resolve_within_root(&context.project_root, &path)?;
            let output = with_file_mutation_queue(&resolved, async {
                let content = fs::read_to_string(&resolved)?;
                let matches = content.matches(&search).count();
                if matches != 1 {
                    return Err(BelltowerError::InvalidState(format!(
                        "edit requires exactly one match, found {matches}"
                    )));
                }
                let updated = content.replacen(&search, &replace, 1);
                fs::write(&resolved, updated)?;
                Ok::<_, BelltowerError>(json!({"path": resolved.to_string(), "edited": true}))
            })
            .await?;
            Ok(tool_result(call_id, "edit", false, output))
        })
    }
}

impl ToolExecutor for ListTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list".to_owned(),
            description: "List files and directories under the project root.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["call_id"],
                "properties": {
                    "path": {"type": "string"},
                    "call_id": {"type": "string"},
                    "max_entries": {"type": "integer"}
                }
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec!["files".to_owned(), "directory".to_owned()],
                display_group: ToolDisplayGroup::Codebase,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".")
                .to_owned();
            let call_id = required_string(&arguments, "call_id")?;
            let max_entries = arguments
                .get("max_entries")
                .and_then(Value::as_u64)
                .unwrap_or(200) as usize;
            let resolved = resolve_within_root(&context.project_root, &path)?;
            let mut builder = WalkBuilder::new(&resolved);
            builder.hidden(false).git_ignore(true).max_depth(Some(3));
            let entries = builder
                .build()
                .filter_map(std::result::Result::ok)
                .take(max_entries)
                .map(|entry| {
                    json!({
                        "path": entry.path().display().to_string(),
                        "is_dir": entry.file_type().is_some_and(|ft| ft.is_dir()),
                    })
                })
                .collect::<Vec<_>>();
            Ok(tool_result(
                call_id,
                "list",
                false,
                json!({"entries": entries}),
            ))
        })
    }
}

impl ToolExecutor for SearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search".to_owned(),
            description: "Search file contents or paths with a regex.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["query", "call_id"],
                "properties": {
                    "query": {"type": "string"},
                    "call_id": {"type": "string"},
                    "mode": {"type": "string", "enum": ["content", "path"]},
                    "path": {"type": "string"},
                    "max_matches": {"type": "integer"}
                }
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec![
                    "search".to_owned(),
                    "regex".to_owned(),
                    "ripgrep".to_owned(),
                ],
                display_group: ToolDisplayGroup::Codebase,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let query = required_string(&arguments, "query")?;
            let call_id = required_string(&arguments, "call_id")?;
            let base = arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".")
                .to_owned();
            let max_matches = arguments
                .get("max_matches")
                .and_then(Value::as_u64)
                .unwrap_or(100) as usize;
            let mode = arguments
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("content")
                .to_owned();
            let resolved = resolve_within_root(&context.project_root, &base)?;
            let regex = Regex::new(&query)
                .map_err(|error| BelltowerError::InvalidState(error.to_string()))?;
            let project_root = context.project_root;
            let cancellation = context.cancellation;
            let search_mode = mode.clone();
            let matches = run_blocking_search(move || {
                search_matches_in_tree(
                    &project_root,
                    &resolved,
                    &regex,
                    &search_mode,
                    max_matches,
                    cancellation.as_ref(),
                )
            })
            .await?;
            Ok(tool_result(
                call_id,
                "search",
                false,
                json!({"mode": mode, "matches": matches}),
            ))
        })
    }
}

async fn run_blocking_search<F>(search: F) -> Result<Vec<Value>>
where
    F: FnOnce() -> Result<Vec<Value>> + Send + 'static,
{
    tokio::task::spawn_blocking(search).await.map_err(|error| {
        BelltowerError::Tool(format!("filesystem search worker failed: {error}"))
    })?
}

fn resolve_within_root(project_root: &Utf8Path, input: &str) -> Result<Utf8PathBuf> {
    let canonical_root = project_root.canonicalize_utf8()?;
    let resolved = resolve_relative_path(&canonical_root, input)?;
    let existing = nearest_existing_ancestor(&resolved)?;
    let canonical_existing = existing.canonicalize_utf8()?;
    if !canonical_existing.starts_with(&canonical_root) {
        return Err(BelltowerError::InvalidState(format!(
            "path `{input}` escapes the project root"
        )));
    }

    Ok(resolved)
}

fn resolve_relative_path(project_root: &Utf8Path, input: &str) -> Result<Utf8PathBuf> {
    let input_path = Path::new(input);
    if input_path.is_absolute() {
        return Err(BelltowerError::InvalidState(format!(
            "path `{input}` escapes the project root"
        )));
    }

    let root_depth = project_root.components().count();
    let mut resolved = project_root.to_owned();
    for component in input_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => resolved.push(
                part.to_str()
                    .expect("file tool arguments are expected to be valid UTF-8"),
            ),
            Component::ParentDir => {
                if resolved.components().count() <= root_depth {
                    return Err(BelltowerError::InvalidState(format!(
                        "path `{input}` escapes the project root"
                    )));
                }
                resolved.pop();
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(BelltowerError::InvalidState(format!(
                    "path `{input}` escapes the project root"
                )));
            }
        }
    }

    Ok(resolved)
}

fn nearest_existing_ancestor(path: &Utf8Path) -> Result<Utf8PathBuf> {
    let mut current = path.to_owned();
    loop {
        if current.exists() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(BelltowerError::InvalidState(format!(
                "path `{}` is not reachable from the project root",
                path
            )));
        }
    }
}

fn required_string(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| BelltowerError::InvalidState(format!("missing string argument `{key}`")))
}

fn tool_result(
    call_id: String,
    tool_name: &str,
    is_error: bool,
    output: Value,
) -> ToolResultEnvelope {
    ToolResultEnvelope {
        call_id: bt_core::ToolCallId::new(call_id),
        tool_name: tool_name.to_owned(),
        is_error,
        output,
        duration_ms: None,
    }
}

fn search_matches_in_tree(
    project_root: &Utf8Path,
    resolved: &Utf8Path,
    regex: &Regex,
    mode: &str,
    max_matches: usize,
    cancellation: Option<&bt_core::CancellationSignal>,
) -> Result<Vec<Value>> {
    ensure_search_not_cancelled(cancellation)?;
    if max_matches == 0 {
        return Ok(Vec::new());
    }

    let mut builder = WalkBuilder::new(resolved);
    builder.hidden(false).git_ignore(true);
    if resolved.is_file() {
        builder.max_depth(Some(1));
    }
    let mut matches = Vec::new();
    'outer: for entry in builder.build().filter_map(std::result::Result::ok) {
        ensure_search_not_cancelled(cancellation)?;
        let path = Utf8PathBuf::from_path_buf(entry.path().to_path_buf())
            .unwrap_or_else(|_| project_root.join("non-utf8"));
        match mode {
            "content" => {
                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    continue;
                }
                if is_binary_file(&path)? {
                    continue;
                }
                let file = match fs::File::open(&path) {
                    Ok(file) => file,
                    Err(_) => continue,
                };
                let reader = BufReader::new(file);
                for (index, line) in reader.lines().enumerate() {
                    ensure_search_not_cancelled(cancellation)?;
                    let Ok(line) = line else {
                        break;
                    };
                    if regex.is_match(&line) {
                        matches.push(json!({
                            "path": path.to_string(),
                            "line": index + 1,
                            "content": truncate_text(&line, 1, 500),
                        }));
                        if matches.len() >= max_matches {
                            break 'outer;
                        }
                    }
                }
            }
            "path" => {
                if regex.is_match(path.as_str()) {
                    matches.push(json!({
                        "path": path.to_string(),
                        "is_dir": entry.file_type().is_some_and(|ft| ft.is_dir()),
                    }));
                    if matches.len() >= max_matches {
                        break 'outer;
                    }
                }
            }
            other => {
                return Err(BelltowerError::InvalidState(format!(
                    "unsupported search mode `{other}`"
                )));
            }
        }
    }
    Ok(matches)
}

fn ensure_search_not_cancelled(cancellation: Option<&bt_core::CancellationSignal>) -> Result<()> {
    if cancellation.is_some_and(bt_core::CancellationSignal::is_cancelled) {
        return Err(BelltowerError::Tool(
            "filesystem search cancelled".to_owned(),
        ));
    }
    Ok(())
}

fn is_binary_file(path: &Utf8Path) -> Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut sample = [0u8; 1024];
    let read = file.read(&mut sample)?;
    Ok(is_binary(&sample[..read]))
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(1024).any(|byte| *byte == 0)
}

fn slice_lines(input: &str, start_line: Option<u64>, end_line: Option<u64>) -> String {
    let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
    let end = end_line.map_or(usize::MAX, |value| value as usize);
    input
        .lines()
        .enumerate()
        .filter(|(index, _)| *index >= start && (*index + 1) <= end)
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{EditTool, ReadTool, SearchTool, WriteTool, run_blocking_search};
    use bt_core::{CancellationSignal, ToolContext, traits::ToolExecutor};
    use serde_json::json;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    #[tokio::test]
    async fn read_write_and_edit_tools_work() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let context = ToolContext::new(root);

        let write = WriteTool;
        write
            .execute(
                json!({"path": "hello.txt", "content": "hello world", "call_id": "call-1"}),
                context.clone(),
            )
            .await
            .expect("write");

        let edit = EditTool;
        edit.execute(
            json!({"path": "hello.txt", "search": "world", "replace": "belltower", "call_id": "call-2"}),
            context.clone(),
        )
        .await
        .expect("edit");

        let read = ReadTool;
        let result = read
            .execute(json!({"path": "hello.txt", "call_id": "call-3"}), context)
            .await
            .expect("read");
        let content = result
            .output
            .get("content")
            .and_then(|value| value.as_str())
            .expect("content");
        assert!(content.contains("belltower"));
    }

    #[tokio::test]
    async fn write_to_new_nested_path_stays_within_root() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let context = ToolContext::new(root);

        let write = WriteTool;
        write
            .execute(
                json!({
                    "path": "nested/dir/note.txt",
                    "content": "safe",
                    "call_id": "call-1"
                }),
                context,
            )
            .await
            .expect("write");

        let content =
            std::fs::read_to_string(dir.path().join("nested/dir/note.txt")).expect("nested file");
        assert_eq!(content, "safe");
    }

    #[tokio::test]
    async fn write_rejects_parent_escape_for_missing_path() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let context = ToolContext::new(root);

        let write = WriteTool;
        let error = write
            .execute(
                json!({
                    "path": "../escape.txt",
                    "content": "unsafe",
                    "call_id": "call-1"
                }),
                context,
            )
            .await
            .expect_err("escape should fail");

        assert!(error.to_string().contains("escapes the project root"));
    }

    #[tokio::test]
    async fn search_tool_finds_matches() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        std::fs::write(root.join("a.txt"), "alpha\nbeta\n").expect("write file");
        let context = ToolContext::new(root);
        let search = SearchTool;
        let result = search
            .execute(json!({"query": "alp.*", "call_id": "call-1"}), context)
            .await
            .expect("search");
        let matches = result
            .output
            .get("matches")
            .and_then(|value| value.as_array())
            .expect("matches");
        assert_eq!(matches.len(), 1);
    }

    #[tokio::test]
    async fn search_tool_supports_path_mode_natively() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        std::fs::create_dir_all(root.join("src")).expect("src dir");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write file");
        let context = ToolContext::new(root);
        let search = SearchTool;
        let result = search
            .execute(
                json!({"query": "main\\.rs$", "mode": "path", "call_id": "call-2"}),
                context,
            )
            .await
            .expect("search");
        let matches = result
            .output
            .get("matches")
            .and_then(|value| value.as_array())
            .expect("matches");
        assert_eq!(matches.len(), 1);
        let path = matches[0]["path"].as_str().expect("path string");
        assert!(path.ends_with("src/main.rs"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn held_search_worker_does_not_block_immediate_cancellation() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let cancellation = CancellationSignal::new();
        let task_cancellation = cancellation.clone();
        let search = tokio::spawn(async move {
            tokio::select! {
                biased;
                () = wait_for_cancellation(&task_cancellation) => true,
                result = run_blocking_search(move || {
                    started_tx.send(()).expect("announce held search");
                    release_rx.recv().expect("release held search");
                    Ok(Vec::new())
                }) => {
                    result.expect("held search worker");
                    false
                },
            }
        });

        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .expect("blocking search must yield the async runtime")
            .expect("held search started");
        cancellation.cancel();
        let cancellation_won = tokio::time::timeout(Duration::from_millis(250), search)
            .await
            .expect("immediate cancellation must not wait for the search worker")
            .expect("search task");
        release_tx.send(()).expect("release background search");

        assert!(cancellation_won);
    }

    async fn wait_for_cancellation(cancellation: &CancellationSignal) {
        while !cancellation.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}
