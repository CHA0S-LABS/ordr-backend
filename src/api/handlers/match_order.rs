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

/// Token decimal scale factor. Both mints use 0 decimals, so 1 UI unit = 1 raw unit.
const TOKEN_DECIMALS: f64 = 1.0;

#[derive(Debug, Deserialize)]
pub struct MatchRequest {
    pub side: Side,
    /// Size in human-readable token units (e.g. 0.1 = 100_000 base units)
    pub size: f64,
    /// Limit price in human-readable units (optional)
    pub limit_price: Option<f64>,
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
    // Validate inputs before touching the DB or building transactions.
    if !req.size.is_finite() || req.size <= 0.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "size must be a positive finite number" })),
        );
    }
    if req.size > 1_000_000.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "size exceeds maximum" })),
        );
    }
    if let Some(lp) = req.limit_price {
        if !lp.is_finite() || lp <= 0.0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({ "error": "limit_price must be a positive finite number" }),
                ),
            );
        }
    }
    let taker_pubkey: Pubkey = match req.taker.parse() {
        Ok(pk) => pk,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid taker pubkey" })),
            )
        }
    };
    if req.taker_base_ata.parse::<Pubkey>().is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid taker_base_ata" })),
        );
    }
    if req.taker_quote_ata.parse::<Pubkey>().is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid taker_quote_ata" })),
        );
    }

    let taker_order = TakerOrder {
        side: req.side,
        size: (req.size * TOKEN_DECIMALS).round() as u64,
        limit_price: req.limit_price.map(|p| (p * TOKEN_DECIMALS).round() as u64),
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

    // Cap at 1 fill to avoid same-slab aliasing in the on-chain program.
    // When two fills hit the same slab, `borrow_unchecked_mut` is called twice
    // on the same account, causing UB that makes best_mut() return None on the
    // second pass. One fill per transaction is safe and sufficient for now.
    let mut plan = plan;
    plan.fills.truncate(1);

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
        Json(serde_json::json!({
            "transaction": encoded,
            "price": plan.avg_price.unwrap_or(0.0).round() as i64,
            "size": plan.total_filled as i64,
            "side": plan.taker_order.side,
        })),
    )
}
