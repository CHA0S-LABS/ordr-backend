use anyhow::Result;
use ordr_backend::{api, config, db, indexer};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    dotenvy::dotenv().ok();

    let config = config::Config::from_env()?;

    info!("Starting Ordr Matching Engine");
    info!("RPC: {}", config.rpc_url);
    info!("Program: {}", config.program_id);
    info!("Base mint: {}", config.base_mint);
    info!("Quote mint: {}", config.quote_mint);

    let pool = db::pool::create_pool(&config.database_url).await?;
    db::migrations::run_migrations(&pool).await?;

    let indexer_pool = pool.clone();
    let indexer_config = config.clone();

    let indexer_handle = tokio::spawn(async move {
        if let Err(e) = indexer::subscriber::run_polling_indexer(indexer_config, indexer_pool).await
        {
            tracing::error!("Indexer error: {e:#}");
        }
    });

    info!("Indexer started");

    let api_pool = pool.clone();
    let api_handle = tokio::spawn(async move { api::run(api_pool, "0.0.0.0:3000").await });

    info!("API listening on 0.0.0.0:3000");

    tokio::select! {
        _ = indexer_handle => {},
        _ = api_handle => {},
    }

    Ok(())
}
