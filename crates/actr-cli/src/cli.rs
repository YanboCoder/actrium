//! CLI surface: `Cli`/`Commands` enum + unified dispatch.
//!
//! Guiding principles (see `crates/actr-cli/README.md` for the long form):
//!
//! - **High-frequency commands are flat**: `init`, `gen`, `build`, `run`,
//!   `ps`, `logs`, `start/stop/restart/rm`, `check`, `doc`.
//! - **Low-frequency / fine-grained operations are grouped**:
//!   `deps`, `pkg`, `registry`, `dlq`.
//! - **Meta commands** sit at the top level: `config`, `version`, `completion`.
//! - **Every subcommand implements [`crate::core::Command`]**; main dispatches
//!   through a single `cmd.execute(&ctx)` call and only builds a
//!   `ServiceContainer` when `cmd.required_components()` is non-empty.

use std::sync::Arc;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use owo_colors::OwoColorize;
use url::Url;

use crate::commands::discovery::StandaloneDiscoverConfig;
use crate::commands::{
    BuildCommand, CheckCommand, CompletionCommand, ConfigCommand, DepsArgs, DlqArgs, DocCommand,
    GenCommand, InitCommand, LogsCommand, PkgArgs, PsCommand, PublisherArgs, RealmArgs,
    RegistryArgs, RegistryCommand, RestartCommand, RmCommand, RunCommand, StartCommand,
    StopCommand, VersionCommand,
};
use crate::core::{
    ActrCliError, Command, CommandContext, CommandResult, ConfigManager, ConfigRequirement,
    ConsoleUI, ContainerBuilder, DefaultCacheManager, DefaultDependencyResolver,
    DefaultFingerprintValidator, DefaultNetworkValidator, DefaultProtoProcessor, DiscoveryContext,
    ErrorReporter, NetworkServiceDiscovery, ServiceContainer, TomlConfigManager,
};

/// Top-level `actr` CLI.
#[derive(Parser)]
#[command(name = "actr")]
#[command(
    about = "Actor-RTC Command Line Tool",
    long_about = "Actor-RTC Command Line Tool.\n\n\
        Commands are grouped by audience:\n  \
        development:  init / gen / build / check / doc\n  \
        runtime:      run / ps / logs / start / stop / restart / rm\n  \
        resources:    deps / pkg / registry / dlq / realm / publisher\n  \
        meta:         config / version / completion",
    version,
    disable_version_flag = true
)]
pub struct Cli {
    /// Verbosity level (currently unused; -v is reserved for future telemetry).
    #[arg(short, action = clap::ArgAction::Count, hide = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    // ── development (flat, high-frequency) ──
    /// Initialize a new Actor project
    Init(InitCommand),
    /// Generate code from proto files
    Gen(GenCommand),
    /// Build source artifact and package a signed .actr workload
    Build(BuildCommand),
    /// Validate project dependencies
    Check(CheckCommand),
    /// Generate project documentation
    Doc(DocCommand),

    // ── runtime (flat, docker-style) ──
    /// Run a packaged workload
    Run(RunCommand),
    /// List detached runtime instances
    Ps(PsCommand),
    /// Show logs for a detached runtime instance
    Logs(LogsCommand),
    /// Start a stopped detached runtime instance
    Start(StartCommand),
    /// Stop a detached runtime instance
    Stop(StopCommand),
    /// Restart a detached runtime instance
    Restart(RestartCommand),
    /// Remove a detached runtime instance record
    Rm(RmCommand),

    // ── resources (grouped) ──
    /// Local dependency management (install)
    Deps(DepsArgs),
    /// Local package operations (sign, verify, keygen)
    Pkg(PkgArgs),
    /// Remote Actor Registry (discover, fingerprint)
    Registry(RegistryArgs),
    /// Dead Letter Queue inspection and remediation
    Dlq(DlqArgs),
    /// Realm signing key holder's tool (keys, Realm management requests)
    Realm(RealmArgs),
    /// Publisher Profile keys and signed domain metadata
    Publisher(PublisherArgs),

    // ── meta ──
    /// Manage project configuration
    Config(ConfigCommand),
    /// Print version, git hash, and build date
    Version(VersionCommand),
    /// Generate shell completion script
    Completion(CompletionCommand),
}

impl Commands {
    /// Cast the parsed subcommand to its [`Command`] trait object.
    pub fn as_command(&self) -> &dyn Command {
        match self {
            Commands::Init(c) => c,
            Commands::Gen(c) => c,
            Commands::Build(c) => c,
            Commands::Check(c) => c,
            Commands::Doc(c) => c,
            Commands::Run(c) => c,
            Commands::Ps(c) => c,
            Commands::Logs(c) => c,
            Commands::Start(c) => c,
            Commands::Stop(c) => c,
            Commands::Restart(c) => c,
            Commands::Rm(c) => c,
            Commands::Deps(c) => c,
            Commands::Pkg(c) => c,
            Commands::Registry(c) => c,
            Commands::Dlq(c) => c,
            Commands::Realm(c) => c,
            Commands::Publisher(c) => c,
            Commands::Config(c) => c,
            Commands::Version(c) => c,
            Commands::Completion(c) => c,
        }
    }
}

/// Build the raw clap [`clap::Command`] for completion-script generation.
pub fn build_cli() -> clap::Command {
    Cli::command()
}

/// Entry point for `main.rs`.
pub async fn run() -> Result<()> {
    run_with(Cli::parse()).await
}

/// Run the CLI from an already parsed command line.
///
/// The binary parses arguments before installing its tracing subscriber so
/// runtime commands can initialize their own observability pipeline.
pub async fn run_with(cli: Cli) -> Result<()> {
    let Some(ref cmd_variant) = cli.command else {
        Cli::command().print_help()?;
        return Ok(());
    };

    // Extract standalone discover config early (before building container).
    let standalone_discover = extract_standalone_discover(cmd_variant);

    let command = cmd_variant.as_command();

    // Validate project-config requirements before touching the DI container,
    // so missing-configuration failures surface as actionable command errors
    // (identifying the missing file and current directory) rather than
    // internal "X not registered" DI invariant violations. Standalone
    // `registry discover` bypasses project config entirely.
    if standalone_discover.is_none() {
        if let Err(e) = validate_config_requirement(command.config_requirement()) {
            report_error(&e);
        }
    }

    let needs_container = !command.required_components().is_empty();
    let container = if needs_container {
        build_container(standalone_discover).await?
    } else {
        ContainerBuilder::new().build()?
    };

    let ctx = CommandContext {
        container: Arc::new(std::sync::Mutex::new(container)),
        args: crate::core::CommandArgs {
            command: String::new(),
            subcommand: None,
            flags: std::collections::HashMap::new(),
            positional: Vec::new(),
        },
        working_dir: std::env::current_dir()?,
    };

    match command.execute(&ctx).await {
        Ok(result) => {
            render_result(result);
            Ok(())
        }
        Err(e) => {
            report_error(&e);
        }
    }
}

/// If the user ran `actr registry discover` with standalone flags,
/// extract them before the container is built so we can register
/// ServiceDiscovery without needing a local `manifest.toml`.
fn extract_standalone_discover(cmd: &Commands) -> Option<StandaloneDiscoverConfig> {
    if let Commands::Registry(registry_args) = cmd {
        if let RegistryCommand::Discover(discover_cmd) = &registry_args.command {
            return discover_cmd.standalone_config();
        }
    }
    None
}

fn render_result(result: CommandResult) {
    match result {
        CommandResult::Success(msg) => {
            if !msg.is_empty() && msg != "Help displayed" {
                println!("{msg}");
            }
        }
        CommandResult::Install(install_result) => {
            println!("Installation complete: {}", install_result.summary());
        }
        CommandResult::Validation(report) => {
            let formatted = ErrorReporter::format_validation_report(&report);
            println!("{formatted}");
        }
        CommandResult::Generation(gen_result) => {
            println!("Generated {} files", gen_result.generated_files.len());
        }
        CommandResult::Error(error) => {
            eprintln!("{} {error}", "Error:".red());
            std::process::exit(1);
        }
    }
}

async fn build_container(
    standalone_discover: Option<StandaloneDiscoverConfig>,
) -> Result<ServiceContainer> {
    // ── Standalone mode (--endpoint --realm --realm-client-key) ──
    if let Some(cfg) = standalone_discover {
        let mut builder = ContainerBuilder::new();
        builder = builder.config_path(std::path::Path::new("."));

        let mut container = builder.build()?;
        container = container.register_user_interface(Arc::new(ConsoleUI::new()));

        let discovery_context = DiscoveryContext {
            package_actr_type: actr_protocol::ActrType {
                publisher: "cli".into(),
                name: "registry-discover".into(),
                version: "0.0.0".into(),
            },
            signaling_url: cfg.endpoint.clone(),
            issuer_endpoint: cfg.endpoint.to_string(),
            realm: Some(cfg.realm),
            realm_client_key: Some(cfg.realm_client_key),
        };

        container = container
            .register_service_discovery(Arc::new(NetworkServiceDiscovery::new(discovery_context)));
        return Ok(container);
    }

    // ── Project mode (needs manifest.toml or actr.toml in cwd) ──
    let manifest_path = std::path::Path::new("manifest.toml");
    let actr_path = std::path::Path::new("actr.toml");

    let config_path = if manifest_path.exists() {
        manifest_path
    } else if actr_path.exists() {
        actr_path
    } else {
        // Neither file exists — register minimal container;
        // ServiceDiscovery will fail validation with a clear error.
        return build_minimal_container().await;
    };

    let mut builder = ContainerBuilder::new();
    builder = builder.config_path(config_path);

    let mut container = builder.build()?;
    container = container.register_user_interface(Arc::new(ConsoleUI::new()));

    let manager = Arc::new(TomlConfigManager::new(config_path));
    container = container.register_config_manager(manager.clone());

    let mut container =
        container.register_dependency_resolver(Arc::new(DefaultDependencyResolver::new()));
    container = container.register_network_validator(Arc::new(DefaultNetworkValidator::new()));
    container =
        container.register_fingerprint_validator(Arc::new(DefaultFingerprintValidator::new()));
    container = container.register_proto_processor(Arc::new(DefaultProtoProcessor::new()));
    container = container.register_cache_manager(Arc::new(DefaultCacheManager::new()));

    let config = manager.load_config(config_path).await?;
    let effective_cli = crate::config::resolver::resolve_effective_cli_config().unwrap_or_default();

    let signaling_url = Url::parse(&effective_cli.network.signaling_url).map_err(|e| {
        anyhow::anyhow!(
            "Invalid network.signaling_url '{}': {}",
            effective_cli.network.signaling_url,
            e
        )
    })?;

    let issuer_endpoint = effective_cli.network.issuer_endpoint.clone();
    let realm = effective_cli.network.realm.clone();
    let realm_client_key = effective_cli.network.realm_client_key.clone();

    let discovery_context = DiscoveryContext {
        package_actr_type: config.package.actr_type.clone(),
        signaling_url,
        issuer_endpoint,
        realm,
        realm_client_key,
    };

    container = container
        .register_service_discovery(Arc::new(NetworkServiceDiscovery::new(discovery_context)));

    Ok(container)
}

/// Build a container with only shell-level components (UI),
/// for when neither manifest.toml nor actr.toml exists.
async fn build_minimal_container() -> Result<ServiceContainer> {
    let mut container = ContainerBuilder::new().build()?;
    container = container.register_user_interface(Arc::new(ConsoleUI::new()));
    Ok(container)
}

/// Report an error to stderr and exit.
///
/// [`ActrCliError`] variants are formatted via [`ErrorReporter`]; other errors
/// fall back to a debug dump. `OperationCancelled` exits 0. Shared by the
/// pre-container config check and the command execution error path so both
/// produce the same user-facing rendering.
fn report_error(e: &anyhow::Error) -> ! {
    if let Some(cli_error) = e.downcast_ref::<ActrCliError>() {
        if matches!(cli_error, ActrCliError::OperationCancelled) {
            std::process::exit(0);
        }
        eprintln!("{}", ErrorReporter::format_error(cli_error));
    } else {
        eprintln!("{} {e:?}", "Error:".red());
    }
    std::process::exit(1)
}

/// Validate that the current directory satisfies the command's project-config
/// requirement before the DI container is built.
///
/// On failure, returns a user-facing [`ActrCliError::InvalidProject`] that
/// identifies the missing file and the current directory, rather than letting
/// the container surface an internal "X not registered" invariant violation.
fn validate_config_requirement(req: ConfigRequirement) -> Result<()> {
    use std::path::Path;
    let cwd = std::env::current_dir()?;
    let manifest = Path::new("manifest.toml");
    let actr = Path::new("actr.toml");
    match req {
        ConfigRequirement::None => Ok(()),
        ConfigRequirement::RuntimeConfig => {
            if manifest.exists() || actr.exists() {
                Ok(())
            } else {
                Err(ActrCliError::InvalidProject {
                    message: format!(
                        "No project configuration found (manifest.toml or actr.toml) in {}.\n\
                         Run this command from a project directory, or use \
                         'actr registry discover --endpoint ... --realm ... \
                         --realm-client-key ...' for standalone discovery.",
                        cwd.display()
                    ),
                }
                .into())
            }
        }
        ConfigRequirement::WorkloadManifest => {
            if manifest.exists() {
                Ok(())
            } else if actr.exists() {
                Err(ActrCliError::InvalidProject {
                    message: format!(
                        "manifest.toml is required for this command, but only actr.toml was \
                         found in {}. Dependencies live in manifest.toml — run this command \
                         from a workload directory containing manifest.toml.",
                        cwd.display()
                    ),
                }
                .into())
            } else {
                Err(ActrCliError::InvalidProject {
                    message: format!(
                        "manifest.toml not found in {}. Run this command from a workload \
                         directory (one containing manifest.toml), or run 'actr init' to \
                         create one.",
                        cwd.display()
                    ),
                }
                .into())
            }
        }
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
