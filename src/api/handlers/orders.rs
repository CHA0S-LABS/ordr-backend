use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
use std::sync::Arc;

use crate::api::AppState;

#[derive(Debug, Serialize)]
pub struct OrderRow {
    pub order_id: i64,
    pub side: String,
    pub offset: i64,
    pub size: i64,
    pub filled_size: i64,
}

pub async fn get_orders(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Vec<OrderRow>>) {
    let base_mint = state.base_mint.to_string();
    let quote_mint = state.quote_mint.to_string();

    let rows = sqlx::query_as::<_, (i64, String, i64, i64, i64)>(
        r#"
        SELECT o.order_id, o.side, o."offset", o.size, o.filled_size
        FROM orders o
        JOIN markets m ON o.market_address = m.market_address
        WHERE o.status IN ('open', 'partiallyfilled')
          AND o.size > o.filled_size
          AND m.base_mint = $1
          AND m.quote_mint = $2
        ORDER BY ABS(o."offset") DESC
        "#,
    )
    .bind(&base_mint)
    .bind(&quote_mint)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(order_id, side, offset, size, filled_size)| OrderRow {
        order_id,
        side,
        offset,
        size,
        filled_size,
    })
    .collect();

    (StatusCode::OK, Json(rows))
}
