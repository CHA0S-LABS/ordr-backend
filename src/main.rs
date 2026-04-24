use anyhow::Result;
use ordr_backend::{api, config, db, indexer, ws::WsMessage};
use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use tokio::sync::broadcast;
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

    info!("Starting Ordr Backend");
    info!("RPC: {}", config.rpc_url);
    info!("Program: {}", config.program_id);
    info!("Base mint: {}", config.base_mint);
    info!("Quote mint: {}", config.quote_mint);

    let pool = db::pool::create_pool(&config.database_url).await?;
    db::migrations::run_migrations(&pool).await?;

    let rpc_client = Arc::new(RpcClient::new(config.rpc_url.clone()));
    let program_id = config.program_id.parse()?;
    let base_mint = config.base_mint.parse()?;
    let quote_mint = config.quote_mint.parse()?;

    let (ws_tx, _) = broadcast::channel::<WsMessage>(128);

    let indexer_pool = pool.clone();
    let indexer_config = config.clone();
    let indexer_ws_tx = ws_tx.clone();

    let indexer_handle = tokio::spawn(async move {
        if let Err(e) =
            indexer::subscriber::run_polling_indexer(indexer_config, indexer_pool, indexer_ws_tx)
                .await
        {
            tracing::error!("Indexer error: {e:#}");
        }
    });

    info!("Indexer started");

    let api_pool = pool.clone();
    let api_rpc = rpc_client.clone();
    let api_ws_tx = ws_tx.clone();
    let api_handle = tokio::spawn(async move {
        api::run(
            api_pool,
            api_rpc,
            program_id,
            base_mint,
            quote_mint,
            api_ws_tx,
            "0.0.0.0:8080",
        )
        .await
    });

    info!("API listening on 0.0.0.0:8080");

    tokio::select! {
        _ = indexer_handle => {},
        _ = api_handle => {},
    }

    Ok(())
}
