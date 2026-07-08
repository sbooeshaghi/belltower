use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const STARTUP_TRACE_ENV: &str = "BELLTOWER_STARTUP_TRACE";

#[derive(Clone, Debug)]
enum StartupTraceSink {
    Stderr,
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct StartupTrace {
    process: &'static str,
    start: Instant,
    last: Instant,
    sink: Option<StartupTraceSink>,
}

impl StartupTrace {
    #[must_use]
    pub fn from_env(process: &'static str) -> Self {
        let sink = std::env::var(STARTUP_TRACE_ENV)
            .ok()
            .and_then(|value| trace_sink_from_env_value(&value));
        let now = Instant::now();
        Self {
            process,
            start: now,
            last: now,
            sink,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.sink.is_some()
    }

    pub fn mark(&mut self, stage: impl AsRef<str>) {
        let Some(sink) = &self.sink else {
            return;
        };
        let now = Instant::now();
        let total_ms = now.duration_since(self.start).as_secs_f64() * 1000.0;
        let step_ms = now.duration_since(self.last).as_secs_f64() * 1000.0;
        self.last = now;
        let stage = sanitize_stage(stage.as_ref());
        let line = format!(
            "startup_trace process={} total_ms={:.3} step_ms={:.3} stage={}\n",
            self.process, total_ms, step_ms, stage
        );
        match sink {
            StartupTraceSink::Stderr => {
                let _ = io::stderr().write_all(line.as_bytes());
            }
            StartupTraceSink::File(path) => {
                let _ = append_trace_line(path, &line);
            }
        }
    }
}

fn sanitize_stage(stage: &str) -> String {
    stage.split_whitespace().collect::<Vec<_>>().join("_")
}

fn trace_sink_from_env_value(value: &str) -> Option<StartupTraceSink> {
    let value = value.trim();
    match value {
        "" | "0" | "false" | "off" => None,
        "1" | "true" | "stderr" => Some(StartupTraceSink::Stderr),
        path => Some(StartupTraceSink::File(PathBuf::from(path))),
    }
}

fn append_trace_line(path: &Path, line: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}
