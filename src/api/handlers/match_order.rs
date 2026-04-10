use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use base64::{engine::general_purpose, Engine};
use serde::{Deserialize, Serialize};
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_transaction::Transaction;

use crate::api::AppState;
use crate::engine::{fill_plan::TakerOrder, matcher, transaction};
use crate::types::Side;

#[derive(Debug, Deserialize)]
pub struct MatchRequest {
    pub side: Side,
    pub size: u64,
    pub limit_price: Option<u64>,
    pub taker: String,
    pub taker_base_ata: String,
    pub taker_quote_ata: String,
}

#[derive(Debug, Serialize)]
pub struct MatchResponse {
    transaction: String,
}

pub async fn match_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MatchRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let taker_order = TakerOrder {
        side: req.side,
        size: req.size,
        limit_price: req.limit_price,
        taker: req.taker.clone(),
        taker_base_ata: req.taker_base_ata,
        taker_quote_ata: req.taker_quote_ata,
    };

    let plan = match matcher::match_taker_order(&state.pool, taker_order).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Matcher error: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "matching failed" })),
            );
        }
    };

    if plan.fills.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "error": "no liquidity" })),
        );
    }

    let ix = match transaction::build_match_taker_order_ix(
        &state.program_id,
        &plan,
        &state.base_mint,
        &state.quote_mint,
    ) {
        Ok(Some(ix)) => ix,
        Ok(None) => {
            return (
                StatusCode::OK,
                Json(serde_json::json!({ "error": "no fills" })),
            )
        }
        Err(e) => {
            tracing::error!("Transaction builder error: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to build instruction" })),
            );
        }
    };

    let blockhash = match state.rpc_client.get_latest_blockhash().await {
        Ok(bh) => bh,
        Err(e) => {
            tracing::error!("Failed to fetch blockhash: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to fetch blockhash" })),
            );
        }
    };

    let taker_pubkey: Pubkey = match req.taker.parse() {
        Ok(pk) => pk,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid taker pubkey" })),
            )
        }
    };

    let message = Message::new_with_blockhash(&[ix], Some(&taker_pubkey), &blockhash);
    let tx = Transaction::new_unsigned(message);

    let bytes = match bincode::serialize(&tx) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Serialization error: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "serialization failed" })),
            );
        }
    };

    let encoded = general_purpose::STANDARD.encode(&bytes);

    (
        StatusCode::OK,
        Json(serde_json::json!({ "transaction": encoded })),
    )
}
