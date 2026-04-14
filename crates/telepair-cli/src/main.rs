#![deny(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use chrono::{DateTime, Duration, Utc};

use telepair_agent::virtual_target::TargetEngine;
use telepair_control::auth_service::{AuthService, SmtpConfig};
use telepair_control::session_service::SessionService;
use telepair_core::audit::{AuditEvent, AuditEventType, AuditFilter, AuditSink};
use telepair_core::auth::TokenAuthProvider;
use telepair_core::session::{CloseReason, User};
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

    /// SMTP server hostname for email verification (enables registration)
    #[arg(long, env = "TELEPAIR_SMTP_HOST")]
    smtp_host: Option<String>,

    /// SMTP port [default: 587]
    #[arg(long, env = "TELEPAIR_SMTP_PORT", default_value_t = 587)]
    smtp_port: u16,

    /// SMTP username
    #[arg(long, env = "TELEPAIR_SMTP_USER")]
    smtp_user: Option<String>,

    /// SMTP password
    #[arg(long, env = "TELEPAIR_SMTP_PASS")]
    smtp_pass: Option<String>,

    /// SMTP sender address, e.g. "Telepair <noreply@example.com>"
    #[arg(long, env = "TELEPAIR_SMTP_FROM")]
    smtp_from: Option<String>,

    /// Trust the `X-Forwarded-For` / `X-Real-IP` headers when keying
    /// the per-IP register rate limiter. Set this ONLY when telepair
    /// is behind a reverse proxy that rewrites those headers on every
    /// inbound request (the documented nginx deployment). With it
    /// enabled in a direct-to-internet setup, any client can forge
    /// the header and bypass the throttle. Off by default, which is
    /// the safe fail-closed.
    #[arg(
        long,
        env = "TELEPAIR_TRUST_FORWARDED_HEADERS",
        default_value_t = false
    )]
    trust_forwarded_headers: bool,
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
    /// Manage password-login user accounts (create / list / enable / disable).
    /// Useful on single-node installs without SMTP, where self-served
    /// registration is unavailable.
    Users {
        #[command(subcommand)]
        cmd: UsersCommand,
    },
}

#[derive(Subcommand)]
enum UsersCommand {
    /// Provision a new user directly, bypassing the OTP flow. The account
    /// is created as `approved` so the user can log in immediately. Prints
    /// the freshly minted bearer token to stderr; if no password is
    /// supplied a random one is generated and printed too.
    Create(CreateUserArgs),
    /// List non-guest accounts (newest first).
    List,
    /// Flip `session_enabled = TRUE` on the target account. Accepts the
    /// user's email (case-insensitive) or raw UUID.
    Enable {
        /// User email or UUID.
        user: String,
    },
    /// Flip `session_enabled = FALSE` on the target account. Accepts the
    /// user's email (case-insensitive) or raw UUID.
    Disable {
        /// User email or UUID.
        user: String,
    },
}

#[derive(clap::Args, Debug)]
struct CreateUserArgs {
    /// Email address (login identifier). Lowercased before insert.
    #[arg(long)]
    email: String,

    /// Display name. Defaults to the email's local part.
    #[arg(long)]
    name: Option<String>,

    /// Read the password from this file (one line, trailing whitespace
    /// stripped). Mutually exclusive with `--password`; if neither is set
    /// a random 16-character password is generated and printed once.
    #[arg(long, conflicts_with = "password")]
    password_file: Option<PathBuf>,

    /// Password supplied inline. Prefer `--password-file` to keep the
    /// value out of shell history; this flag exists for scripted runs
    /// where the caller has already secured the input channel.
    #[arg(long)]
    password: Option<String>,

    /// Mark the account as an admin. Defaults to false — admins should be
    /// a deliberate, rare choice.
    #[arg(long, default_value_t = false)]
    admin: bool,

    /// Create the account with `session_enabled = FALSE`. Useful for
    /// pre-provisioning an account an operator will enable later.
    #[arg(long, default_value_t = false)]
    no_session: bool,
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

async fn run_admin_command(cmd: AdminCommand, data_dir: &std::path::Path) -> anyhow::Result<()> {
    match cmd {
        AdminCommand::ShowToken => {
            let token_file = data_dir.join("admin_token");
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
        AdminCommand::Audit(args) => run_audit_command(args, data_dir).await,
        AdminCommand::Users { cmd } => run_users_command(cmd, data_dir).await,
    }
}

async fn open_storage(data_dir: &std::path::Path) -> anyhow::Result<Arc<SqliteStorage>> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("telepair.db");
    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
    Ok(Arc::new(SqliteStorage::new(&db_url).await?))
}

/// Resolve a `--user` CLI argument (email or UUID) to a concrete
/// [`User`] row. Unknown values are a hard error — otherwise a typo
/// silently returns zero rows and the operator thinks nothing happened.
async fn resolve_user(storage: &SqliteStorage, raw: &str) -> anyhow::Result<User> {
    if let Ok(uuid) = uuid::Uuid::parse_str(raw) {
        return storage
            .find_user_by_id(uuid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no user with id {uuid}"));
    }
    let email = raw.to_lowercase();
    storage
        .get_user_by_email(&email)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no user with email '{raw}' (or raw UUID)"))
}

async fn run_users_command(cmd: UsersCommand, data_dir: &std::path::Path) -> anyhow::Result<()> {
    let storage = open_storage(data_dir).await?;
    match cmd {
        UsersCommand::Create(args) => run_users_create(args, storage).await,
        UsersCommand::List => run_users_list(storage).await,
        UsersCommand::Enable { user } => run_users_set_enabled(user, true, storage).await,
        UsersCommand::Disable { user } => run_users_set_enabled(user, false, storage).await,
    }
}

async fn run_users_create(args: CreateUserArgs, storage: Arc<SqliteStorage>) -> anyhow::Result<()> {
    let email = args.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        anyhow::bail!("--email must be a non-empty address containing '@'");
    }

    let display_name = match args.name {
        Some(n) => {
            let n = n.trim().to_owned();
            if n.is_empty() {
                anyhow::bail!("--name, if provided, must be non-empty");
            }
            n
        }
        None => email
            .split('@')
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_owned())
            .ok_or_else(|| anyhow::anyhow!("cannot derive display name from email '{email}'"))?,
    };

    // Password resolution: --password-file → file, --password → inline,
    // otherwise generate a 16-char nanoid. The generated case mirrors the
    // admin-token bootstrap: print once to stderr so the operator can
    // hand it off, and never persist it.
    let (password, generated) = match (args.password_file, args.password) {
        (Some(path), _) => {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("read --password-file {}: {e}", path.display()))?;
            let trimmed = raw.trim_end_matches(['\n', '\r']).to_owned();
            if trimmed.is_empty() {
                anyhow::bail!("--password-file {} is empty", path.display());
            }
            (trimmed, false)
        }
        (None, Some(p)) => {
            if p.is_empty() {
                anyhow::bail!("--password must be non-empty");
            }
            (p, false)
        }
        (None, None) => (nanoid::nanoid!(16), true),
    };

    let audit = Arc::new(AuditSink::new(storage.clone()));
    let auth = AuthService::new(storage.clone(), None, audit);
    let session_enabled = !args.no_session;
    let (user, token) = auth
        .admin_create_user(
            &email,
            &display_name,
            &password,
            args.admin,
            session_enabled,
        )
        .await?;

    println!("Created user:");
    println!("  id            : {}", user.id);
    println!("  name          : {}", user.name);
    println!("  email         : {}", user.email.as_deref().unwrap_or("-"));
    println!("  admin         : {}", user.is_admin);
    println!("  session_enabled: {}", user.session_enabled);

    eprintln!();
    eprintln!("Bearer token (save — it will not be shown again):");
    eprintln!("  {token}");
    if generated {
        eprintln!();
        eprintln!("Generated password (save — it will not be shown again):");
        eprintln!("  {password}");
    }
    Ok(())
}

async fn run_users_list(storage: Arc<SqliteStorage>) -> anyhow::Result<()> {
    let users = storage.list_accounts().await?;
    if users.is_empty() {
        println!("no accounts (not counting invite-minted scoped guests)");
        return Ok(());
    }
    const NAME_W: usize = 20;
    const EMAIL_W: usize = 28;
    println!(
        "{:<NAME_W$}  {:<EMAIL_W$}  {:<5}  {:<8}  state",
        "name", "email", "admin", "session",
    );
    for u in users {
        println!(
            "{:<NAME_W$}  {:<EMAIL_W$}  {:<5}  {:<8}  {}",
            truncate(&u.name, NAME_W),
            truncate(u.email.as_deref().unwrap_or("-"), EMAIL_W),
            if u.is_admin { "yes" } else { "no" },
            if u.session_enabled {
                "enabled"
            } else {
                "disabled"
            },
            u.approval_state.as_str(),
        );
    }
    Ok(())
}

async fn run_users_set_enabled(
    raw: String,
    enabled: bool,
    storage: Arc<SqliteStorage>,
) -> anyhow::Result<()> {
    let user = resolve_user(&storage, &raw).await?;
    if user.session_enabled == enabled {
        println!(
            "user {} already {} — no change",
            user.name,
            if enabled { "enabled" } else { "disabled" }
        );
        return Ok(());
    }
    let audit = Arc::new(AuditSink::new(storage.clone()));
    let auth = AuthService::new(storage.clone(), None, audit);
    // CLI has no authenticated admin identity; reuse the user's own row
    // as actor so the audit trail at least names the target. The bulk
    // "who ran the CLI" context lives in the shell history.
    let updated = auth
        .set_session_access(user.id, &user.name, user.id, enabled)
        .await?;
    println!(
        "user {} -> session_enabled = {}",
        updated.name, updated.session_enabled
    );
    Ok(())
}

async fn run_audit_command(args: AuditArgs, data_dir: &std::path::Path) -> anyhow::Result<()> {
    // Open the DB in the same shape the server does so the CLI sees
    // exactly the rows the running process writes. Use `mode=rwc` so
    // operators running `telepair admin audit` against a cold machine
    // get a clear "nothing happened yet" instead of a connection error;
    // `mode=rwc` fails if the parent directory is missing so we mkdir
    // it ourselves, same as the server startup path.
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("telepair.db");
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
            Command::Admin { cmd } => run_admin_command(cmd, &data_dir()).await,
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
            let is_missing = matches!(
                &e,
                telepair_core::error::Error::Io(io)
                    if io.kind() == std::io::ErrorKind::NotFound
            );
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

    // Sweep abandoned pending registrations older than 24h. These are
    // signups that started but were never completed — the OTP expired
    // and the user never came back. The pending row carries no
    // authority (no `users` entry, no token), so dropping it is purely
    // hygiene. Running at startup keeps the table tidy without
    // requiring a separate cron job.
    {
        let cutoff = Utc::now() - Duration::hours(24);
        match storage.sweep_pending_registrations(cutoff).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("swept {n} abandoned pending registration(s) older than 24h"),
            Err(e) => tracing::warn!("failed to sweep pending registrations: {e}"),
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

    // Build SMTP config if the host flag was provided. Missing host = no
    // SMTP = registration returns 503. The other flags are optional so
    // operators can mix CLI and env vars.
    let smtp = cli.smtp_host.map(|host| {
        Arc::new(SmtpConfig {
            host,
            port: cli.smtp_port,
            username: cli.smtp_user.unwrap_or_default(),
            password: cli.smtp_pass.unwrap_or_default(),
            from: cli
                .smtp_from
                .unwrap_or_else(|| "Telepair <noreply@localhost>".into()),
        })
    });

    if gateway {
        let web_dir = cli
            .web_dir
            .as_ref()
            .map(|p| {
                p.to_str()
                    .ok_or_else(|| anyhow::anyhow!("--web-dir path is not valid UTF-8"))
            })
            .transpose()?;
        let mut state = AppState::new(storage, engine, targets_path, smtp, data_dir.clone()).await;
        state.trust_forwarded_headers = cli.trust_forwarded_headers;
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
        // `into_make_service_with_connect_info::<SocketAddr>` is what
        // makes the `ConnectInfo` extractor in `http::register`
        // actually populate — without it the per-IP rate limiter
        // silently degrades to "no limit" because every call sees
        // `ConnectInfo = None`. Keep this wired even if the register
        // surface is later moved behind a reverse proxy: the proxy
        // is then the addr, which is still a useful shape of IP to
        // throttle (better than global no-op).
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
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

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("abcdefghij", 5);
        assert_eq!(result.chars().count(), 5);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("abc", 3), "abc");
    }

    fn audit_args() -> AuditArgs {
        AuditArgs {
            last: Some("1h".into()),
            since: None,
            until: None,
            session: None,
            actor: None,
            event_types: vec![],
            limit: 5,
            format: AuditFormat::Table,
        }
    }

    #[tokio::test]
    async fn show_token_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_admin_command(AdminCommand::ShowToken, dir.path()).await;
        let err = result.expect_err("missing admin_token must error");
        assert!(
            err.to_string().contains("admin token file not found"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn show_token_empty_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("admin_token"), "   \n").unwrap();
        let err = run_admin_command(AdminCommand::ShowToken, dir.path())
            .await
            .expect_err("empty admin_token must error");
        assert!(err.to_string().contains("empty"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn audit_command_with_bad_actor_name_errors() {
        let dir = tempfile::tempdir().unwrap();
        let args = AuditArgs {
            actor: Some("nonexistent-user-xyz-12345".into()),
            ..audit_args()
        };
        let err = run_audit_command(args, dir.path())
            .await
            .expect_err("unknown actor must error");
        let msg = err.to_string();
        assert!(
            msg.contains("nonexistent-user-xyz-12345"),
            "error must mention the bad actor name, got: {msg}"
        );
    }

    #[tokio::test]
    async fn audit_command_with_bad_event_type_errors() {
        let dir = tempfile::tempdir().unwrap();
        let args = AuditArgs {
            event_types: vec!["invalid.event.type".into()],
            ..audit_args()
        };
        let err = run_audit_command(args, dir.path())
            .await
            .expect_err("unknown event type must error");
        let msg = err.to_string();
        assert!(
            msg.contains("invalid.event.type"),
            "error must mention the bad event type, got: {msg}"
        );
    }

    #[tokio::test]
    async fn audit_command_json_format_on_empty_db_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let args = AuditArgs {
            format: AuditFormat::Json,
            ..audit_args()
        };
        run_audit_command(args, dir.path())
            .await
            .expect("empty isolated DB must succeed");
    }

    #[tokio::test]
    async fn audit_command_table_format_on_empty_db_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        run_audit_command(audit_args(), dir.path())
            .await
            .expect("empty isolated DB must succeed");
    }

    // ── Admin users CLI ──────────────────────────────────────────────

    fn create_args(email: &str, password: Option<&str>) -> CreateUserArgs {
        CreateUserArgs {
            email: email.into(),
            name: None,
            password_file: None,
            password: password.map(|s| s.into()),
            admin: false,
            no_session: false,
        }
    }

    #[tokio::test]
    async fn users_create_persists_account_and_allows_listing() {
        let dir = tempfile::tempdir().unwrap();
        run_users_command(
            UsersCommand::Create(create_args("dev@example.com", Some("hunter2a"))),
            dir.path(),
        )
        .await
        .expect("create must succeed");

        // Listing should surface the freshly-created account.
        run_users_command(UsersCommand::List, dir.path())
            .await
            .expect("list must succeed");

        // Second attempt with the same email is a conflict.
        let err = run_users_command(
            UsersCommand::Create(create_args("dev@example.com", Some("hunter2a"))),
            dir.path(),
        )
        .await
        .expect_err("duplicate email must error");
        assert!(
            err.to_string().to_lowercase().contains("already exists"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn users_create_rejects_invalid_email() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_users_command(
            UsersCommand::Create(create_args("not-an-email", Some("hunter2a"))),
            dir.path(),
        )
        .await
        .expect_err("missing '@' must error");
        assert!(err.to_string().contains("@"), "got: {err}");
    }

    #[tokio::test]
    async fn users_create_uses_password_file_when_supplied() {
        let dir = tempfile::tempdir().unwrap();
        let pw_path = dir.path().join("pw.txt");
        std::fs::write(&pw_path, "file-pass-1\n").unwrap();
        let args = CreateUserArgs {
            email: "filed@example.com".into(),
            name: Some("filed".into()),
            password_file: Some(pw_path),
            password: None,
            admin: false,
            no_session: false,
        };
        run_users_command(UsersCommand::Create(args), dir.path())
            .await
            .expect("password-file path must succeed");

        // Trimmed password should authenticate via AuthService::login.
        let storage = open_storage(dir.path()).await.unwrap();
        let audit = Arc::new(AuditSink::new(storage.clone()));
        let svc = AuthService::new(storage, None, audit);
        svc.login("filed@example.com", "file-pass-1")
            .await
            .expect("login with the file-sourced password");
    }

    #[tokio::test]
    async fn users_create_empty_password_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let pw_path = dir.path().join("empty.txt");
        std::fs::write(&pw_path, "\n").unwrap();
        let args = CreateUserArgs {
            email: "e@example.com".into(),
            name: None,
            password_file: Some(pw_path),
            password: None,
            admin: false,
            no_session: false,
        };
        let err = run_users_command(UsersCommand::Create(args), dir.path())
            .await
            .expect_err("empty password-file must error");
        assert!(err.to_string().contains("empty"), "got: {err}");
    }

    #[tokio::test]
    async fn users_enable_disable_flips_bit() {
        let dir = tempfile::tempdir().unwrap();
        run_users_command(
            UsersCommand::Create(create_args("toggle@example.com", Some("hunter2a"))),
            dir.path(),
        )
        .await
        .unwrap();

        run_users_command(
            UsersCommand::Disable {
                user: "toggle@example.com".into(),
            },
            dir.path(),
        )
        .await
        .expect("disable must succeed");

        let storage = open_storage(dir.path()).await.unwrap();
        let user = storage
            .get_user_by_email("toggle@example.com")
            .await
            .unwrap()
            .expect("user must exist");
        assert!(!user.session_enabled, "disable should clear the bit");

        run_users_command(
            UsersCommand::Enable {
                user: "toggle@example.com".into(),
            },
            dir.path(),
        )
        .await
        .unwrap();

        let user = storage
            .get_user_by_email("toggle@example.com")
            .await
            .unwrap()
            .unwrap();
        assert!(user.session_enabled, "enable should restore the bit");
    }

    #[tokio::test]
    async fn users_enable_unknown_user_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_users_command(
            UsersCommand::Enable {
                user: "nobody@example.com".into(),
            },
            dir.path(),
        )
        .await
        .expect_err("unknown user must error");
        assert!(err.to_string().contains("nobody@example.com"), "got: {err}");
    }
}
