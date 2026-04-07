use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;

#[derive(Parser)]
#[command(name = "telepair", version, about = "Web terminal collaboration tool")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Enable the agent role (PTY management, virtual targets)
    #[arg(long, hide = true)]
    agent: bool,

    /// Enable the control role (auth, sessions, storage)
    #[arg(long, hide = true)]
    control: bool,

    /// Enable the gateway role (HTTP/WS endpoints)
    #[arg(long, hide = true)]
    gateway: bool,

    /// Server bind address
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// Server port
    #[arg(long, default_value_t = 7700)]
    port: u16,

    /// Path to config file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Path to targets config file
    #[arg(long)]
    targets: Option<PathBuf>,

    /// Path to web frontend dist directory
    #[arg(long)]
    web_dir: Option<PathBuf>,

    /// Allowed CORS origins (comma-separated). Unset defaults to
    /// loopback dev origins (http://localhost:5173, http://127.0.0.1:5173).
    /// Use absolute URLs only. Parse failures are fatal at startup.
    #[arg(long, value_delimiter = ',')]
    allowed_origins: Vec<String>,

    /// Allow any origin (equivalent to `Access-Control-Allow-Origin: *`).
    /// Only safe in dev or behind a reverse proxy that enforces CORS.
    /// Mutually exclusive with `--allowed-origins` (this flag wins).
    #[arg(long, default_value_t = false)]
    allow_any_origin: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Admin operations (token recovery, user management)
    Admin {
        #[command(subcommand)]
        cmd: AdminCommand,
    },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Print the saved admin token from ~/.telepair/admin_token
    ShowToken,
}

fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".telepair")
}

fn run_admin_command(cmd: AdminCommand) -> anyhow::Result<()> {
    match cmd {
        AdminCommand::ShowToken => {
            let token_file = data_dir().join("admin_token");
            if !token_file.exists() {
                anyhow::bail!(
                    "admin token file not found at {}\n\
                     The token is written once at first startup. If it was deleted, \
                     you'll need to remove ~/.telepair/telepair.db and let telepair \
                     re-create the admin user on next startup.",
                    token_file.display()
                );
            }
            let token = std::fs::read_to_string(&token_file)?;
            let token = token.trim();
            if token.is_empty() {
                anyhow::bail!("admin token file is empty: {}", token_file.display());
            }
            println!("{token}");
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Subcommands short-circuit the server startup path so we don't spin up
    // tracing / sqlite / axum just to print a single value.
    if let Some(command) = cli.command {
        return match command {
            Command::Admin { cmd } => run_admin_command(cmd),
        };
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // No flags = all roles enabled
    let (agent, control, gateway) = if !cli.agent && !cli.control && !cli.gateway {
        (true, true, true)
    } else {
        (cli.agent, cli.control, cli.gateway)
    };

    tracing::info!(
        agent = agent,
        control = control,
        gateway = gateway,
        "starting telepair"
    );

    // Ensure data directory exists
    let data_dir = data_dir();
    std::fs::create_dir_all(&data_dir)?;

    // Initialize storage (needed by control)
    let db_path = data_dir.join("telepair.db");
    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
    let storage = Arc::new(SqliteStorage::new(&db_url).await?);

    // Initialize target engine. Missing default `~/.telepair/targets.yaml`
    // is the fresh-install norm and stays silent; any other failure (parse
    // error, permission denied, explicit `--targets` path) warns so the
    // operator can see why their targets didn't load.
    let engine = match &cli.targets {
        Some(path) => TargetEngine::from_file(path).unwrap_or_else(|e| {
            tracing::warn!(
                "failed to load targets from {}: {e}, using defaults",
                path.display()
            );
            TargetEngine::empty()
        }),
        None => {
            let default_path = data_dir.join("targets.yaml");
            TargetEngine::from_file(&default_path).unwrap_or_else(|e| {
                let is_missing = e
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                if !is_missing {
                    tracing::warn!(
                        "failed to load targets from {}: {e}, using defaults",
                        default_path.display()
                    );
                }
                TargetEngine::empty()
            })
        }
    };

    // Close any sessions left "active" from a previous unclean shutdown
    match storage.close_stale_sessions().await {
        Ok(0) => {}
        Ok(n) => tracing::info!("closed {n} stale session(s) from previous run"),
        Err(e) => tracing::warn!("failed to close stale sessions: {e}"),
    }

    // Auto-create admin user on first run
    let auth = TokenAuthProvider::new(storage.clone());
    if storage.get_user_by_name("admin").await?.is_none() {
        let (_, token) = auth.setup_initial_admin("admin").await?;
        let token_file = data_dir.join("admin_token");
        std::fs::write(&token_file, &token)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600))?;
        }
        tracing::info!("=== First run: admin user created ===");
        tracing::info!("Admin token saved to: {}", token_file.display());
        eprintln!("Admin token: {token}");
        eprintln!("Save this token — it won't be shown again!");
    }

    if gateway {
        let web_dir = cli
            .web_dir
            .as_ref()
            .map(|p| {
                p.to_str()
                    .ok_or_else(|| anyhow::anyhow!("--web-dir path is not valid UTF-8"))
            })
            .transpose()?;
        let state = AppState::new(storage, engine).await;
        let cors_mode = if cli.allow_any_origin {
            telepair_gateway::CorsMode::AllowAny
        } else {
            telepair_gateway::CorsMode::Origins(cli.allowed_origins)
        };
        let router = telepair_gateway::build_router_with_options(state, web_dir, cors_mode)
            .map_err(|e| anyhow::anyhow!("CORS config error: {e}"))?;
        let addr = format!("{}:{}", cli.host, cli.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!("telepair listening on http://{addr}");
        if let Some(dir) = &cli.web_dir {
            tracing::info!("serving web frontend from {}", dir.display());
        }
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                tokio::signal::ctrl_c().await.ok();
                tracing::info!("shutting down gracefully...");
            })
            .await?;
    } else {
        tracing::info!("no gateway role — running headless");
        // In a future cluster mode, agent/control-only instances would
        // connect to a remote gateway here. For now, just wait.
        tokio::signal::ctrl_c().await?;
    }

    Ok(())
}
