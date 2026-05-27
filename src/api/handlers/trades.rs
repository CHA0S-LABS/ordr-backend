use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::queries;
use crate::types::Side;
use crate::ws::{WsMessage, WsTrade};

#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    pub taker: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecordTradeRequest {
    pub price: i64,
    pub size: i64,
    pub side: Side,
    pub taker: String,
}

pub async fn record_trade(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RecordTradeRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match queries::insert_trade(&state.pool, req.price, req.size, &req.side, &req.taker).await {
        Ok(_) => {
            if let Ok(trades) = queries::get_trades_by_taker(&state.pool, &req.taker, 1).await {
                if let Some(t) = trades.into_iter().next() {
                    let _ = state.ws_tx.send(WsMessage::Trade(WsTrade {
                        id: t.id,
                        price: t.price,
                        size: t.size,
                        side: format!("{:?}", t.side).to_lowercase(),
                        taker: t.taker,
                        created_at: t.created_at.to_rfc3339(),
                    }));
                }
            }
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }
        Err(e) => {
            tracing::error!("Failed to record trade: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed" })),
            )
        }
    }
}

pub async fn get_trades(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TradesQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let result = if let Some(taker) = &q.taker {
        queries::get_trades_by_taker(&state.pool, taker, 100).await
    } else {
        queries::get_recent_trades(&state.pool, 50).await
    };

    match result {
        Ok(trades) => (
            StatusCode::OK,
            Json(serde_json::json!({ "trades": trades })),
        ),
        Err(e) => {
            tracing::error!("Failed to fetch trades: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to fetch trades" })),
            )
        }
    }
}
