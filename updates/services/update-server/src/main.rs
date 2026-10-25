use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use update_server::{
    config::Config,
    http::{AppState, AuthorizationSigner, router_with_authorization},
    repository::PostgresRepository,
    storage::R2Storage,
};

#[derive(Parser)]
struct Args {
    #[arg(long, env = "UPDATE_CONFIG", default_value = "config.json")]
    config: PathBuf,
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, env = "UPDATE_BIND", default_value = "0.0.0.0:8080")]
        bind: SocketAddr,
        #[arg(long, env = "UPDATE_INSTANCE", default_value = "local")]
        instance: String,
    },
    Migrate,
    Validate,
    Smoke {
        #[arg(long)]
        origin: url::Url,
        #[arg(long, default_value = "0.0.0")]
        current_version: semver::Version,
        #[arg(long)]
        require_artifacts: bool,
    },
    Healthcheck {
        #[arg(long, default_value = "http://127.0.0.1:8080/health/ready")]
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    if let Some(Command::Healthcheck { url }) = &args.command {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?
            .get(url)
            .send()
            .await?
            .error_for_status()?;
        return Ok(());
    }
    if !matches!(args.command, Some(Command::Migrate)) {
        let config: Config = serde_json::from_slice(&tokio::fs::read(&args.config).await?)?;
        config.validate()?;
        if matches!(args.command, Some(Command::Validate)) {
            println!("Configuration valid");
            return Ok(());
        }
        if let Some(Command::Smoke {
            origin,
            current_version,
            require_artifacts,
        }) = &args.command
        {
            update_server::smoke::run(&config, origin, current_version, *require_artifacts).await?;
            return Ok(());
        }
    }
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(
            args.database_url
                .as_deref()
                .context("DATABASE_URL required for serve/migrate")?,
        )
        .await?;
    if matches!(args.command, Some(Command::Migrate)) {
        sqlx::migrate!("./migrations").run(&pool).await?;
        return Ok(());
    }
    let config: Config = serde_json::from_slice(&tokio::fs::read(&args.config).await?)?;
    config.validate()?;
    let access = std::env::var("R2_ACCESS_KEY_ID")?;
    let secret = std::env::var("R2_SECRET_ACCESS_KEY")?;
    let storage = Arc::new(R2Storage::new(&config, &access, &secret)?);
    let (bind, instance) = match args.command {
        Some(Command::Serve { bind, instance }) => (bind, instance),
        _ => ("0.0.0.0:8080".parse()?, "local".into()),
    };
    let draining = Arc::new(AtomicBool::new(false));
    let state = AppState {
        config: Arc::new(config),
        repository: Arc::new(PostgresRepository { pool }),
        storage,
        draining: draining.clone(),
        inflight: Arc::new(AtomicUsize::new(0)),
        instance,
    };
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let authorization = match std::env::var_os("UPDATE_AUTHORIZATION_KEY_FILE") {
        Some(path) => {
            let raw = tokio::fs::read_to_string(path).await?;
            let raw = raw.trim();
            anyhow::ensure!(
                raw.len() == 64 && raw.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid authorization key"
            );
            let mut bytes = [0u8; 32];
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&raw[i * 2..i * 2 + 2], 16)?;
            }
            let key_id = std::env::var("UPDATE_AUTHORIZATION_KEY_ID")
                .context("authorization key ID required")?;
            update_protocol::identifier(&key_id)?;
            let key = ed25519_dalek::SigningKey::from_bytes(&bytes);
            use base64::Engine as _;
            let public =
                base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes());
            anyhow::ensure!(
                state
                    .config
                    .apps
                    .iter()
                    .all(|a| !a.metadata_keys.values().any(|v| v == &public)),
                "authorization key must not be a release metadata key"
            );
            Some(Arc::new(AuthorizationSigner { key_id, key }))
        }
        None => None,
    };
    tracing::info!(%bind,"update server listening");
    axum::serve(listener, router_with_authorization(state, authorization))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! {_ = tokio::signal::ctrl_c()=>(),_ = terminate.recv()=>()}
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            draining.store(true, Ordering::Relaxed);
        })
        .await?;
    Ok(())
}
