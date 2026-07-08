//! Local server/TUI helper launch and workspace helper discovery for the belltower CLI.
//!
//! This module owns process startup, helper binary resolution, and local server
//! compatibility checks. CLI command parsing, provider setup, and readiness
//! reporting remain in `main.rs`.

use bt_core::{
    BelltowerError, ConnectionId, Result, StartupTrace, auth_token_path,
    server_auth_token_path_for_url,
};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use std::collections::BTreeSet;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use toml::Value as TomlValue;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiLaunchMode {
    NewSession,
    ResumePicker,
}

pub(crate) fn spawn_server(host: &str, port: u16, fresh_helpers: bool) -> Result<Child> {
    let args = vec![
        "--host".to_owned(),
        host.to_owned(),
        "--port".to_owned(),
        port.to_string(),
    ];
    spawn_binary("bt-server", "bt-server", &args, true, fresh_helpers)
}

pub(crate) fn pick_unused_port(host: &str) -> Result<u16> {
    let listener = TcpListener::bind((host, 0)).map_err(BelltowerError::Io)?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(BelltowerError::Io)
}

pub(crate) fn spawn_tui(
    server_url: &str,
    connection_id: &ConnectionId,
    project_root: &str,
    launch_mode: TuiLaunchMode,
    fresh_helpers: bool,
) -> Result<std::process::ExitStatus> {
    let args = vec![
        "--server".to_owned(),
        server_url.to_owned(),
        match launch_mode {
            TuiLaunchMode::NewSession => "chat".to_owned(),
            TuiLaunchMode::ResumePicker => "resume".to_owned(),
        },
        "--connection".to_owned(),
        connection_id.to_string(),
        "--project-root".to_owned(),
        project_root.to_owned(),
    ];
    let mut child = spawn_binary("bt-tui", "bt-tui", &args, false, fresh_helpers)?;
    child.wait().map_err(BelltowerError::Io)
}

fn spawn_binary(
    binary_name: &str,
    package_name: &str,
    args: &[String],
    background: bool,
    fresh_helpers: bool,
) -> Result<Child> {
    let mut command = if let Some(path) = resolve_binary(binary_name, fresh_helpers) {
        Command::new(path)
    } else if workspace_root().is_some() {
        let mut command = Command::new("cargo");
        command.arg("run").arg("-p").arg(package_name).arg("--");
        if let Some(root) = workspace_root() {
            command.current_dir(root);
        }
        command
    } else {
        return Err(BelltowerError::InvalidState(format!(
            "could not resolve `{binary_name}` from sibling binaries or PATH, and no Cargo workspace is available for fallback"
        )));
    };

    command.args(args);
    if background {
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
    }
    command.spawn().map_err(BelltowerError::Io)
}

pub(crate) fn resolve_binary(name: &str, fresh_helpers: bool) -> Option<PathBuf> {
    if should_prefer_workspace_helpers() {
        return if !fresh_helpers
            && workspace_helper_mode_for_current_process(name, fresh_helpers)
                == WorkspaceHelperMode::BuiltBinary
        {
            sibling_binary(name)
        } else {
            None
        };
    }
    if let Some(path) = sibling_binary(name) {
        return Some(path);
    }
    if command_in_path(name) {
        return Some(PathBuf::from(name));
    }
    None
}

fn sibling_binary(name: &str) -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let dir = current.parent()?;
    let candidate = dir.join(name);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

pub(crate) fn sibling_binary_is_fresh_for(
    _current_exe: &std::path::Path,
    sibling: &std::path::Path,
    workspace_root: Option<&std::path::Path>,
    binary_name: &str,
) -> bool {
    let Ok(sibling_metadata) = fs::metadata(sibling) else {
        return false;
    };
    let Ok(sibling_modified) = sibling_metadata.modified() else {
        return false;
    };
    let Some(latest_source_edit) = latest_helper_source_edit(workspace_root, binary_name) else {
        return true;
    };
    sibling_modified >= latest_source_edit
}

fn latest_helper_source_edit(
    workspace_root: Option<&Path>,
    binary_name: &str,
) -> Option<std::time::SystemTime> {
    let workspace_root = workspace_root?;
    let crate_root = workspace_root.join("crates").join(binary_name);
    latest_workspace_crate_edit(&crate_root, &mut BTreeSet::new())
}

fn latest_workspace_crate_edit(
    crate_root: &Path,
    visited: &mut BTreeSet<PathBuf>,
) -> Option<std::time::SystemTime> {
    let crate_root = normalize_existing_path(crate_root);
    if !visited.insert(crate_root.clone()) {
        return None;
    }

    let cargo_toml = crate_root.join("Cargo.toml");
    let src_dir = crate_root.join("src");
    let mut latest = latest_path_edit(&cargo_toml);
    if let Some(src_latest) = latest_tree_edit(&src_dir) {
        latest = match latest {
            Some(current) => Some(current.max(src_latest)),
            None => Some(src_latest),
        };
    }
    for dependency_root in local_path_dependencies(&cargo_toml) {
        if let Some(dependency_latest) = latest_workspace_crate_edit(&dependency_root, visited) {
            latest = match latest {
                Some(current) => Some(current.max(dependency_latest)),
                None => Some(dependency_latest),
            };
        }
    }
    latest
}

fn local_path_dependencies(manifest: &Path) -> Vec<PathBuf> {
    let Some(manifest_dir) = manifest.parent() else {
        return Vec::new();
    };
    let Ok(raw) = fs::read_to_string(manifest) else {
        return Vec::new();
    };
    let Ok(parsed) = raw.parse::<TomlValue>() else {
        return Vec::new();
    };

    let mut dependencies = Vec::new();
    collect_local_path_dependencies(&parsed, manifest_dir, "dependencies", &mut dependencies);
    collect_local_path_dependencies(
        &parsed,
        manifest_dir,
        "build-dependencies",
        &mut dependencies,
    );
    if let Some(targets) = parsed.get("target").and_then(TomlValue::as_table) {
        for target in targets.values() {
            collect_local_path_dependencies(
                target,
                manifest_dir,
                "dependencies",
                &mut dependencies,
            );
            collect_local_path_dependencies(
                target,
                manifest_dir,
                "build-dependencies",
                &mut dependencies,
            );
        }
    }

    dependencies
}

fn collect_local_path_dependencies(
    table_root: &TomlValue,
    manifest_dir: &Path,
    table_name: &str,
    dependencies: &mut Vec<PathBuf>,
) {
    let Some(table) = table_root.get(table_name).and_then(TomlValue::as_table) else {
        return;
    };
    for dependency in table.values() {
        let Some(path) = dependency
            .as_table()
            .and_then(|spec| spec.get("path"))
            .and_then(TomlValue::as_str)
        else {
            continue;
        };
        dependencies.push(normalize_existing_path(&manifest_dir.join(path)));
    }
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn latest_tree_edit(path: &Path) -> Option<std::time::SystemTime> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.is_file() {
        return metadata.modified().ok();
    }
    if !metadata.is_dir() {
        return None;
    }

    let mut latest = metadata.modified().ok();
    let entries = fs::read_dir(path).ok()?;
    for entry in entries.flatten() {
        if let Some(child_latest) = latest_tree_edit(&entry.path()) {
            latest = match latest {
                Some(current) => Some(current.max(child_latest)),
                None => Some(child_latest),
            };
        }
    }
    latest
}

fn latest_path_edit(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn command_in_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(name);
            candidate.exists()
        })
    })
}

fn workspace_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let manifest = dir.join("Cargo.toml");
        if manifest.exists() {
            let raw = fs::read_to_string(&manifest).ok()?;
            if raw.contains("[workspace]") {
                return Some(dir);
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub(crate) fn should_prefer_workspace_helpers() -> bool {
    let Some(workspace_root) = workspace_root() else {
        return false;
    };
    let Some(current_exe) = std::env::current_exe().ok() else {
        return false;
    };
    should_prefer_workspace_helpers_for(&current_exe, Some(&workspace_root))
}

fn should_prefer_workspace_helpers_for(
    current_exe: &std::path::Path,
    workspace_root: Option<&std::path::Path>,
) -> bool {
    let Some(workspace_root) = workspace_root else {
        return false;
    };
    current_exe.starts_with(workspace_root.join("target"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceHelperMode {
    BuiltBinary,
    CargoRun,
    NotWorkspace,
}

pub(crate) fn workspace_helper_mode(
    target: BinaryTarget,
    fresh_helpers: bool,
) -> WorkspaceHelperMode {
    workspace_helper_mode_for_current_process(binary_name_for(target), fresh_helpers)
}

fn workspace_helper_mode_for_current_process(
    binary_name: &str,
    fresh_helpers: bool,
) -> WorkspaceHelperMode {
    let Some(workspace_root) = workspace_root() else {
        return WorkspaceHelperMode::NotWorkspace;
    };
    let Some(current_exe) = std::env::current_exe().ok() else {
        return WorkspaceHelperMode::NotWorkspace;
    };
    let sibling_usable = sibling_binary(binary_name).as_ref().is_some_and(|path| {
        sibling_binary_is_fresh_for(&current_exe, path, Some(&workspace_root), binary_name)
    });
    workspace_helper_mode_for(
        &current_exe,
        Some(&workspace_root),
        sibling_usable,
        fresh_helpers,
    )
}

pub(crate) fn workspace_helper_mode_for(
    current_exe: &std::path::Path,
    workspace_root: Option<&std::path::Path>,
    sibling_usable: bool,
    fresh_helpers: bool,
) -> WorkspaceHelperMode {
    if !should_prefer_workspace_helpers_for(current_exe, workspace_root) {
        return WorkspaceHelperMode::NotWorkspace;
    }
    if sibling_usable && !fresh_helpers {
        WorkspaceHelperMode::BuiltBinary
    } else {
        WorkspaceHelperMode::CargoRun
    }
}

pub(crate) async fn server_healthy(server_url: &str) -> bool {
    let client = reqwest::Client::new();
    let Ok(response) = client
        .get(format!("{server_url}health"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
    else {
        return false;
    };
    response.status() == StatusCode::OK
}

#[cfg(test)]
pub(crate) async fn server_protocol_ready_with_token_paths(
    server_url: &str,
    token_paths: &[std::path::PathBuf],
) -> bool {
    server_healthy(server_url).await
        && server_auth_compatible_with_token_paths(server_url, token_paths).await
}

pub(crate) async fn server_auth_compatible(server_url: &str) -> bool {
    let Ok(candidate_paths) = server_auth_token_candidate_paths(server_url) else {
        return false;
    };
    server_auth_compatible_with_token_paths(server_url, &candidate_paths).await
}

async fn server_auth_compatible_with_token_paths(
    server_url: &str,
    token_paths: &[std::path::PathBuf],
) -> bool {
    for path in token_paths {
        if server_auth_compatible_with_token_path(server_url, path).await {
            return true;
        }
    }
    false
}

pub(crate) async fn server_auth_compatible_with_token_path(
    server_url: &str,
    token_path: &std::path::Path,
) -> bool {
    let Ok(token) = fs::read_to_string(token_path) else {
        return false;
    };
    let token = token.trim();
    if token.is_empty() {
        return false;
    }

    let Ok(auth_value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        return false;
    };

    let client = reqwest::Client::new();
    let Ok(response) = client
        .get(format!("{server_url}connections"))
        .header(AUTHORIZATION, auth_value)
        .timeout(Duration::from_millis(750))
        .send()
        .await
    else {
        return false;
    };

    response.status() == StatusCode::OK
}

pub(crate) fn server_auth_token_candidate_paths(
    server_url: &str,
) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    let url = Url::parse(server_url).map_err(BelltowerError::Url)?;
    let endpoint = server_auth_token_path_for_url(&url)?.into_std_path_buf();
    paths.push(endpoint.clone());
    let legacy = auth_token_path().into_std_path_buf();
    if legacy != endpoint {
        paths.push(legacy);
    }
    Ok(paths)
}

pub(crate) async fn wait_for_server(server_url: &str, timeout: Duration) -> Result<()> {
    let mut trace = StartupTrace::from_env("belltower-wait");
    let started_at = std::time::Instant::now();
    let mut attempt = 0;
    trace.mark("wait.start");
    while started_at.elapsed() < timeout {
        attempt += 1;
        if server_accepting_connections(server_url).await {
            trace.mark(format!("wait.ready.attempt.{attempt}"));
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    trace.mark(format!("wait.timeout.attempts.{attempt}"));
    Err(BelltowerError::InvalidState(format!(
        "server at `{server_url}` did not become healthy within {:?}",
        timeout
    )))
}

async fn server_accepting_connections(server_url: &str) -> bool {
    let Ok(url) = Url::parse(server_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let Some(port) = url.port_or_known_default() else {
        return false;
    };

    tokio::time::timeout(
        Duration::from_millis(100),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

#[derive(Clone, Copy)]
pub(crate) enum BinaryTarget {
    Server,
    Tui,
}

pub(crate) fn binary_name_for(target: BinaryTarget) -> &'static str {
    match target {
        BinaryTarget::Server => "bt-server",
        BinaryTarget::Tui => "bt-tui",
    }
}

pub(crate) fn binary_resolution_message(target: BinaryTarget) -> String {
    let name = binary_name_for(target);
    match workspace_helper_mode(target, false) {
        WorkspaceHelperMode::BuiltBinary => {
            return "using built workspace helper binaries".to_owned();
        }
        WorkspaceHelperMode::CargoRun => {
            return "using `cargo run` helper binaries from the workspace".to_owned();
        }
        WorkspaceHelperMode::NotWorkspace => {}
    }
    if sibling_binary(name).is_some() {
        return "resolved next to the launcher binary".to_owned();
    }
    if command_in_path(name) {
        return "resolved from PATH".to_owned();
    }
    if workspace_root().is_some() {
        return "will fall back to `cargo run` from the workspace".to_owned();
    }
    "not found; install the helper binary or run from the Belltower workspace".to_owned()
}
