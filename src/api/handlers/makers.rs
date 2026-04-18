use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
use std::sync::Arc;

use crate::api::AppState;

#[derive(Serialize, sqlx::FromRow)]
pub struct Market {
    pub market_address: String,
    pub authority: String,
    pub base_mint: String,
    pub quote_mint: String,
    pub vault_address: String,
    pub bid_address: String,
    pub ask_address: String,
    pub tick_size: i64,
    pub lot_size: i64,
    pub mid_price: i64,
    pub bump: i16,
}

pub async fn get_makers(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Vec<Market>>) {
    match sqlx::query_as::<_, Market>("SELECT * FROM markets ORDER BY updated_at DESC")
        .fetch_all(&state.pool)
        .await
    {
        Ok(markets) => (StatusCode::OK, Json(markets)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(vec![])),
    }
}
