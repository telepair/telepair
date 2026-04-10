use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use chrono::{DateTime, Duration, Utc};

use telepair_agent::virtual_target::TargetEngine;
use telepair_control::session_service::SessionService;
use telepair_core::audit::{AuditEvent, AuditEventType, AuditFilter, AuditSink};
use telepair_core::auth::TokenAuthProvider;
use telepair_core::session::CloseReason;
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
    /// Query the append-only audit log
    Audit(AuditArgs),
}

#[derive(clap::Args, Debug)]
struct AuditArgs {
    /// Time window ending now, e.g. `30m`, `2h`, `7d`. Default: 24h.
    /// Mutually exclusive with `--since` / `--until`; specifying both
    /// `--last` and either side of the absolute window is a parse error.
    #[arg(long)]
    last: Option<String>,

    /// Inclusive lower bound, RFC 3339 (e.g. `2026-04-09T00:00:00Z`).
    #[arg(long, conflicts_with = "last")]
    since: Option<DateTime<Utc>>,

    /// Exclusive upper bound, RFC 3339. Defaults to now if omitted.
    #[arg(long, conflicts_with = "last")]
    until: Option<DateTime<Utc>>,

    /// Filter to rows touching this session id.
    #[arg(long)]
    session: Option<String>,

    /// Filter to rows whose actor matches. Accepts a user name (resolved
    /// via storage) or a raw UUID. Unknown names are a hard error rather
    /// than returning zero rows — much easier to debug.
    #[arg(long)]
    actor: Option<String>,

    /// Filter to specific event types. Repeat for OR semantics, e.g.
    /// `--type session.created --type session.closed`.
    #[arg(long = "type", value_name = "EVENT_TYPE")]
    event_types: Vec<String>,

    /// Maximum number of rows. Default 100 (sane humans, sane tables).
    #[arg(long, default_value_t = 100)]
    limit: i64,

    /// Output format.
    #[arg(long, value_enum, default_value_t = AuditFormat::Table)]
    format: AuditFormat,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum AuditFormat {
    Table,
    Json,
}

/// Parse `30m`, `2h`, `7d`, `45s` into a positive chrono::Duration.
/// Intentionally strict: whitespace, plus signs, compound forms like
/// `1h30m` all rejected — the error message points the operator at the
/// exact accepted shape instead of silently interpreting `1h30m` as
/// `1h`.
fn parse_last_window(s: &str) -> anyhow::Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("--last value is empty; expected e.g. 1h, 24h, 7d");
    }
    let split_at = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| anyhow::anyhow!("--last value '{s}' missing unit (s|m|h|d)"))?;
    let (num_str, unit) = s.split_at(split_at);
    let n: i64 = num_str
        .parse()
        .map_err(|_| anyhow::anyhow!("--last numeric prefix must be a positive integer: {s}"))?;
    if n <= 0 {
        anyhow::bail!("--last value must be positive: {s}");
    }
    let dur = match unit {
        "s" => Duration::seconds(n),
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        other => {
            anyhow::bail!("--last unit must be one of s|m|h|d, got '{other}'");
        }
    };
    Ok(dur)
}

fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".telepair")
}

async fn run_admin_command(cmd: AdminCommand) -> anyhow::Result<()> {
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
        AdminCommand::Audit(args) => run_audit_command(args).await,
    }
}

async fn run_audit_command(args: AuditArgs) -> anyhow::Result<()> {
    // Open the DB in the same shape the server does so the CLI sees
    // exactly the rows the running process writes. Use `mode=rwc` so
    // operators running `telepair admin audit` against a cold machine
    // get a clear "nothing happened yet" instead of a connection error;
    // `mode=rwc` fails if the parent directory is missing so we mkdir
    // it ourselves, same as the server startup path.
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let db_path = dir.join("telepair.db");
    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
    let storage = Arc::new(SqliteStorage::new(&db_url).await?);

    // Resolve time bounds. `--last` wins and collapses into a (since, now)
    // pair; otherwise trust `--since`/`--until` verbatim; otherwise default
    // to the last 24h so a flag-free invocation returns something useful.
    let now = Utc::now();
    let (since, until) = if let Some(last) = args.last.as_deref() {
        let window = parse_last_window(last)?;
        (Some(now - window), Some(now))
    } else if args.since.is_some() || args.until.is_some() {
        (args.since, args.until)
    } else {
        (Some(now - Duration::hours(24)), Some(now))
    };

    // Resolve actor by name if it doesn't parse as a UUID. Unknown
    // names are a hard error — otherwise a typo silently returns zero
    // rows and the operator thinks nothing happened.
    let actor_id = if let Some(ref raw) = args.actor {
        if let Ok(uuid) = uuid::Uuid::parse_str(raw) {
            Some(uuid)
        } else {
            let user = storage.get_user_by_name(raw).await?.ok_or_else(|| {
                anyhow::anyhow!("--actor '{raw}' is not a UUID and no user with that name exists")
            })?;
            Some(user.id)
        }
    } else {
        None
    };

    // Event type strings → enum; any typo is a hard error for the same
    // reason as --actor. We accept the dotted form (session.created)
    // since that's what the table header prints, and error listing the
    // full canonical set on mismatch.
    let mut event_types = Vec::with_capacity(args.event_types.len());
    for raw in &args.event_types {
        event_types.push(raw.parse::<AuditEventType>().map_err(|_| {
            anyhow::anyhow!(
                "--type '{raw}' unknown; valid types: session.created, session.closed, \
                 participant.joined, invite.minted, invite.redeemed, invite.revoked, \
                 target.access_denied"
            )
        })?);
    }

    let filter = AuditFilter {
        since,
        until,
        actor_id,
        session_id: args.session.clone(),
        event_types,
        limit: Some(args.limit),
        offset: 0,
    };

    let audit = AuditSink::new(storage);
    let rows = audit.query(filter).await?;

    match args.format {
        AuditFormat::Json => print_audit_json(&rows)?,
        AuditFormat::Table => print_audit_table(&rows),
    }
    Ok(())
}

fn print_audit_json(rows: &[AuditEvent]) -> anyhow::Result<()> {
    // Array form, not NDJSON — this is a one-shot human/CI read path,
    // not a streaming firehose. `serde_json::to_writer_pretty` onto
    // stdout keeps newlines where humans expect them and still pipes
    // into `jq` cleanly.
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer_pretty(&mut out, rows)?;
    use std::io::Write;
    writeln!(out)?;
    Ok(())
}

fn print_audit_table(rows: &[AuditEvent]) {
    if rows.is_empty() {
        println!("no audit events matched the filter");
        return;
    }

    // Fixed column widths keep the output stable across runs so diffs
    // and grep are meaningful. Actor/session/detail get truncated at
    // the width rather than wrapping — a terminal that's too narrow
    // just trims the right edge, same as `ps`.
    const TS_W: usize = 20;
    const EVT_W: usize = 22;
    const ACTOR_W: usize = 18;
    const SES_W: usize = 14;
    let header = format!(
        "{:<TS_W$}  {:<EVT_W$}  {:<ACTOR_W$}  {:<SES_W$}  detail",
        "timestamp", "event_type", "actor", "session",
    );
    println!("{header}");
    println!("{}", "-".repeat(header.len() + 20));
    for row in rows {
        let ts = row.ts.format("%Y-%m-%d %H:%M:%S").to_string();
        let actor = row.actor_name.as_deref().unwrap_or("-");
        let session = row.session_id.as_deref().unwrap_or("-");
        let detail = if row.detail.is_null() {
            "-".to_string()
        } else {
            row.detail.to_string()
        };
        println!(
            "{:<TS_W$}  {:<EVT_W$}  {:<ACTOR_W$}  {:<SES_W$}  {}",
            truncate(&ts, TS_W),
            truncate(row.event_type.as_str(), EVT_W),
            truncate(actor, ACTOR_W),
            truncate(session, SES_W),
            detail,
        );
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Subcommands short-circuit the server startup path so we don't spin up
    // tracing / sqlite / axum just to print a single value. `admin audit`
    // opens its own storage handle inside the handler — the server
    // startup path stays untouched.
    if let Some(command) = cli.command {
        return match command {
            Command::Admin { cmd } => run_admin_command(cmd).await,
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
    //
    // Resolve the effective path first (explicit `--targets` or the
    // default under the data dir) and remember it regardless of
    // whether this initial load succeeds. That's what lets the admin
    // `POST /api/admin/targets/reload` endpoint re-attempt a file
    // the operator fixed after startup — otherwise the only way to
    // recover from a yaml typo at boot would be a full restart.
    let targets_path: Option<PathBuf> = match &cli.targets {
        Some(path) => Some(path.clone()),
        None => Some(data_dir.join("targets.yaml")),
    };
    let engine = match &targets_path {
        Some(path) => TargetEngine::from_file(path).unwrap_or_else(|e| {
            let is_missing = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
            // An explicit `--targets` path that doesn't exist is a
            // loud operator mistake; the default path missing on a
            // fresh install is the norm and stays silent.
            if cli.targets.is_some() || !is_missing {
                tracing::warn!(
                    "failed to load targets from {}: {e}, using defaults",
                    path.display()
                );
            }
            TargetEngine::empty()
        }),
        None => TargetEngine::empty(),
    };

    // Close any sessions left "active" from a previous unclean shutdown.
    // These rows get `CloseReason::Startup` so the history view can
    // distinguish "the server came back up and reclaimed these" from
    // "the owner clicked Close" or "the reaper timed them out". Route
    // through SessionService (not raw storage) so the bulk audit row
    // gets emitted — the history timeline would otherwise have a gap
    // every time the server restarted.
    {
        let audit = Arc::new(AuditSink::new(storage.clone()));
        let sweep_sessions = SessionService::new(storage.clone(), audit);
        match sweep_sessions
            .close_stale_sessions(CloseReason::Startup)
            .await
        {
            Ok(0) => {}
            Ok(n) => tracing::info!("closed {n} stale session(s) from previous run"),
            Err(e) => tracing::warn!("failed to close stale sessions: {e}"),
        }
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
        let state = AppState::new(storage, engine, targets_path).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_last_window_accepts_units() {
        assert_eq!(parse_last_window("30s").unwrap(), Duration::seconds(30));
        assert_eq!(parse_last_window("45m").unwrap(), Duration::minutes(45));
        assert_eq!(parse_last_window("24h").unwrap(), Duration::hours(24));
        assert_eq!(parse_last_window("7d").unwrap(), Duration::days(7));
    }

    #[test]
    fn parse_last_window_rejects_garbage() {
        // Empty, missing unit, unknown unit, compound form, zero/negative
        // — all of these must error rather than silently reinterpret.
        assert!(parse_last_window("").is_err());
        assert!(parse_last_window("42").is_err());
        assert!(parse_last_window("42y").is_err());
        assert!(parse_last_window("1h30m").is_err());
        assert!(parse_last_window("0h").is_err());
        assert!(parse_last_window("-5h").is_err());
        assert!(parse_last_window("abc").is_err());
    }

    #[test]
    fn parse_last_window_trims_whitespace() {
        assert_eq!(parse_last_window("  2h  ").unwrap(), Duration::hours(2));
    }
}
