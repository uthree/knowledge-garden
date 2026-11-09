use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use knowledge_garden::auth::{INVITE_PREFIX, TOKEN_PREFIX, generate_secret, hash_secret};
use knowledge_garden::config::{Config, Registration};
use knowledge_garden::model::{DEFAULT_SCOPES, Role, Scope, TokenKind, Visibility, validate_username};
use knowledge_garden::store::sqlite::SqliteStore;
use knowledge_garden::store::{AccountStore, NewInvite, NewToken, NewUser};
use knowledge_garden::{AppState, router};

#[derive(Parser)]
#[command(version, about = "Knowledge Garden: a knowledge base for coding agents and humans")]
struct Cli {
    /// SQLite database URL.
    #[arg(
        long,
        env = "KG_DATABASE",
        default_value = "sqlite://knowledge-garden.db",
        global = true
    )]
    database: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server.
    Serve {
        #[arg(long, env = "KG_BIND", default_value = "0.0.0.0:8080")]
        bind: SocketAddr,
        /// Public base URL; entry URIs are built from it.
        #[arg(long, env = "KG_BASE_URL", default_value = "http://localhost:8080")]
        base_url: String,
        #[arg(long, env = "KG_NAME", default_value = "Knowledge Garden")]
        name: String,
        /// Allow guests without a token to read public entries.
        #[arg(long, env = "KG_GUEST_READ", default_value_t = true, action = clap::ArgAction::Set)]
        guest_read: bool,
        #[arg(long, env = "KG_REGISTRATION", value_enum, default_value = "invite")]
        registration: Registration,
        /// Visibility for new entries that do not specify one.
        #[arg(long, env = "KG_DEFAULT_VISIBILITY", default_value = "public", value_parser = parse_visibility)]
        default_visibility: Visibility,
        /// Maximum writes per agent token per hour (0 = unlimited).
        #[arg(long, env = "KG_AGENT_WRITES_PER_HOUR", default_value_t = 60)]
        agent_writes_per_hour: u32,
        /// Seconds between pulls from approved peer Gardens (0 = only on demand).
        #[arg(long, env = "KG_SYNC_INTERVAL_SECS", default_value_t = 300)]
        sync_interval_secs: u64,
    },
    /// Create an admin user and print their first token.
    CreateAdmin {
        username: String,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Set a user's password for signing in to the web UI (read from stdin).
    SetPassword { username: String },
    /// Write a consistent copy of the database to a new file (safe while serving).
    Backup {
        /// Path of the copy, e.g. /data/backups/garden-2026-11-09.db
        path: String,
    },
    /// Create an invite code (attributed to an existing admin).
    CreateInvite {
        /// Username of the admin issuing the invite.
        #[arg(long)]
        by: String,
        #[arg(long, value_enum, default_value = "member")]
        role: RoleArg,
        #[arg(long, default_value_t = 7)]
        days: i64,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum RoleArg {
    Admin,
    Member,
}

/// Reads a password from stdin: twice when typed at a terminal, once when piped.
fn read_password() -> Result<String, Box<dyn std::error::Error>> {
    use std::io::{BufRead, IsTerminal, Write};
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal();
    let read = |prompt: &str| -> std::io::Result<String> {
        if interactive {
            eprint!("{prompt}");
            std::io::stderr().flush()?;
        }
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    };
    let password = read("new password (shown as you type): ")?;
    if interactive && read("again: ")? != password {
        return Err("the passwords do not match".into());
    }
    Ok(password)
}

fn parse_visibility(s: &str) -> Result<Visibility, String> {
    s.parse()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cli = Cli::parse();
    let store = SqliteStore::connect(&cli.database).await?;

    match cli.command {
        Command::Serve {
            bind,
            base_url,
            name,
            guest_read,
            registration,
            default_visibility,
            agent_writes_per_hour,
            sync_interval_secs,
        } => {
            let mut config = Config::new(&base_url, &name);
            config.guest_read = guest_read;
            config.registration = registration;
            config.default_visibility = default_visibility;
            config.agent_writes_per_hour = agent_writes_per_hour;
            let state = AppState::new(store, config);
            if sync_interval_secs > 0 {
                tokio::spawn(knowledge_garden::federation::run_sync_loop(
                    state.clone(),
                    std::time::Duration::from_secs(sync_interval_secs),
                ));
            }
            let app = router(state);
            let listener = tokio::net::TcpListener::bind(bind).await?;
            tracing::info!("listening on {bind}");
            axum::serve(listener, app).await?;
        }
        Command::CreateAdmin { username, display_name } => {
            validate_username(&username)?;
            let user = store
                .create_user(NewUser {
                    display_name: display_name.unwrap_or_else(|| username.clone()),
                    username,
                    role: Role::Admin,
                })
                .await?;
            let secret = generate_secret(TOKEN_PREFIX);
            let mut scopes = DEFAULT_SCOPES.to_vec();
            scopes.push(Scope::Admin);
            store
                .create_token(NewToken {
                    user_id: user.id,
                    name: "default".into(),
                    kind: TokenKind::Human,
                    scopes,
                    token_hash: hash_secret(&secret),
                })
                .await?;
            println!("created admin '{}'. token (shown once):\n{secret}", user.username);
        }
        Command::SetPassword { username } => {
            let user = store.find_user(&username).await?.ok_or("no such user")?;
            let password = read_password()?;
            knowledge_garden::auth::validate_password(&password)?;
            let hash = knowledge_garden::auth::hash_password(password).await?;
            store.set_password(&user.id, Some(&hash)).await?;
            store.delete_user_sessions(&user.id).await?;
            println!("password set for '{}'", user.username);
        }
        Command::Backup { path } => {
            store.backup_to(&path).await?;
            println!("wrote a copy of the database to {path}");
        }
        Command::CreateInvite { by, role, days } => {
            let admin = store
                .find_user(&by)
                .await?
                .filter(|u| u.role == Role::Admin)
                .ok_or("no such admin")?;
            let code = generate_secret(INVITE_PREFIX);
            store
                .create_invite(NewInvite {
                    code_hash: hash_secret(&code),
                    role: match role {
                        RoleArg::Admin => Role::Admin,
                        RoleArg::Member => Role::Member,
                    },
                    created_by: admin.id,
                    note: None,
                    expires_at: (days > 0).then(|| chrono::Utc::now() + chrono::Duration::days(days)),
                })
                .await?;
            println!("invite code (shown once):\n{code}");
        }
    }
    Ok(())
}
