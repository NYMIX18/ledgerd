use ledgerd::{
    api::{self, AppState},
    config::Config,
    domain::AccountKind,
    money::Currency,
    repo, worker,
};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&cfg.database_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    repo::ensure_account(
        &pool,
        worker::MPESA_CLEARING,
        "M-Pesa clearing (paybill)",
        AccountKind::Asset,
        Currency::Kes,
        true,
    )
    .await?;

    let bind = cfg.bind_addr.clone();
    let state = AppState::new(pool, cfg);
    let worker_handle = worker::spawn(state.clone());

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("ledgerd listening on {bind}");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
        })
        .await?;
    worker_handle.abort();
    Ok(())
}
