#![forbid(unsafe_code)]

use bt_auth::{
    ChatGptDeviceCodeSession, ChatGptLoginOptions, CredentialInput, CredentialStoreMode,
    OPENAI_CHATGPT_CLIENT_ID, OPENAI_CHATGPT_ISSUER, auth_storage_summary, clear_credentials,
    configured_credential_store, store_credential,
};
use bt_client::{BelltowerClient, ClientError};
use bt_core::{
    AuthMethodKind, BelltowerConfig, BelltowerError, BranchId, ConnectionDescriptor, ConnectionId,
    ConnectionModelInventory, ConnectionReadinessInspection, ConnectionSupportState, Message,
    ModelBackendStatus, PortableHash, Result, Role, SessionBundleArtifactMode, SessionId,
    StartupTrace, config_dir, config_path, persist_connection_default_model,
    persist_default_connection, persist_web_search_backend,
};
use bt_models::LocalModelManager;
use bt_protocol::{CreateSessionRequest, SendMessageOutcome, SendMessageRequest};
use bt_providers::connection_supported;
use bt_readiness::{
    backend_models_detail, format_backend_status, inspect_connection_model_inventory,
    inspect_connection_models, inspect_connection_status, inspect_connection_status_with_timeout,
    inspect_status, inspect_web_status,
};
use bt_session::{
    PortableSessionBundleExporter, PortableSessionBundleImporter, SessionBundleArtifactInput,
    SessionBundleExportOptions, SqliteSessionStore, continue_session_bundle_directory,
    diff_session_bundle_directories, pull_session_bundle_directory, push_session_bundle_directory,
    validate_session_bundle_directory,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use launcher::{
    BinaryTarget, TuiLaunchMode, WorkspaceHelperMode, binary_name_for, binary_resolution_message,
    pick_unused_port, resolve_binary, server_auth_compatible, server_healthy,
    should_prefer_workspace_helpers, spawn_server, spawn_tui, wait_for_server,
    workspace_helper_mode,
};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand};
use std::time::Duration;
use url::Url;

mod launcher;

const DEFAULT_SERVER_START_TIMEOUT: Duration = Duration::from_secs(10);
const WORKSPACE_SERVER_START_TIMEOUT: Duration = Duration::from_secs(90);
const IMPLICIT_REMOTE_READINESS_TIMEOUT: Duration = Duration::from_millis(750);
const TUI_DETACH_EXIT_CODE: i32 = 75;

#[derive(Parser, Debug)]
#[command(name = "belltower")]
#[command(version)]
#[command(about = "Belltower launcher, setup, and operator CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<CommandSet>,
}

#[derive(Subcommand, Debug)]
enum CommandSet {
    Chat(ChatArgs),
    Resume(ChatArgs),
    Run(RunArgs),
    Session(SessionArgs),
    Setup(SetupArgs),
    Login(LoginArgs),
    Logout(LogoutArgs),
    Model(ModelArgs),
    Web(WebArgs),
    Doctor,
    Status(StatusArgs),
}

#[derive(Args, Debug, Clone, Default)]
struct ChatArgs {
    #[arg(long)]
    server: Option<String>,
    #[arg(long)]
    connection: Option<ConnectionId>,
    #[arg(long)]
    project_root: Option<String>,
    #[arg(long)]
    no_setup: bool,
    #[arg(long)]
    fresh_helpers: bool,
}

#[derive(Args, Debug, Clone, Default)]
struct RunArgs {
    #[arg(long)]
    server: Option<String>,
    #[arg(long)]
    connection: Option<ConnectionId>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    project_root: Option<String>,
    #[arg(long)]
    no_setup: bool,
    #[arg(long)]
    fresh_helpers: bool,
    #[arg(long, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    #[arg(long, value_name = "PATH", conflicts_with = "prompt")]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug, Clone)]
struct SessionArgs {
    #[command(subcommand)]
    command: SessionCommand,
}

#[derive(Subcommand, Debug, Clone)]
enum SessionCommand {
    Export(SessionExportArgs),
    Validate(SessionValidateArgs),
    Import(SessionImportArgs),
    Diff(SessionDiffArgs),
    Continue(SessionContinueArgs),
    Push(SessionPushArgs),
    Pull(SessionPullArgs),
}

#[derive(Args, Debug, Clone)]
struct SessionExportArgs {
    session_id: SessionId,
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,
    #[arg(long, value_enum)]
    mode: Option<SessionExportModeArg>,
    #[arg(long = "artifact", value_name = "PATH")]
    artifacts: Vec<PathBuf>,
}

#[derive(Clone, Debug, ValueEnum)]
enum SessionExportModeArg {
    TraceOnly,
    TracePlusPatches,
    TracePlusArtifacts,
}

impl From<SessionExportModeArg> for SessionBundleArtifactMode {
    fn from(value: SessionExportModeArg) -> Self {
        match value {
            SessionExportModeArg::TraceOnly => Self::TraceOnly,
            SessionExportModeArg::TracePlusPatches => Self::TracePlusPatches,
            SessionExportModeArg::TracePlusArtifacts => Self::TracePlusArtifacts,
        }
    }
}

#[derive(Args, Debug, Clone)]
struct SessionValidateArgs {
    #[arg(value_name = "DIR")]
    bundle: PathBuf,
}

#[derive(Args, Debug, Clone)]
struct SessionImportArgs {
    #[arg(value_name = "DIR")]
    bundle: PathBuf,
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
struct SessionDiffArgs {
    #[arg(value_name = "LEFT_DIR")]
    left: PathBuf,
    #[arg(value_name = "RIGHT_DIR")]
    right: PathBuf,
}

#[derive(Args, Debug, Clone)]
struct SessionContinueArgs {
    #[arg(value_name = "DIR")]
    bundle: PathBuf,
    #[arg(long, value_name = "HASH")]
    event_hash: Option<String>,
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
struct SessionPushArgs {
    #[arg(value_name = "DIR")]
    bundle: PathBuf,
    #[arg(long, value_name = "DIR")]
    remote: PathBuf,
    #[arg(long, value_name = "ID")]
    branch: Option<BranchId>,
    #[arg(long, value_name = "HASH")]
    expected_head: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct SessionPullArgs {
    #[arg(long, value_name = "DIR")]
    remote: PathBuf,
    #[arg(long, value_name = "ID")]
    session: SessionId,
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
    #[arg(long, value_name = "ID")]
    branch: Option<BranchId>,
}

#[derive(Args, Debug, Clone, Default)]
struct SetupArgs {
    #[arg(long)]
    connection: Option<ConnectionId>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    api_key_env: Option<String>,
    #[arg(long)]
    api_key_command: Option<String>,
    #[arg(long)]
    non_interactive: bool,
}

#[derive(Args, Debug, Clone)]
struct LoginArgs {
    connection: Option<ConnectionId>,
    #[arg(long, value_enum)]
    storage: Option<AuthStorageArg>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    api_key_env: Option<String>,
    #[arg(long)]
    api_key_command: Option<String>,
    #[arg(long)]
    device_auth: bool,
    #[arg(long, hide = true)]
    issuer_base_url: Option<String>,
    #[arg(long = "experimental-client-id", hide = true)]
    client_id: Option<String>,
}

#[derive(Args, Debug, Clone, Default)]
struct ModelArgs {
    #[arg(long)]
    connection: Option<ConnectionId>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    non_interactive: bool,
}

#[derive(Args, Debug, Clone, Default)]
struct LogoutArgs {
    connection: Option<ConnectionId>,
}

#[derive(Args, Debug, Clone)]
struct WebArgs {
    #[command(subcommand)]
    command: WebCommand,
}

#[derive(Subcommand, Debug, Clone)]
enum WebCommand {
    Status,
    Configure(WebBackendCredentialArgs),
    Login(WebBackendCredentialArgs),
    Logout(WebBackendOnlyArgs),
}

#[derive(Args, Debug, Clone)]
struct WebBackendCredentialArgs {
    backend: String,
    #[arg(long, value_enum)]
    storage: Option<AuthStorageArg>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    api_key_env: Option<String>,
    #[arg(long)]
    api_key_command: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct WebBackendOnlyArgs {
    backend: String,
}

#[derive(Clone, Debug, ValueEnum)]
enum AuthStorageArg {
    Auto,
    File,
    Keychain,
    Ephemeral,
}

impl From<AuthStorageArg> for CredentialStoreMode {
    fn from(value: AuthStorageArg) -> Self {
        match value {
            AuthStorageArg::Auto => Self::Auto,
            AuthStorageArg::File => Self::File,
            AuthStorageArg::Keychain => Self::Keychain,
            AuthStorageArg::Ephemeral => Self::Ephemeral,
        }
    }
}

#[derive(Args, Debug, Clone, Default)]
struct StatusArgs {
    #[arg(long)]
    probe: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(CommandSet::Chat(ChatArgs::default())) {
        CommandSet::Chat(args) => run_chat(args).await,
        CommandSet::Resume(args) => run_resume(args).await,
        CommandSet::Run(args) => run_headless(args).await,
        CommandSet::Session(args) => run_session(args).await,
        CommandSet::Setup(args) => run_setup(args).await,
        CommandSet::Login(args) => run_login(args).await,
        CommandSet::Logout(args) => run_logout(args).await,
        CommandSet::Model(args) => run_model(args).await,
        CommandSet::Web(args) => run_web(args).await,
        CommandSet::Doctor => run_doctor().await,
        CommandSet::Status(args) => run_status(args).await,
    }
}

async fn run_chat(args: ChatArgs) -> Result<()> {
    run_chat_mode(args, TuiLaunchMode::NewSession).await
}

async fn run_resume(args: ChatArgs) -> Result<()> {
    run_chat_mode(args, TuiLaunchMode::ResumePicker).await
}

async fn run_session(args: SessionArgs) -> Result<()> {
    match args.command {
        SessionCommand::Export(args) => run_session_export(args),
        SessionCommand::Validate(args) => run_session_validate(args),
        SessionCommand::Import(args) => run_session_import(args),
        SessionCommand::Diff(args) => run_session_diff(args),
        SessionCommand::Continue(args) => run_session_continue(args),
        SessionCommand::Push(args) => run_session_push(args),
        SessionCommand::Pull(args) => run_session_pull(args),
    }
}

fn run_session_export(args: SessionExportArgs) -> Result<()> {
    let database = args.database.unwrap_or_else(default_database_path);
    let store = SqliteSessionStore::open(&database)?;
    let artifact_mode = resolve_session_export_mode(args.mode, &args.artifacts);
    let mut artifact_inputs = args
        .artifacts
        .iter()
        .map(|path| {
            let kind = if matches!(&artifact_mode, SessionBundleArtifactMode::TracePlusPatches) {
                "patch"
            } else {
                "artifact"
            };
            SessionBundleArtifactInput::file(path, kind)
        })
        .collect::<Vec<_>>();
    if matches!(&artifact_mode, SessionBundleArtifactMode::TracePlusPatches)
        && let Some(patch_bytes) = git_diff_patch_artifact(&store, args.session_id)?
    {
        artifact_inputs.push(SessionBundleArtifactInput::bytes(
            "git-diff.patch",
            patch_bytes,
            "patch",
            Some("text/x-patch".to_owned()),
        ));
    }
    let options = SessionBundleExportOptions {
        artifact_mode,
        artifacts: artifact_inputs,
    };
    let manifest =
        store.export_session_bundle_directory_with_options(args.session_id, &args.out, &options)?;
    println!(
        "Exported session {} to {}",
        args.session_id,
        args.out.display()
    );
    println!(
        "bundle_hash={} events={} contents={} artifacts={}",
        manifest.bundle_hash,
        manifest
            .event_ranges
            .iter()
            .map(|range| range.ordinal_end - range.ordinal_start + 1)
            .sum::<u64>(),
        manifest.contents.len(),
        manifest.artifacts.len()
    );
    Ok(())
}

fn resolve_session_export_mode(
    mode: Option<SessionExportModeArg>,
    artifacts: &[PathBuf],
) -> SessionBundleArtifactMode {
    mode.map(SessionBundleArtifactMode::from)
        .unwrap_or_else(|| {
            if artifacts.is_empty() {
                SessionBundleArtifactMode::TraceOnly
            } else {
                SessionBundleArtifactMode::TracePlusArtifacts
            }
        })
}

fn git_diff_patch_artifact(
    store: &SqliteSessionStore,
    session_id: SessionId,
) -> Result<Option<Vec<u8>>> {
    let session = store
        .load_session(session_id)?
        .ok_or_else(|| BelltowerError::NotFound(format!("session {session_id}")))?;
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(&session.project_root)
        .arg("diff")
        .arg("--binary")
        .arg("--no-color")
        .arg("HEAD")
        .output()
        .map_err(|error| BelltowerError::Config(format!("failed to run git diff: {error}")))?;
    if !output.status.success() {
        return Err(BelltowerError::Config(format!(
            "failed to create patch artifact from {}: {}",
            session.project_root,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if output.stdout.is_empty() {
        Ok(None)
    } else {
        Ok(Some(output.stdout))
    }
}

fn run_session_validate(args: SessionValidateArgs) -> Result<()> {
    let report = validate_session_bundle_directory(&args.bundle)?;
    println!("Valid session bundle {}", args.bundle.display());
    println!(
        "bundle_hash={} session={} events={} raw_chunk_references={} raw_chunk_contents={} contents={} artifacts={}",
        report.bundle_hash,
        report.session_id,
        report.event_count,
        report.raw_chunk_reference_count,
        report.raw_chunk_content_count,
        report.content_count,
        report.artifact_count
    );
    Ok(())
}

fn run_session_import(args: SessionImportArgs) -> Result<()> {
    let database = args.database.unwrap_or_else(default_database_path);
    let mut store = SqliteSessionStore::open(&database)?;
    let report = store.import_session_bundle_directory(&args.bundle)?;
    println!("Imported session bundle {}", args.bundle.display());
    println!(
        "bundle_hash={} session={} branches={} events={} raw_chunk_references={} raw_chunk_contents={}",
        report.bundle_hash,
        report.session_id,
        report.branch_count,
        report.event_count,
        report.raw_chunk_reference_count,
        report.raw_chunk_content_count
    );
    Ok(())
}

fn run_session_diff(args: SessionDiffArgs) -> Result<()> {
    let report = diff_session_bundle_directories(&args.left, &args.right)?;
    println!(
        "Session bundle diff {} {}",
        args.left.display(),
        args.right.display()
    );
    println!(
        "relationship={} left_session={} right_session={}",
        report.relationship.as_str(),
        report.left.session_id,
        report.right.session_id
    );
    println!(
        "events common={} left_only={} right_only={}",
        report.common_event_count, report.left_only_event_count, report.right_only_event_count
    );
    println!(
        "branches left_only={} right_only={} contents left_only={} right_only={} artifacts left_only={} right_only={} redaction_changed={}",
        report.left_only_branch_ids.len(),
        report.right_only_branch_ids.len(),
        report.left_only_content_hashes.len(),
        report.right_only_content_hashes.len(),
        report.left_only_artifact_ids.len(),
        report.right_only_artifact_ids.len(),
        report.redaction_changed
    );
    Ok(())
}

fn run_session_continue(args: SessionContinueArgs) -> Result<()> {
    let database = args.database.unwrap_or_else(default_database_path);
    let mut store = SqliteSessionStore::open(&database)?;
    let event_hash = args
        .event_hash
        .as_deref()
        .map(PortableHash::new)
        .transpose()
        .map_err(|error| BelltowerError::Config(error.to_string()))?;
    let report = continue_session_bundle_directory(&mut store, &args.bundle, event_hash.as_ref())?;
    println!("Continued session bundle {}", args.bundle.display());
    println!(
        "bundle_hash={} session={} branch={} parent_branch={} parent_event={} parent_event_hash={} imported={}",
        report.bundle_hash,
        report.session_id,
        report.branch_id,
        report.parent_branch_id,
        report.parent_event_id,
        report.parent_event_hash,
        report.imported_session
    );
    Ok(())
}

fn run_session_push(args: SessionPushArgs) -> Result<()> {
    let expected_head = args
        .expected_head
        .as_deref()
        .map(PortableHash::new)
        .transpose()
        .map_err(|error| BelltowerError::Config(error.to_string()))?;
    let report = push_session_bundle_directory(
        &args.bundle,
        &args.remote,
        args.branch,
        expected_head.as_ref(),
    )?;
    println!("Pushed session bundle {}", args.bundle.display());
    println!(
        "remote={} session={} branch={} previous_head={} new_head={} bundle_hash={} copied_bundle={} changed_remote_head={}",
        args.remote.display(),
        report.session_id,
        report.branch_id,
        report
            .previous_head
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "none".to_owned()),
        report.new_head,
        report.bundle_hash,
        report.copied_bundle,
        report.changed_remote_head
    );
    Ok(())
}

fn run_session_pull(args: SessionPullArgs) -> Result<()> {
    let report = pull_session_bundle_directory(&args.remote, args.session, &args.out, args.branch)?;
    println!("Pulled session bundle to {}", report.output_dir.display());
    println!(
        "remote={} session={} branch={} head={} bundle_hash={}",
        args.remote.display(),
        report.session_id,
        report.branch_id,
        report.head_event_hash,
        report.bundle_hash
    );
    Ok(())
}

fn default_database_path() -> PathBuf {
    config_dir().join("belltower.sqlite").into_std_path_buf()
}

#[derive(Clone, Copy, Debug)]
enum LaunchNoticeTarget {
    Stdout,
    Stderr,
}

impl LaunchNoticeTarget {
    fn emit(self, message: impl AsRef<str>) {
        match self {
            Self::Stdout => println!("{}", message.as_ref()),
            Self::Stderr => eprintln!("{}", message.as_ref()),
        }
    }
}

struct PreparedLaunch {
    trace: StartupTrace,
    connection_id: ConnectionId,
    server_url: String,
    project_root: String,
    fresh_helpers: bool,
    launched_server: Option<Child>,
}

async fn run_chat_mode(args: ChatArgs, launch_mode: TuiLaunchMode) -> Result<()> {
    let mut launch = prepare_local_runtime(args, "belltower", LaunchNoticeTarget::Stdout).await?;

    launch.trace.mark("tui.spawn.start");
    let status = spawn_tui(
        &launch.server_url,
        &launch.connection_id,
        &launch.project_root,
        launch_mode,
        launch.fresh_helpers,
    )?;
    launch
        .trace
        .mark(format!("tui.exit.code.{}", status.code().unwrap_or(-1)));
    let detached = status.code() == Some(TUI_DETACH_EXIT_CODE);
    shutdown_launched_server(launch.launched_server.take(), detached);

    if !status.success() && !detached {
        return Err(BelltowerError::InvalidState(format!(
            "bt-tui exited with status {status}"
        )));
    }

    Ok(())
}

async fn prepare_local_runtime(
    args: ChatArgs,
    trace_name: &'static str,
    notice_target: LaunchNoticeTarget,
) -> Result<PreparedLaunch> {
    let mut trace = StartupTrace::from_env(trace_name);
    trace.mark("launcher.start");

    trace.mark("config.load.start");
    let mut config = BelltowerConfig::load(None)?;
    trace.mark("config.load.done");

    let explicit_connection = args.connection.is_some();
    let mut connection_id = args
        .connection
        .unwrap_or_else(|| config.defaults.default_connection.clone());
    trace.mark(format!("connection.selected.{connection_id}"));

    let mut connection = find_connection(&config, &connection_id)?;
    if !connection_supported(connection) {
        if explicit_connection || args.no_setup {
            return Err(unsupported_connection_belltower_error(connection));
        }
        notice_target.emit(format!(
            "Default connection `{}` is not runnable yet. Launching setup so you can choose a supported provider.",
            connection_id
        ));
        run_setup(SetupArgs {
            connection: None,
            model: None,
            api_key: None,
            api_key_env: None,
            api_key_command: None,
            non_interactive: false,
        })
        .await?;
        trace.mark("setup.default_connection.done");
        trace.mark("config.reload_after_setup.start");
        config = BelltowerConfig::load(None)?;
        trace.mark("config.reload_after_setup.done");
        connection_id = config.defaults.default_connection.clone();
        connection = find_connection(&config, &connection_id)?;
    }
    ensure_connection_supported(connection)?;

    trace.mark("readiness.selected.start");
    let mut readiness =
        launch_connection_readiness(&config, &connection_id, explicit_connection).await?;
    trace.mark(format!(
        "readiness.selected.done.{}",
        readiness.readiness_label()
    ));

    apply_local_fallback(
        &config,
        explicit_connection,
        &mut connection_id,
        &mut readiness,
        &mut trace,
        notice_target,
    )
    .await?;

    if !args.no_setup && connection_needs_launch_setup(&connection_id, &readiness) {
        notice_target.emit(format!(
            "Belltower needs setup for `{}` before starting chat.",
            connection_id
        ));
        run_setup(SetupArgs {
            connection: explicit_connection.then_some(connection_id.clone()),
            model: None,
            api_key: None,
            api_key_env: None,
            api_key_command: None,
            non_interactive: false,
        })
        .await?;
        trace.mark("setup.readiness.done");
        trace.mark("config.reload_after_readiness_setup.start");
        config = BelltowerConfig::load(None)?;
        trace.mark("config.reload_after_readiness_setup.done");
        trace.mark("readiness.selected_after_setup.start");
        readiness = connection_readiness(&config, &connection_id).await?;
        trace.mark(format!(
            "readiness.selected_after_setup.done.{}",
            readiness.readiness_label()
        ));
        apply_local_fallback(
            &config,
            explicit_connection,
            &mut connection_id,
            &mut readiness,
            &mut trace,
            notice_target,
        )
        .await?;
    }

    if !readiness.is_ready() {
        return Err(connection_not_ready_error(&connection_id, &readiness));
    }

    let default_server_url = format!("http://{}:{}/", config.server.host, config.server.port);
    let mut server_url = args
        .server
        .clone()
        .unwrap_or_else(|| default_server_url.clone());
    trace.mark("server.helper_mode.start");
    let workspace_helper_mode = workspace_helper_mode(BinaryTarget::Server, args.fresh_helpers);
    trace.mark(format!("server.helper_mode.done.{workspace_helper_mode:?}"));
    let startup_timeout = if workspace_helper_mode == WorkspaceHelperMode::CargoRun {
        WORKSPACE_SERVER_START_TIMEOUT
    } else {
        DEFAULT_SERVER_START_TIMEOUT
    };
    let mut launched_server = None;
    if args.server.is_none() {
        trace.mark("server.default_health.start");
        if server_healthy(&default_server_url).await {
            trace.mark("server.default_health.done.healthy");
            if workspace_helper_mode == WorkspaceHelperMode::CargoRun {
                let fresh_port = pick_unused_port(&config.server.host)?;
                server_url = format!("http://{}:{}/", config.server.host, fresh_port);
                notice_target.emit(format!(
                    "Launching a fresh workspace server at {server_url} so current bt-server changes are active. The first build can take a little while."
                ));
                trace.mark("server.spawn.fresh_workspace.start");
                launched_server = Some(spawn_server(
                    config.server.host.as_str(),
                    fresh_port,
                    args.fresh_helpers,
                )?);
                trace.mark("server.spawn.fresh_workspace.done");
                trace.mark("server.wait.fresh_workspace.start");
                wait_for_server(&server_url, startup_timeout).await?;
                trace.mark("server.wait.fresh_workspace.done");
            } else if server_auth_compatible(&default_server_url).await {
                trace.mark("server.default_auth.done.compatible");
                server_url = default_server_url;
            } else {
                trace.mark("server.default_auth.done.incompatible");
                let fresh_port = pick_unused_port(&config.server.host)?;
                server_url = format!("http://{}:{}/", config.server.host, fresh_port);
                notice_target.emit(format!(
                    "Running server at {default_server_url} is not compatible with the current auth token. Launching a compatible workspace server at {server_url}."
                ));
                trace.mark("server.spawn.compatible_workspace.start");
                launched_server = Some(spawn_server(
                    config.server.host.as_str(),
                    fresh_port,
                    args.fresh_helpers,
                )?);
                trace.mark("server.spawn.compatible_workspace.done");
                trace.mark("server.wait.compatible_workspace.start");
                wait_for_server(&server_url, startup_timeout).await?;
                trace.mark("server.wait.compatible_workspace.done");
            }
        } else {
            trace.mark("server.default_health.done.unhealthy");
            if workspace_helper_mode == WorkspaceHelperMode::CargoRun {
                notice_target.emit(format!(
                    "Starting workspace bt-server at {server_url}. The first build can take a little while."
                ));
            } else if should_prefer_workspace_helpers() {
                notice_target.emit(format!(
                    "Starting built workspace bt-server at {server_url}."
                ));
            }
            trace.mark("server.spawn.default.start");
            launched_server = Some(spawn_server(
                config.server.host.as_str(),
                config.server.port,
                args.fresh_helpers,
            )?);
            trace.mark("server.spawn.default.done");
            trace.mark("server.wait.default.start");
            wait_for_server(&server_url, startup_timeout).await?;
            trace.mark("server.wait.default.done");
        }
    }

    let project_root = args.project_root.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
            .to_string()
    });

    Ok(PreparedLaunch {
        trace,
        connection_id,
        server_url,
        project_root,
        fresh_helpers: args.fresh_helpers,
        launched_server,
    })
}

fn shutdown_launched_server(launched_server: Option<Child>, detached: bool) {
    if let Some(mut child) = launched_server
        && !detached
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

async fn run_headless(args: RunArgs) -> Result<()> {
    let prompt = read_headless_prompt(&args)?;
    let launch_args = ChatArgs {
        server: args.server.clone(),
        connection: args.connection.clone(),
        project_root: args.project_root.clone(),
        no_setup: args.no_setup,
        fresh_helpers: args.fresh_helpers,
    };
    let mut launch =
        prepare_local_runtime(launch_args, "belltower-run", LaunchNoticeTarget::Stderr).await?;
    let result = run_headless_turn(&args, &prompt, &launch).await;
    shutdown_launched_server(launch.launched_server.take(), false);
    result
}

const HEADLESS_TURN_TIMEOUT_SECONDS: u64 = 600;

async fn run_headless_turn(args: &RunArgs, prompt: &str, launch: &PreparedLaunch) -> Result<()> {
    let client = BelltowerClient::new_discovering_auth(Url::parse(&launch.server_url)?)
        .map_err(client_error)?;
    let create_response = client
        .create_session(&CreateSessionRequest {
            project_root: launch.project_root.clone(),
            connection_id: launch.connection_id.clone(),
            model_id: args.model.clone(),
            tool_mode: None,
            display_name: Some(headless_display_name(prompt)),
            objective: Some(prompt.to_owned()),
            budget: None,
            approval_mode: None,
        })
        .await
        .map_err(client_error)?;
    let send_response = client
        .send_message(
            create_response.session.session_id,
            &SendMessageRequest {
                branch_id: create_response.branch.branch_id,
                message: Message::text(Role::User, prompt),
            },
        )
        .await
        .map_err(client_error)?;
    // Turns run detached from the send POST; wait for the session to settle
    // before reading the outcome.
    client
        .wait_for_session_settle(
            create_response.session.session_id,
            std::time::Duration::from_secs(HEADLESS_TURN_TIMEOUT_SECONDS),
        )
        .await
        .map_err(client_error)?;
    let messages = client
        .session_messages(create_response.session.session_id)
        .await
        .map_err(client_error)?;
    let assistant_text = render_headless_assistant_output(&messages.messages);
    let turns = client
        .session_turns(create_response.session.session_id)
        .await
        .map_err(client_error)?;
    let last_turn_status = turns.turns.last().and_then(|turn| turn.status.clone());
    let turn_failed = last_turn_status.as_deref() == Some("failed");
    let session_errors = if turn_failed {
        collect_session_error_lines(&client, create_response.session.session_id).await
    } else {
        Vec::new()
    };

    if args.json {
        let payload = serde_json::json!({
            "session_id": create_response.session.session_id.to_string(),
            "branch_id": create_response.branch.branch_id.to_string(),
            "connection_id": create_response.session.connection_id.to_string(),
            "model_id": create_response.session.model_id,
            "outcome": headless_outcome_label(&send_response.outcome),
            "queued_position": headless_queued_position(&send_response.outcome),
            "turn_status": last_turn_status,
            "errors": session_errors,
            "assistant_text": assistant_text,
            "message_count": messages.messages.len(),
        });
        serde_json::to_writer_pretty(io::stdout(), &payload)?;
        println!();
    } else {
        eprintln!(
            "Session: {} Branch: {} | {} ({})",
            create_response.session.session_id,
            create_response.branch.branch_id,
            create_response.session.connection_id,
            create_response
                .session
                .model_id
                .as_deref()
                .unwrap_or("default model")
        );
        if let SendMessageOutcome::Queued { position } = send_response.outcome {
            eprintln!("Queued at position {position}.");
        }
        match last_turn_status.as_deref() {
            Some("awaiting_approval") => {
                eprintln!(
                    "Turn ended awaiting a tool approval. Approve it in the TUI or via POST /sessions/{{session_id}}/approve, or rerun with an auto-approving tool mode."
                );
            }
            Some("awaiting_input") => {
                eprintln!("Turn ended awaiting operator input (ask tool).");
            }
            _ => {}
        }
        if !turn_failed {
            if assistant_text.trim().is_empty() {
                eprintln!(
                    "No assistant text returned. Inspect the session for pending control state."
                );
            } else {
                println!("{}", assistant_text.trim_end());
            }
        }
    }

    if turn_failed {
        for line in &session_errors {
            eprintln!("session error: {line}");
        }
        return Err(BelltowerError::Provider(format!(
            "turn failed with {} recorded session error(s); session {}",
            session_errors.len(),
            create_response.session.session_id
        )));
    }

    Ok(())
}

async fn collect_session_error_lines(
    client: &BelltowerClient,
    session_id: bt_core::SessionId,
) -> Vec<String> {
    let Ok(events) = client.session_events(session_id, None).await else {
        return Vec::new();
    };
    events
        .events
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            bt_core::EventPayload::SessionError {
                class,
                code,
                message,
                retryable,
            } => Some(format!(
                "[{class}/{code}{}] {message}",
                if *retryable { ", retryable" } else { "" }
            )),
            _ => None,
        })
        .collect()
}

fn read_headless_prompt(args: &RunArgs) -> Result<String> {
    let prompt = if let Some(prompt) = &args.prompt {
        prompt.clone()
    } else if let Some(path) = &args.prompt_file {
        fs::read_to_string(path)?
    } else if io::stdin().is_terminal() {
        return Err(BelltowerError::Config(
            "`belltower run` requires --prompt, --prompt-file, or piped stdin".to_owned(),
        ));
    } else {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        input
    };
    if prompt.trim().is_empty() {
        return Err(BelltowerError::Config(
            "headless prompt cannot be empty".to_owned(),
        ));
    }
    Ok(prompt)
}

fn render_headless_assistant_output(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(|message| message.text_parts().collect::<Vec<_>>().join(""))
        .unwrap_or_default()
}

fn headless_display_name(prompt: &str) -> String {
    let compact = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 80 {
        compact
    } else {
        let mut display = compact.chars().take(77).collect::<String>();
        display.push_str("...");
        display
    }
}

fn headless_outcome_label(outcome: &SendMessageOutcome) -> &'static str {
    match outcome {
        SendMessageOutcome::Dispatched => "dispatched",
        SendMessageOutcome::Queued { .. } => "queued",
    }
}

fn headless_queued_position(outcome: &SendMessageOutcome) -> Option<usize> {
    match outcome {
        SendMessageOutcome::Dispatched => None,
        SendMessageOutcome::Queued { position } => Some(*position),
    }
}

fn client_error(error: ClientError) -> BelltowerError {
    match error {
        ClientError::Core(error) => error,
        other => BelltowerError::Protocol(other.to_string()),
    }
}

async fn run_setup(args: SetupArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    fs::create_dir_all(config_dir())?;

    let selected = match args.connection.clone() {
        Some(connection) => connection,
        None if args.non_interactive => {
            return Err(BelltowerError::Config(
                "--non-interactive setup requires --connection".to_owned(),
            ));
        }
        None => {
            println!("Welcome to Belltower setup.");
            println!();
            prompt_for_connection(&config, ConnectionPromptMode::Runnable).await?
        }
    };

    let selected_connection = find_connection(&config, &selected)?;
    ensure_connection_supported(selected_connection)?;
    if selected != ConnectionId::new("local") {
        let readiness = connection_readiness(&config, &selected).await?;
        let secret = resolve_secret_input(args.api_key, args.api_key_env, args.api_key_command)?;
        if let Some(secret) = secret {
            ensure_connection_supports_api_key_login(selected_connection)?;
            let storage = store_credential(selected.0.as_str(), secret, None)?;
            println!("Stored credentials in {storage}");
            println!(
                "Belltower will resolve them through the configured auth storage on each run."
            );
        } else if connection_needs_auth_setup(&readiness) {
            if connection_supports_device_code_login(selected_connection) {
                if args.non_interactive {
                    return Err(BelltowerError::Config(format!(
                        "non-interactive setup for `{selected}` requires `belltower login {selected}` to complete device-code auth first"
                    )));
                }
                let login_args = LoginArgs {
                    connection: Some(selected.clone()),
                    storage: None,
                    api_key: None,
                    api_key_env: None,
                    api_key_command: None,
                    device_auth: true,
                    issuer_base_url: None,
                    client_id: None,
                };
                run_chatgpt_device_login(&selected, &login_args, None).await?;
            } else {
                ensure_connection_supports_api_key_login(selected_connection)?;
                if args.non_interactive {
                    return Err(BelltowerError::Config(
                    "non-interactive setup for cloud providers requires --api-key, --api-key-env, or --api-key-command"
                        .to_owned(),
                ));
                }
                let storage = store_credential(
                    selected.0.as_str(),
                    prompt_for_credential_input(&selected)?,
                    None,
                )?;
                println!("Stored credentials in {storage}");
                println!(
                    "Belltower will resolve them through the configured auth storage on each run."
                );
            }
        } else {
            println!(
                "Credentials for `{selected}` are already configured. Leaving them unchanged."
            );
        }
    }
    let model = resolve_model_selection(
        &config,
        &selected,
        args.model.clone(),
        args.non_interactive,
        None,
    )
    .await?;
    persist_default_connection(&selected)?;
    persist_connection_default_model(&selected, &model)?;
    println!("Default connection set to `{selected}`.");
    println!("Default model set to `{model}`.");
    let refreshed = BelltowerConfig::load(None)?;
    let inspection = inspect_status(&refreshed).await?;
    let selected_connection = find_connection_inspection(&inspection, &selected)?;
    print_check(
        selected_connection.is_ready(),
        &format!("connection probe for `{selected}`"),
        format!(
            "{}: {}",
            selected_connection.readiness_label(),
            selected_connection.probe_status
        ),
    );
    println!("Start chatting with `belltower`.");
    Ok(())
}

async fn run_login(args: LoginArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let explicit_storage = args.storage.clone().map(Into::into);
    let connection = match args.connection.clone() {
        Some(connection) => connection,
        None => prompt_for_connection(&config, ConnectionPromptMode::Loginable).await?,
    };
    if connection == ConnectionId::new("local") {
        return Err(BelltowerError::Unsupported(
            "`local` does not require a login; use `belltower model --connection local` instead"
                .to_owned(),
        ));
    }
    let descriptor = find_connection(&config, &connection)?;
    if connection_supports_device_code_login(descriptor) {
        let api_key_override =
            args.api_key.is_some() || args.api_key_env.is_some() || args.api_key_command.is_some();
        if api_key_override {
            return Err(BelltowerError::Unsupported(format!(
                "connection `{connection}` uses device-code OAuth; API-key login flags are not supported"
            )));
        }
        return run_chatgpt_device_login(&connection, &args, explicit_storage).await;
    }

    ensure_connection_supported(descriptor)?;
    if args.device_auth {
        return Err(BelltowerError::Unsupported(format!(
            "connection `{connection}` does not support device-code login"
        )));
    }
    ensure_connection_supports_api_key_login(descriptor)?;

    let secret = match resolve_secret_input(args.api_key, args.api_key_env, args.api_key_command)? {
        Some(secret) => secret,
        None => prompt_for_credential_input(&connection)?,
    };
    let storage = store_credential(connection.0.as_str(), secret, explicit_storage)?;
    println!("Stored credentials for `{}` in {}", connection, storage);
    Ok(())
}

async fn run_chatgpt_device_login(
    connection: &ConnectionId,
    args: &LoginArgs,
    explicit_storage: Option<CredentialStoreMode>,
) -> Result<()> {
    let issuer = args
        .issuer_base_url
        .clone()
        .unwrap_or_else(|| OPENAI_CHATGPT_ISSUER.to_owned());
    let client_id = args
        .client_id
        .clone()
        .unwrap_or_else(|| OPENAI_CHATGPT_CLIENT_ID.to_owned());
    let session = ChatGptDeviceCodeSession::start(
        ChatGptLoginOptions::for_provider(connection.0.as_str())
            .with_shared_store(configured_credential_store(explicit_storage)?.store())
            .with_issuer(issuer)
            .with_client_id(client_id),
    )
    .await?;
    println!("To authenticate `{connection}` with ChatGPT, visit:");
    println!("  {}", session.authorize_url());
    println!("Enter this code when prompted:");
    println!("  {}", session.user_code());
    println!("Waiting for authorization...");
    let storage = session.wait_for_completion().await?;
    println!(
        "Stored ChatGPT credentials for `{}` in {}",
        connection, storage
    );
    Ok(())
}

async fn run_logout(args: LogoutArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let connection = match args.connection {
        Some(connection) => connection,
        None => prompt_for_connection(&config, ConnectionPromptMode::Logout).await?,
    };
    let storage = auth_storage_summary(None)?;
    if clear_credentials(connection.0.as_str(), None)? {
        println!("Removed stored credentials for `{connection}` from {storage}.",);
    } else {
        println!("No stored credentials found for `{connection}` in {storage}.",);
    }
    Ok(())
}

async fn run_web(args: WebArgs) -> Result<()> {
    match args.command {
        WebCommand::Status => run_web_status().await,
        WebCommand::Configure(args) => run_web_configure(args).await,
        WebCommand::Login(args) => run_web_login(args).await,
        WebCommand::Logout(args) => run_web_logout(args).await,
    }
}

async fn run_web_status() -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    print_web_status(&config)
}

async fn run_web_configure(args: WebBackendCredentialArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let backend = find_web_backend(&config, &args.backend)?;
    if !backend.enabled {
        return Err(BelltowerError::Config(format!(
            "web backend `{}` is disabled in config",
            backend.id
        )));
    }
    persist_web_search_backend(&backend.id)?;
    let explicit_storage = args.storage.clone().map(Into::into);
    if let Some(secret) =
        resolve_secret_input(args.api_key, args.api_key_env, args.api_key_command)?
    {
        let storage =
            store_credential(&backend.credential_provider_id(), secret, explicit_storage)?;
        println!(
            "Stored credentials for web backend `{}` in {storage}.",
            backend.id
        );
    }
    println!("Default web search backend set to `{}`.", backend.id);
    let refreshed = BelltowerConfig::load(None)?;
    print_web_status(&refreshed)
}

async fn run_web_login(args: WebBackendCredentialArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let backend = find_web_backend(&config, &args.backend)?;
    if !backend.enabled {
        return Err(BelltowerError::Config(format!(
            "web backend `{}` is disabled in config",
            backend.id
        )));
    }
    let explicit_storage = args.storage.clone().map(Into::into);
    let credential_id = ConnectionId::new(backend.credential_provider_id());
    let secret = match resolve_secret_input(args.api_key, args.api_key_env, args.api_key_command)? {
        Some(secret) => secret,
        None => prompt_for_credential_input(&credential_id)?,
    };
    let storage = store_credential(&credential_id.0, secret, explicit_storage)?;
    println!(
        "Stored credentials for web backend `{}` in {storage}.",
        backend.id
    );
    Ok(())
}

async fn run_web_logout(args: WebBackendOnlyArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let backend = find_web_backend(&config, &args.backend)?;
    let credential_id = backend.credential_provider_id();
    let storage = auth_storage_summary(None)?;
    if clear_credentials(&credential_id, None)? {
        println!(
            "Removed stored credentials for web backend `{}` from {storage}.",
            backend.id
        );
    } else {
        println!(
            "No stored credentials found for web backend `{}` in {storage}.",
            backend.id
        );
    }
    Ok(())
}

async fn run_model(args: ModelArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let selected = match args.connection {
        Some(connection) => connection,
        None if args.non_interactive => {
            return Err(BelltowerError::Config(
                "--non-interactive model selection requires --connection".to_owned(),
            ));
        }
        None => prompt_for_connection(&config, ConnectionPromptMode::Runnable).await?,
    };
    ensure_connection_supported(find_connection(&config, &selected)?)?;

    let mut remote_inventory = None;
    if selected != ConnectionId::new("local") {
        let mut inventory = inspect_connection_model_inventory(&config, &selected).await?;
        if inventory_needs_auth_setup(&inventory) {
            println!("No credentials found for `{selected}`.");
            if args.non_interactive {
                return Err(BelltowerError::Auth(format!(
                    "credentials are required for `{selected}` in non-interactive mode"
                )));
            }
            run_login(LoginArgs {
                connection: Some(selected.clone()),
                storage: None,
                api_key: None,
                api_key_env: None,
                api_key_command: None,
                device_auth: false,
                issuer_base_url: None,
                client_id: None,
            })
            .await?;
            inventory = inspect_connection_model_inventory(&config, &selected).await?;
        } else if !inventory.is_ready() {
            println!(
                "Connection `{selected}` is {}: {}. You can still change the default model.",
                inventory.readiness_label(),
                inventory.probe_status
            );
        }
        remote_inventory = Some(inventory);
    }

    let model = resolve_model_selection(
        &config,
        &selected,
        args.model,
        args.non_interactive,
        remote_inventory.as_ref(),
    )
    .await?;
    persist_default_connection(&selected)?;
    persist_connection_default_model(&selected, &model)?;
    println!("Default connection set to `{selected}`.");
    println!("Default model set to `{model}`.");
    Ok(())
}

async fn run_doctor() -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let inspection = inspect_status(&config).await?;
    let connection_models = inspect_connection_models(&config).await?;
    let local_model_manager = LocalModelManager::from_config(&config)?;
    let backends = local_model_manager.backends().await;
    let server_url = format!("http://{}:{}/", config.server.host, config.server.port);
    let server_running = server_healthy(&server_url).await;

    println!("Belltower doctor");
    println!();
    print_check(
        config_path().exists(),
        "config file",
        if config_path().exists() {
            format!("using {}", config_path())
        } else {
            format!("will create {}", config_path())
        },
    );
    print_check(true, "auth storage", auth_storage_summary(None)?);
    print_check(
        resolve_binary(binary_name_for(BinaryTarget::Server), false).is_some(),
        "server binary",
        binary_resolution_message(BinaryTarget::Server),
    );
    print_check(
        resolve_binary(binary_name_for(BinaryTarget::Tui), false).is_some(),
        "tui binary",
        binary_resolution_message(BinaryTarget::Tui),
    );
    print_check(server_running, "server health", server_url.clone());

    println!();
    println!("Connections:");
    for connection in &inspection.connections {
        print_check(
            connection.is_ready(),
            &format!("  {}", connection.connection_id),
            doctor_connection_detail(connection),
        );
    }

    println!();
    println!("Connection models:");
    for connection in &connection_models {
        println!("  {}", render_connection_model_inventory(connection));
    }

    println!();
    print_web_status(&config)?;

    println!();
    println!("Local model backends:");
    for backend in backends {
        let ready = matches!(backend.status, ModelBackendStatus::Ready);
        let status = format_backend_status(&backend.status);
        let models = backend_models_detail(&backend);
        print_check(
            ready,
            &format!("  {}", backend.label),
            format!("{status}; {models}"),
        );
    }

    println!();
    println!("Default connection: {}", config.defaults.default_connection);
    if let Some(connection) = config
        .connections
        .iter()
        .find(|connection| connection.id == config.defaults.default_connection)
    {
        println!("Default model: {}", connection.default_model);
    }

    println!();
    if !server_running {
        println!("Next step: run `belltower` to auto-start the server and launch the TUI.");
    } else {
        println!("Next step: run `belltower` to connect the TUI to the running server.");
    }
    Ok(())
}

async fn run_status(args: StatusArgs) -> Result<()> {
    let config = BelltowerConfig::load(None)?;
    let inspection = inspect_status(&config).await?;
    let connection_models = inspect_connection_models(&config).await?;
    let model_manager = LocalModelManager::from_config(&config)?;
    let model_backends = model_manager.backends().await;

    println!("Config dir: {}", config_dir());
    println!("Config file: {}", config_path());
    println!("Auth storage: {}", inspection.auth_storage);
    println!("Default connection: {}", inspection.default_connection);
    println!(
        "Server: http://{}:{}/",
        config.server.host, config.server.port
    );

    println!();
    println!("Connections:");
    for connection in inspection.connections {
        let auth_state = match connection.auth_state {
            bt_core::ConnectionAuthState::NotRequired => "not required",
            bt_core::ConnectionAuthState::Missing => "missing",
            bt_core::ConnectionAuthState::Configured => "configured",
        };
        let support = match connection.support_state {
            bt_core::ConnectionSupportState::RuntimeSupported => "runtime-supported",
            bt_core::ConnectionSupportState::Planned => "planned",
        };
        let note = match (&connection.auth_kind, &connection.auth_source) {
            (Some(kind), Some(source)) => {
                let kind = match kind {
                    bt_core::CredentialKind::ApiKey => "secret",
                    bt_core::CredentialKind::OAuthToken => "oauth",
                    bt_core::CredentialKind::JsonDocument => "json",
                };
                let tried = if connection.auth_tried_sources.is_empty() {
                    String::new()
                } else {
                    format!(" tried={}", connection.auth_tried_sources.join(" -> "))
                };
                format!(" kind={kind} source={source}{tried}")
            }
            _ => String::new(),
        };
        let methods_note = if connection.auth_methods.is_empty() {
            String::new()
        } else {
            format!(
                " methods={}",
                connection
                    .auth_methods
                    .iter()
                    .map(|method| method.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let probe_note = if args.probe {
            format!(" probe={}", connection.probe_status)
        } else {
            String::new()
        };
        println!(
            "  {} ({}) [auth={} support={} readiness={}] model={}{}{}{}",
            connection.connection_id,
            connection.provider,
            auth_state,
            support,
            connection.readiness_label(),
            connection.default_model,
            note,
            methods_note,
            probe_note,
        );
    }

    println!();
    println!("Connection models:");
    for connection in &connection_models {
        println!("  {}", render_connection_model_inventory(connection));
    }

    println!();
    print_web_status(&config)?;

    println!();
    println!("Local model backends:");
    for backend in model_backends {
        println!(
            "  {} {} {}",
            backend.label,
            format_backend_status(&backend.status),
            backend_models_detail(&backend)
        );
    }

    let health_url = format!("http://{}:{}/", config.server.host, config.server.port);
    println!();
    println!(
        "Server running: {}",
        if server_healthy(&health_url).await {
            "yes"
        } else {
            "no"
        }
    );
    Ok(())
}

async fn prompt_for_connection(
    config: &BelltowerConfig,
    mode: ConnectionPromptMode,
) -> Result<ConnectionId> {
    let choices = connection_choices(config)
        .await?
        .into_iter()
        .filter(|choice| connection_prompt_allows(mode, choice))
        .collect::<Vec<_>>();
    if choices.is_empty() {
        return Err(BelltowerError::Config(match mode {
            ConnectionPromptMode::Runnable => {
                "no runnable connections are available; supported choices are local, openai, and anthropic"
                    .to_owned()
            }
            ConnectionPromptMode::Loginable => {
                "no login-capable connections are available".to_owned()
            }
            ConnectionPromptMode::Logout => {
                "no stored provider credentials are available to clear".to_owned()
            }
        }));
    }
    let recommended_index = choices
        .iter()
        .position(|choice| choice.recommended)
        .unwrap_or(0);
    println!("{}", connection_prompt_title(mode));
    for (index, choice) in choices.iter().enumerate() {
        let suffix = if choice.recommended {
            " (recommended)"
        } else {
            ""
        };
        println!(
            "  {}. {} [{}]{}",
            index + 1,
            choice.id,
            choice.status,
            suffix
        );
        println!(
            "     provider={} model={}",
            choice.provider, choice.default_model
        );
    }
    print!("Selection [{}]: ", recommended_index + 1);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(choices[recommended_index].id.clone());
    }
    if let Ok(index) = trimmed.parse::<usize>()
        && let Some(choice) = choices.get(index.saturating_sub(1))
    {
        return Ok(choice.id.clone());
    }
    choices
        .iter()
        .find(|choice| choice.id.0 == trimmed)
        .map(|choice| choice.id.clone())
        .ok_or_else(|| BelltowerError::Config(format!("unknown setup selection `{trimmed}`")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionPromptMode {
    Runnable,
    Loginable,
    Logout,
}

fn connection_prompt_title(mode: ConnectionPromptMode) -> &'static str {
    match mode {
        ConnectionPromptMode::Runnable => "Choose a default connection:",
        ConnectionPromptMode::Loginable => "Choose a provider to authenticate:",
        ConnectionPromptMode::Logout => "Choose a provider to clear from the auth store:",
    }
}

fn connection_prompt_allows(mode: ConnectionPromptMode, choice: &ConnectionChoice) -> bool {
    match mode {
        ConnectionPromptMode::Runnable => choice.supported,
        ConnectionPromptMode::Loginable => {
            choice.auth_required
                && (choice.api_key_login_supported || choice.device_code_login_supported)
        }
        ConnectionPromptMode::Logout => choice.auth_required,
    }
}

async fn connection_readiness(
    config: &BelltowerConfig,
    connection_id: &ConnectionId,
) -> Result<ConnectionReadinessInspection> {
    inspect_connection_status(config, connection_id).await
}

async fn launch_connection_readiness(
    config: &BelltowerConfig,
    connection_id: &ConnectionId,
    explicit_connection: bool,
) -> Result<ConnectionReadinessInspection> {
    if explicit_connection || connection_id == &ConnectionId::new("local") {
        connection_readiness(config, connection_id).await
    } else {
        inspect_connection_status_with_timeout(
            config,
            connection_id,
            IMPLICIT_REMOTE_READINESS_TIMEOUT,
        )
        .await
    }
}

fn connection_needs_auth_setup(connection: &ConnectionReadinessInspection) -> bool {
    matches!(connection.auth_state, bt_core::ConnectionAuthState::Missing)
        || matches!(
            connection.readiness_state,
            bt_core::ConnectionReadinessState::MissingAuth
        )
}

fn inventory_needs_auth_setup(connection: &ConnectionModelInventory) -> bool {
    matches!(connection.auth_state, bt_core::ConnectionAuthState::Missing)
        || matches!(
            connection.readiness_state,
            bt_core::ConnectionReadinessState::MissingAuth
        )
}

fn connection_needs_launch_setup(
    connection_id: &ConnectionId,
    connection: &ConnectionReadinessInspection,
) -> bool {
    if connection_id == &ConnectionId::new("local") {
        !connection.is_ready()
    } else {
        connection_needs_auth_setup(connection)
    }
}

fn should_fallback_to_local_connection(
    explicit_connection: bool,
    connection_id: &ConnectionId,
    readiness: &ConnectionReadinessInspection,
) -> bool {
    !explicit_connection && connection_id != &ConnectionId::new("local") && !readiness.is_ready()
}

async fn apply_local_fallback(
    config: &BelltowerConfig,
    explicit_connection: bool,
    connection_id: &mut ConnectionId,
    readiness: &mut ConnectionReadinessInspection,
    trace: &mut StartupTrace,
    notice_target: LaunchNoticeTarget,
) -> Result<()> {
    if !should_fallback_to_local_connection(explicit_connection, connection_id, readiness) {
        return Ok(());
    }
    let local_id = ConnectionId::new("local");
    if !config
        .connections
        .iter()
        .any(|connection| connection.id == local_id)
    {
        return Ok(());
    }

    trace.mark("readiness.local_fallback.start");
    let local_readiness = connection_readiness(config, &local_id).await?;
    trace.mark(format!(
        "readiness.local_fallback.done.{}",
        local_readiness.readiness_label()
    ));
    if local_readiness.is_ready() {
        notice_target.emit(format!(
            "Default connection `{}` is {}: {}. Falling back to `local`.",
            connection_id,
            readiness.readiness_label(),
            readiness.probe_status
        ));
        *connection_id = local_id;
        *readiness = local_readiness;
    }
    Ok(())
}

fn connection_not_ready_error(
    connection_id: &ConnectionId,
    connection: &ConnectionReadinessInspection,
) -> BelltowerError {
    BelltowerError::Config(format!(
        "connection `{connection_id}` is {}: {}",
        connection.readiness_label(),
        connection.probe_status
    ))
}

fn find_connection<'a>(
    config: &'a BelltowerConfig,
    connection_id: &ConnectionId,
) -> Result<&'a ConnectionDescriptor> {
    config
        .connections
        .iter()
        .find(|connection| &connection.id == connection_id)
        .ok_or_else(|| {
            BelltowerError::Config(format!("connection `{connection_id}` is not configured"))
        })
}

fn find_web_backend<'a>(
    config: &'a BelltowerConfig,
    backend_id: &str,
) -> Result<&'a bt_core::WebBackendConfig> {
    config
        .web
        .backends
        .iter()
        .find(|backend| backend.id.eq_ignore_ascii_case(backend_id.trim()))
        .ok_or_else(|| {
            BelltowerError::Config(format!("web backend `{backend_id}` is not configured"))
        })
}

fn print_web_status(config: &BelltowerConfig) -> Result<()> {
    let inspection = inspect_web_status(config)?;
    println!("Web retrieval");
    println!(
        "Search backend: {}",
        inspection.search_backend.as_deref().unwrap_or("auto")
    );
    println!("Fetch backend: {}", inspection.fetch_backend);
    println!("Backends:");
    for backend in &inspection.backends {
        print_check(
            backend.is_ready(),
            &format!("  {}", backend.backend_id),
            web_backend_detail(backend),
        );
    }
    Ok(())
}

fn web_backend_detail(backend: &bt_core::WebBackendReadinessInspection) -> String {
    let auth_detail = match &backend.auth_source {
        Some(source) => {
            if backend.auth_tried_sources.is_empty() {
                format!("; credential resolved from {source}")
            } else {
                format!(
                    "; credential resolved from {source} (tried: {})",
                    backend.auth_tried_sources.join(" -> ")
                )
            }
        }
        None => String::new(),
    };
    format!(
        "{}: {}{}",
        backend.readiness_label(),
        backend.probe_status,
        auth_detail
    )
}

fn ensure_connection_supported(connection: &ConnectionDescriptor) -> Result<()> {
    if connection_supported(connection) {
        Ok(())
    } else {
        unsupported_connection_error(connection)
    }
}

fn connection_supports_api_key_login(connection: &ConnectionDescriptor) -> bool {
    auth_methods_support_api_key_login(
        &connection.auth_methods,
        !connection.auth_sources.is_empty(),
    )
}

fn connection_supports_device_code_login(connection: &ConnectionDescriptor) -> bool {
    auth_methods_support_device_code_login(&connection.auth_methods)
}

fn auth_methods_support_api_key_login(
    auth_methods: &[bt_core::ConnectionAuthMethodDescriptor],
    auth_required: bool,
) -> bool {
    if auth_methods.is_empty() {
        return auth_required;
    }

    auth_methods
        .iter()
        .any(|method| method.kind == AuthMethodKind::ApiKey)
}

fn auth_methods_support_device_code_login(
    auth_methods: &[bt_core::ConnectionAuthMethodDescriptor],
) -> bool {
    auth_methods
        .iter()
        .any(|method| method.kind == AuthMethodKind::OAuthDeviceCode)
}

fn ensure_connection_supports_api_key_login(connection: &ConnectionDescriptor) -> Result<()> {
    if connection_supports_api_key_login(connection) {
        return Ok(());
    }

    let supported_auth = if connection.auth_methods.is_empty() {
        "its configured auth path".to_owned()
    } else {
        connection
            .auth_methods
            .iter()
            .map(|method| method.label.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(BelltowerError::Unsupported(format!(
        "connection `{}` does not support the current API-key login flow; supported auth methods: {}",
        connection.id, supported_auth
    )))
}

fn unsupported_connection_error(connection: &ConnectionDescriptor) -> Result<()> {
    Err(unsupported_connection_belltower_error(connection))
}

fn unsupported_connection_belltower_error(connection: &ConnectionDescriptor) -> BelltowerError {
    BelltowerError::Unsupported(format!(
        "connection `{}` uses provider `{}` which is not implemented in Belltower yet",
        connection.id, connection.provider
    ))
}

#[cfg(test)]
fn connection_support_label(connection: &ConnectionDescriptor) -> &'static str {
    if connection_supported(connection) {
        "runtime-supported"
    } else {
        "planned"
    }
}

fn resolve_secret_input(
    api_key: Option<String>,
    api_key_env: Option<String>,
    api_key_command: Option<String>,
) -> Result<Option<CredentialInput>> {
    if let Some(secret) = api_key {
        return Ok(Some(CredentialInput::Literal(secret.trim().to_owned())));
    }
    if let Some(env_var) = api_key_env {
        if env_var.trim().is_empty() {
            return Err(BelltowerError::Auth(
                "--api-key-env cannot be empty".to_owned(),
            ));
        }
        let _ = std::env::var(env_var.trim()).map_err(|_| {
            BelltowerError::Auth(format!(
                "environment variable `{}` is not set",
                env_var.trim()
            ))
        })?;
        return Ok(Some(CredentialInput::EnvReference(
            env_var.trim().to_owned(),
        )));
    }
    if let Some(command) = api_key_command {
        let command = command.trim();
        if command.is_empty() {
            return Err(BelltowerError::Auth(
                "--api-key-command cannot be empty".to_owned(),
            ));
        }
        return Ok(Some(CredentialInput::CommandReference(command.to_owned())));
    }
    Ok(None)
}

async fn resolve_model_selection(
    config: &BelltowerConfig,
    connection_id: &ConnectionId,
    requested: Option<String>,
    non_interactive: bool,
    preloaded_inventory: Option<&ConnectionModelInventory>,
) -> Result<String> {
    let connection = find_connection(config, connection_id)?;

    if connection_id == &ConnectionId::new("local") {
        let local_models = configured_local_models(config, connection).await?;
        if let Some(model) = requested {
            let trimmed = model.trim();
            if trimmed.is_empty() {
                return Err(BelltowerError::Config("model cannot be empty".to_owned()));
            }
            if !local_models.models.is_empty()
                && !local_models.models.iter().any(|name| name == trimmed)
            {
                let backend_label = local_models
                    .backend_label
                    .as_deref()
                    .unwrap_or("configured local backend");
                return Err(BelltowerError::Config(format!(
                    "model `{trimmed}` is not available on {backend_label}; choose one of: {}",
                    local_models.models.join(", ")
                )));
            }
            return Ok(trimmed.to_owned());
        }
        if local_models.models.is_empty() {
            println!(
                "No local models were detected. Keeping `{}` as the default model.",
                connection.default_model
            );
            return Ok(connection.default_model.clone());
        }

        let models = local_models.models;
        let recommended_index = models
            .iter()
            .position(|model| model == &connection.default_model)
            .unwrap_or(0);
        if non_interactive {
            let selected = models[recommended_index].clone();
            if selected != connection.default_model {
                println!(
                    "Configured local model `{}` is not available. Using `{selected}` instead.",
                    connection.default_model
                );
            }
            return Ok(selected);
        }
        println!();
        let backend_label = local_models
            .backend_label
            .as_deref()
            .unwrap_or("configured local backend");
        println!("Choose a default local model for {backend_label}:");
        for (index, model) in models.iter().enumerate() {
            let suffix = if index == recommended_index {
                " (recommended)"
            } else {
                ""
            };
            println!("  {}. {}{}", index + 1, model, suffix);
        }
        print!("Selection [{}]: ", recommended_index + 1);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(models[recommended_index].clone());
        }
        if let Ok(index) = trimmed.parse::<usize>()
            && let Some(model) = models.get(index.saturating_sub(1))
        {
            return Ok(model.clone());
        }
        return Ok(trimmed.to_owned());
    }

    let discoverable_models =
        discoverable_remote_model_choices(config, connection_id, preloaded_inventory).await?;

    if let Some(model) = requested {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return Err(BelltowerError::Config("model cannot be empty".to_owned()));
        }
        if let Some(choices) = &discoverable_models
            && !choices.iter().any(|candidate| candidate == trimmed)
        {
            return Err(BelltowerError::Config(format!(
                "model `{trimmed}` is not discoverable for `{connection_id}`; choose one of: {}",
                choices.join(", ")
            )));
        }
        return Ok(trimmed.to_owned());
    }

    if let Some(choices) = discoverable_models {
        let recommended = choices.first().cloned().ok_or_else(|| {
            BelltowerError::Config(format!(
                "no discoverable models are currently available for `{connection_id}`; authenticate and retry once model discovery succeeds"
            ))
        })?;

        if non_interactive {
            return Ok(recommended);
        }

        println!();
        println!("Choose a default model for `{connection_id}`:");
        for (index, model) in choices.iter().enumerate() {
            let suffix = if index == 0 { " (recommended)" } else { "" };
            println!("  {}. {}{}", index + 1, model, suffix);
        }
        print!("Selection [1]: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(recommended);
        }
        if let Ok(index) = trimmed.parse::<usize>()
            && let Some(model) = choices.get(index.saturating_sub(1))
        {
            return Ok(model.clone());
        }
        return Err(BelltowerError::Config(format!(
            "model `{trimmed}` is not discoverable for `{connection_id}`; choose one of: {}",
            choices.join(", ")
        )));
    }

    if non_interactive {
        return Ok(connection.default_model.clone());
    }

    println!();
    println!(
        "Default model for `{connection_id}` is `{}`.",
        connection.default_model
    );
    if !connection.model_fallbacks.is_empty() {
        println!(
            "Known alternatives: {}",
            connection.model_fallbacks.join(", ")
        );
    }
    print!("Model [{}]: ", connection.default_model);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(connection.default_model.clone())
    } else {
        Ok(trimmed.to_owned())
    }
}

async fn discoverable_remote_model_choices(
    config: &BelltowerConfig,
    connection_id: &ConnectionId,
    preloaded_inventory: Option<&ConnectionModelInventory>,
) -> Result<Option<Vec<String>>> {
    let inventory;
    let inventory = if let Some(preloaded_inventory) = preloaded_inventory {
        preloaded_inventory
    } else {
        inventory = inspect_connection_model_inventory(config, connection_id).await?;
        &inventory
    };

    if inventory.discovered_source.is_none() {
        return Ok(None);
    }

    Ok(Some(
        inventory
            .discoverable_models()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect(),
    ))
}

fn prompt_for_secret(connection_id: &ConnectionId) -> Result<String> {
    let prompt = format!("Enter API key for `{connection_id}`: ");
    let secret = rpassword::prompt_password(prompt)
        .map_err(|error| BelltowerError::Auth(error.to_string()))?;
    let secret = secret.trim().to_owned();
    if secret.is_empty() {
        return Err(BelltowerError::Auth("API key cannot be empty".to_owned()));
    }
    Ok(secret)
}

fn prompt_for_credential_input(connection_id: &ConnectionId) -> Result<CredentialInput> {
    println!();
    println!("How should Belltower authenticate `{connection_id}`?");
    println!("  1. Paste API key now (recommended)");
    println!("  2. Reference an environment variable");
    println!("  3. Reference a shell command");
    print!("Selection [1]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    match input.trim() {
        "" | "1" | "paste" => Ok(CredentialInput::Literal(prompt_for_secret(connection_id)?)),
        "2" | "env" => {
            let env_var = prompt_non_empty("Environment variable name: ")?;
            if std::env::var(&env_var).is_err() {
                println!(
                    "Warning: `{env_var}` is not set in the current shell. Belltower will store the reference anyway."
                );
            }
            Ok(CredentialInput::EnvReference(env_var))
        }
        "3" | "command" => {
            let command = prompt_non_empty("Shell command: ")?;
            Ok(CredentialInput::CommandReference(command))
        }
        other => Err(BelltowerError::Auth(format!(
            "unknown credential input mode `{other}`"
        ))),
    }
}

fn prompt_non_empty(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(BelltowerError::Auth("value cannot be empty".to_owned()));
    }
    Ok(trimmed.to_owned())
}

fn print_check(ok: bool, label: &str, detail: String) {
    let state = if ok { "ok" } else { "warn" };
    println!("[{state}] {label}: {detail}");
}

async fn connection_choices(config: &BelltowerConfig) -> Result<Vec<ConnectionChoice>> {
    let inspection = inspect_status(config).await?;
    let mut choices = Vec::new();
    for connection in inspection.connections {
        choices.push(ConnectionChoice {
            id: connection.connection_id.clone(),
            provider: connection.provider.clone(),
            default_model: connection.default_model.clone(),
            ready: connection.is_ready(),
            supported: connection.support_state == ConnectionSupportState::RuntimeSupported,
            auth_required: connection.auth_state != bt_core::ConnectionAuthState::NotRequired,
            api_key_login_supported: auth_methods_support_api_key_login(
                &connection.auth_methods,
                connection.auth_state != bt_core::ConnectionAuthState::NotRequired,
            ),
            device_code_login_supported: auth_methods_support_device_code_login(
                &connection.auth_methods,
            ),
            recommended: false,
            status: format!(
                "{}: {}",
                connection.readiness_label(),
                connection.probe_status
            ),
        });
    }

    if let Some(choice) = choices
        .iter_mut()
        .find(|choice| choice.id == config.defaults.default_connection)
    {
        choice.recommended = choice.ready && choice.supported;
    }
    if !choices.iter().any(|choice| choice.recommended) {
        if let Some(choice) = choices
            .iter_mut()
            .find(|choice| choice.ready && choice.supported)
        {
            choice.recommended = true;
        } else if let Some(choice) = choices.first_mut() {
            choice.recommended = true;
        }
    }
    Ok(choices)
}

fn find_connection_inspection<'a>(
    inspection: &'a bt_core::StatusInspection,
    connection_id: &ConnectionId,
) -> Result<&'a ConnectionReadinessInspection> {
    inspection
        .connections
        .iter()
        .find(|connection| &connection.connection_id == connection_id)
        .ok_or_else(|| {
            BelltowerError::Config(format!("connection `{connection_id}` is not configured"))
        })
}

fn doctor_connection_detail(connection: &ConnectionReadinessInspection) -> String {
    let auth_detail = match &connection.auth_source {
        Some(source) => {
            if connection.auth_tried_sources.is_empty() {
                format!("; credential resolved from {source}")
            } else {
                format!(
                    "; credential resolved from {source} (tried: {})",
                    connection.auth_tried_sources.join(" -> ")
                )
            }
        }
        None => String::new(),
    };
    format!(
        "{}: {}{}",
        connection.readiness_label(),
        connection.probe_status,
        auth_detail
    )
}

async fn configured_local_models(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
) -> Result<ConfiguredLocalModels> {
    let manager = LocalModelManager::from_config(config)?;
    let backends = manager.backends().await;
    let matching = backends
        .into_iter()
        .find(|backend| local_backend_matches(&backend.base_url, &connection.base_url));

    if let Some(backend) = matching {
        let models = if matches!(backend.status, ModelBackendStatus::Ready) {
            backend
                .available_models
                .into_iter()
                .filter(|model| !model.trim().is_empty())
                .collect()
        } else {
            Vec::new()
        };
        return Ok(ConfiguredLocalModels {
            backend_label: Some(backend.label),
            models,
        });
    }

    Ok(ConfiguredLocalModels {
        backend_label: None,
        models: Vec::new(),
    })
}

fn local_backend_matches(backend_base_url: &Url, connection_base_url: &Url) -> bool {
    normalize_local_base_url(backend_base_url) == normalize_local_base_url(connection_base_url)
}

fn normalize_local_base_url(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_query(None);
    normalized.set_fragment(None);
    let path = normalized.path().trim_end_matches('/').to_owned();
    let path = path.strip_suffix("/v1").unwrap_or(&path).to_owned();
    if path.is_empty() {
        normalized.set_path("/");
    } else {
        normalized.set_path(&path);
    }
    normalized.to_string().trim_end_matches('/').to_owned()
}

fn render_connection_model_inventory(connection: &bt_core::ConnectionModelInventory) -> String {
    let models = if connection.models.is_empty() {
        "models=0".to_owned()
    } else {
        format!(
            "models={}",
            connection
                .models
                .iter()
                .map(|model| match model.source {
                    bt_core::ConnectionModelSource::Default => {
                        format!("{} (default)", model.model_id)
                    }
                    bt_core::ConnectionModelSource::Fallback => {
                        format!("{} (fallback)", model.model_id)
                    }
                    bt_core::ConnectionModelSource::Discovered => {
                        format!("{} (discovered)", model.model_id)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let discovered = connection
        .discovered_source
        .as_deref()
        .map(|source| format!(" discovered_from={source}"))
        .unwrap_or_default();
    format!(
        "{} ({}) state={} ready={} probe={}{} {}",
        connection.connection_id,
        connection.provider,
        connection.readiness_label(),
        if connection.is_ready() { "yes" } else { "no" },
        connection.probe_status,
        discovered,
        models
    )
}

#[derive(Clone)]
struct ConnectionChoice {
    id: ConnectionId,
    provider: String,
    default_model: String,
    ready: bool,
    supported: bool,
    auth_required: bool,
    api_key_login_supported: bool,
    device_code_login_supported: bool,
    recommended: bool,
    status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConfiguredLocalModels {
    backend_label: Option<String>,
    models: Vec<String>,
}

#[cfg(test)]
mod tests;
