use crate::text::{MAX_TOOL_RESULT_BYTES, MAX_TOOL_RESULT_LINES, truncate_text};
use bt_core::{
    ApprovalRequirement, BelltowerError, Result, ToolContext, ToolDisplayGroup, ToolExecutionMode,
    ToolExecutor, ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::{Duration, timeout};

#[cfg(unix)]
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

pub struct ShellTool;

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
                .unwrap_or(120);

            let mut child = Command::new("/bin/sh");
            child
                .arg("-lc")
                .arg(&command)
                .current_dir(&context.project_root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            child.process_group(0);

            let started = Instant::now();
            let mut child = child.spawn().map_err(io_error)?;
            let pid = child.id();
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| BelltowerError::Tool("shell stdout pipe missing".to_owned()))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| BelltowerError::Tool("shell stderr pipe missing".to_owned()))?;

            let stdout_task = tokio::spawn(async move { read_stream(stdout).await });
            let stderr_task = tokio::spawn(async move { read_stream(stderr).await });

            let timed_out = match timeout(Duration::from_secs(timeout_seconds), child.wait()).await
            {
                Ok(status) => {
                    let status = status.map_err(io_error)?;
                    let stdout = stdout_task.await.map_err(join_error)??;
                    let stderr = stderr_task.await.map_err(join_error)??;
                    return Ok(ToolResultEnvelope {
                        call_id: bt_core::ToolCallId::new(call_id),
                        tool_name: "shell".to_owned(),
                        is_error: !status.success(),
                        output: json!({
                            "command": command,
                            "status": status.code(),
                            "stdout": truncate_text(&stdout, MAX_TOOL_RESULT_LINES, MAX_TOOL_RESULT_BYTES),
                            "stderr": truncate_text(&stderr, MAX_TOOL_RESULT_LINES, MAX_TOOL_RESULT_BYTES),
                            "timed_out": false,
                        }),
                        duration_ms: Some(started.elapsed().as_millis() as u64),
                    });
                }
                Err(_) => true,
            };

            if timed_out {
                terminate_child(&mut child, pid).await?;
            }

            let stdout = stdout_task.await.map_err(join_error)??;
            let stderr = stderr_task.await.map_err(join_error)??;

            Ok(ToolResultEnvelope {
                call_id: bt_core::ToolCallId::new(call_id),
                tool_name: "shell".to_owned(),
                is_error: true,
                output: json!({
                    "command": command,
                    "status": Value::Null,
                    "stdout": truncate_text(&stdout, MAX_TOOL_RESULT_LINES, MAX_TOOL_RESULT_BYTES),
                    "stderr": truncate_text(&stderr, MAX_TOOL_RESULT_LINES, MAX_TOOL_RESULT_BYTES),
                    "timed_out": true,
                }),
                duration_ms: Some(started.elapsed().as_millis() as u64),
            })
        })
    }
}

async fn read_stream<T>(mut stream: T) -> Result<String>
where
    T: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.map_err(io_error)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn terminate_child(child: &mut tokio::process::Child, pid: Option<u32>) -> Result<()> {
    #[cfg(unix)]
    if let Some(pid) = pid {
        let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(())
}

fn io_error(error: std::io::Error) -> BelltowerError {
    BelltowerError::Tool(error.to_string())
}

fn join_error(error: tokio::task::JoinError) -> BelltowerError {
    BelltowerError::Tool(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::ShellTool;
    use bt_core::{ToolContext, traits::ToolExecutor};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn timeout_kills_the_process_tree_and_reports_partial_output() {
        let dir = TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
        let output_path = root.join("linger.txt");
        let shell = ShellTool;

        let result = shell
            .execute(
                json!({
                    "command": format!("printf 'start\\n'; sleep 2; echo linger > {}", output_path),
                    "call_id": "call-1",
                    "timeout_seconds": 1
                }),
                ToolContext {
                    project_root: root.clone(),
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
}
