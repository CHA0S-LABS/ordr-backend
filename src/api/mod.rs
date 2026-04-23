use axum::routing::post;
use axum::{routing::get, Router};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use sqlx::PgPool;
use std::sync::Arc;

use crate::api::handlers::health::health;
use crate::api::handlers::makers::get_makers;
use crate::api::handlers::match_order::match_order;
use crate::api::handlers::orderbook::get_orderbook;
use crate::api::handlers::orders::get_orders;
use crate::api::handlers::trades::get_trades;
pub mod handlers;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub rpc_client: Arc<RpcClient>,
    pub program_id: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
}

pub async fn run(
    pool: PgPool,
    rpc_client: Arc<RpcClient>,
    program_id: Pubkey,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    addr: &str,
) {
    let state = Arc::new(AppState {
        pool,
        rpc_client,
        program_id,
        base_mint,
        quote_mint,
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/makers", get(get_makers))
        .route("/orderbook", get(get_orderbook))
        .route("/orders", get(get_orders))
        .route("/match_order", post(match_order))
        .route("/trades", get(get_trades))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
