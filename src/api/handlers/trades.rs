use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::queries;

#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    pub taker: Option<String>,
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
