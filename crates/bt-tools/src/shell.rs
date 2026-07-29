use crate::text::{MAX_TOOL_RESULT_BYTES, MAX_TOOL_RESULT_LINES, truncate_text};
use bt_core::{
    ApprovalRequirement, BelltowerError, Result, ToolContext, ToolDisplayGroup, ToolExecutionMode,
    ToolExecutor, ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;
use tokio::time::Duration;

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

#[derive(Clone, Copy, Debug)]
pub struct ShellTool {
    timeout_seconds: u64,
}

impl ShellTool {
    #[must_use]
    pub fn new(timeout_seconds: u64) -> Self {
        Self { timeout_seconds }
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new(120)
    }
}

impl ToolExecutor for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_owned(),
            description: "Run a shell command from the project root.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["command", "call_id"],
                "properties": {
                    "command": {"type": "string"},
                    "call_id": {"type": "string"},
                    "timeout_seconds": {"type": "integer"}
                }
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::High,
                is_read_only: false,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::TerminateProcess,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec![
                    "shell".to_owned(),
                    "command".to_owned(),
                    "execution".to_owned(),
                ],
                display_group: ToolDisplayGroup::Execution,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Always
    }

    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let command = arguments
                .get("command")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BelltowerError::Tool("missing string argument `command`".to_owned())
                })?
                .to_owned();
            let call_id = arguments
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BelltowerError::Tool("missing string argument `call_id`".to_owned())
                })?
                .to_owned();
            let timeout_seconds = arguments
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(self.timeout_seconds);
            if timeout_seconds == 0 || timeout_seconds > self.timeout_seconds {
                return Err(BelltowerError::Tool(format!(
                    "timeout_seconds must be between 1 and configured maximum {}",
                    self.timeout_seconds
                )));
            }

            let started = Instant::now();
            let mut prepared = prepare_shell_command(
                &command,
                &context.project_root,
                Duration::from_secs(timeout_seconds),
            )?;
            let cancellation_token = processkit::CancellationToken::new();
            if context
                .cancellation
                .as_ref()
                .is_some_and(bt_core::CancellationSignal::is_cancelled)
            {
                cancellation_token.cancel();
            }
            let cancellation_task = context.cancellation.clone().map(|signal| {
                let token = cancellation_token.clone();
                tokio::spawn(async move {
                    wait_for_cancellation(&signal).await;
                    token.cancel();
                })
            });

            if context.cancellation.is_some() {
                prepared.command = prepared.command.cancel_on(cancellation_token);
            }

            let process_result = prepared.command.output_string().await;
            if let Some(task) = cancellation_task {
                task.abort();
            }

            match process_result {
                Ok(result) => {
                    let (stdout, stdout_truncated) = bounded_text(result.stdout());
                    let (stderr, stderr_truncated) = bounded_text(result.stderr());
                    let capture_truncated = result.truncated();
                    Ok(ToolResultEnvelope {
                        call_id: bt_core::ToolCallId::new(call_id),
                        tool_name: "shell".to_owned(),
                        is_error: !result.is_success(),
                        output: json!({
                            "command": command,
                            "status": result.code(),
                            "stdout": stdout,
                            "stderr": stderr,
                            "timed_out": result.timed_out(),
                            "cancelled": false,
                            "output_truncated": capture_truncated
                                || stdout_truncated
                                || stderr_truncated,
                        }),
                        duration_ms: Some(started.elapsed().as_millis() as u64),
                    })
                }
                Err(error) if error.kind() == processkit::ErrorKind::Cancelled => {
                    Ok(ToolResultEnvelope {
                        call_id: bt_core::ToolCallId::new(call_id),
                        tool_name: "shell".to_owned(),
                        is_error: true,
                        output: json!({
                            "command": command,
                            "status": Value::Null,
                            "stdout": "",
                            "stderr": "",
                            "timed_out": false,
                            "cancelled": true,
                            "output_truncated": false,
                        }),
                        duration_ms: Some(started.elapsed().as_millis() as u64),
                    })
                }
                Err(error) => Err(BelltowerError::Tool(error.to_string())),
            }
        })
    }
}

struct PreparedShellCommand {
    command: processkit::Command,
    #[cfg(windows)]
    _script_dir: tempfile::TempDir,
}

fn prepare_shell_command(
    command: &str,
    project_root: &camino::Utf8Path,
    timeout: Duration,
) -> Result<PreparedShellCommand> {
    #[cfg(windows)]
    {
        // cmd.exe's /S /C quote rules are not CommandLineToArgvW rules. Keeping
        // user syntax in a script makes nested quotes and metacharacters reach
        // cmd unchanged while processkit can safely quote the script path.
        let script_dir = tempfile::Builder::new()
            .prefix("belltower shell ")
            .tempdir()
            .map_err(io_error)?;
        let script_path = script_dir.path().join("command.cmd");
        std::fs::write(&script_path, format!("@echo off\r\n{command}\r\n")).map_err(io_error)?;
        let shell = processkit::Command::new("cmd.exe")
            .args(["/D", "/S", "/C"])
            .arg(&script_path)
            .current_dir(project_root)
            .timeout(timeout)
            .output_buffer(output_buffer_policy());
        Ok(PreparedShellCommand {
            command: shell,
            _script_dir: script_dir,
        })
    }

    #[cfg(not(windows))]
    {
        let shell = processkit::Command::new("/bin/sh")
            .arg("-lc")
            .arg(command)
            .current_dir(project_root)
            .timeout(timeout)
            .output_buffer(output_buffer_policy());
        Ok(PreparedShellCommand { command: shell })
    }
}

fn output_buffer_policy() -> processkit::OutputBufferPolicy {
    processkit::OutputBufferPolicy::bounded(MAX_TOOL_RESULT_LINES)
        .with_max_bytes(MAX_TOOL_RESULT_BYTES)
}

fn bounded_text(text: &str) -> (String, bool) {
    let bounded = truncate_text(text, MAX_TOOL_RESULT_LINES, MAX_TOOL_RESULT_BYTES);
    let truncated = bounded != text;
    (bounded, truncated)
}

async fn wait_for_cancellation(signal: &bt_core::CancellationSignal) {
    while !signal.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(windows)]
fn io_error(error: std::io::Error) -> BelltowerError {
    BelltowerError::Tool(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::ShellTool;
    use bt_core::{CancellationSignal, ToolContext, traits::ToolExecutor};
    #[cfg(unix)]
    use serde_json::Value;
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::time::{Duration, sleep};

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_the_process_tree_and_reports_partial_output() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let output_path = root.join("linger.txt");
        let shell = ShellTool::default();

        let result = shell
            .execute(
                json!({
                    "command": format!("printf 'start\\n'; sleep 2; echo linger > {}", output_path),
                    "call_id": "call-1",
                    "timeout_seconds": 1
                }),
                ToolContext {
                    project_root: root.clone(),
                    cancellation: None,
                },
            )
            .await
            .expect("shell result");

        assert!(result.is_error);
        assert_eq!(result.output["timed_out"], true);
        assert_eq!(result.output["status"], Value::Null);
        assert!(
            result
                .output
                .get("stdout")
                .and_then(Value::as_str)
                .expect("stdout")
                .contains("start")
        );

        sleep(Duration::from_secs(2)).await;
        assert!(!output_path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_the_process_tree_and_reports_cancelled() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let output_path = root.join("cancel-linger.txt");
        let shell = ShellTool::new(5);
        let cancellation = CancellationSignal::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            shell.execute(
                json!({
                    "command": format!("printf 'start\\n'; sleep 2; echo linger > {}", output_path),
                    "call_id": "call-cancel",
                }),
                ToolContext::new(root.clone()).with_cancellation(cancellation),
            ),
        )
        .await
        .expect("cancellation must bound the shell")
        .expect("shell cancellation result");

        assert!(result.is_error);
        assert_eq!(result.output["cancelled"], true);
        assert_eq!(result.output["timed_out"], false);
        sleep(Duration::from_secs(2)).await;
        assert!(!output_path.exists());
    }

    #[tokio::test]
    async fn configured_timeout_is_the_maximum_allowed_override() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let shell = ShellTool::new(3);
        let error = shell
            .execute(
                json!({
                    "command": "true",
                    "call_id": "call-too-long",
                    "timeout_seconds": 4,
                }),
                ToolContext::new(root),
            )
            .await
            .expect_err("override above configured maximum must fail");
        assert!(error.to_string().contains("configured maximum 3"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_is_bounded_and_reports_truncation() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let result = ShellTool::new(2)
            .execute(
                json!({
                    "command": "yes belltower-output | head -n 10000",
                    "call_id": "call-bounded-output",
                }),
                ToolContext::new(root),
            )
            .await
            .expect("bounded shell result");

        assert!(!result.is_error);
        assert_eq!(result.output["output_truncated"], true);
        assert!(
            result.output["stdout"].as_str().expect("stdout").len() <= super::MAX_TOOL_RESULT_BYTES
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_shell_executes_cmd_builtins() {
        let dir = TempDir::new().expect("tempdir");
        let root_path = dir.path().join("root with spaces");
        std::fs::create_dir(&root_path).expect("project root with spaces");
        let root = camino::Utf8PathBuf::from_path_buf(root_path).expect("utf8 path");
        let result = ShellTool::new(2)
            .execute(
                json!({
                    "command": "echo \"quoted value\" & echo metachar-ok",
                    "call_id": "call-windows-shell",
                }),
                ToolContext::new(root),
            )
            .await
            .expect("cmd.exe shell result");

        assert!(!result.is_error);
        assert!(
            result.output["stdout"]
                .as_str()
                .expect("stdout")
                .contains("\"quoted value\"")
        );
        assert!(
            result.output["stdout"]
                .as_str()
                .expect("stdout")
                .contains("metachar-ok")
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_timeout_terminates_the_cmd_process_tree() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let marker = root.join("timeout-marker.txt");
        let shell = ShellTool::new(1);
        let delayed_marker = windows_delayed_marker_command(&marker, 2);

        let result = tokio::time::timeout(
            Duration::from_secs(3),
            shell.execute(
                json!({
                    "command": delayed_marker,
                    "call_id": "call-windows-tree",
                }),
                ToolContext::new(root),
            ),
        )
        .await
        .expect("timeout must bound the cmd process tree")
        .expect("shell timeout result");

        assert!(result.is_error);
        assert_eq!(result.output["timed_out"], true);
        sleep(Duration::from_secs(2)).await;
        assert!(!marker.exists(), "timed-out descendant wrote its marker");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_cancellation_terminates_the_cmd_process_tree() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let marker = root.join("cancel-marker.txt");
        let cancellation = CancellationSignal::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });

        let result = tokio::time::timeout(
            Duration::from_secs(3),
            ShellTool::new(5).execute(
                json!({
                    "command": windows_delayed_marker_command(&marker, 2),
                    "call_id": "call-windows-cancel",
                }),
                ToolContext::new(root).with_cancellation(cancellation),
            ),
        )
        .await
        .expect("cancellation must bound the cmd process tree")
        .expect("shell cancellation result");

        assert!(result.is_error);
        assert_eq!(result.output["cancelled"], true);
        assert_eq!(result.output["timed_out"], false);
        sleep(Duration::from_secs(2)).await;
        assert!(!marker.exists(), "cancelled descendant wrote its marker");
    }

    #[cfg(windows)]
    fn windows_delayed_marker_command(path: &camino::Utf8Path, delay_seconds: u64) -> String {
        let escaped_path = path.as_str().replace('\'', "''");
        format!(
            "powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"Start-Sleep -Seconds {delay_seconds}; [System.IO.File]::WriteAllText('{escaped_path}', 'linger')\""
        )
    }
}
