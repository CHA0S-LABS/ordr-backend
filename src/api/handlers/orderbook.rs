use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
use std::sync::Arc;

use crate::api::AppState;

#[derive(Debug, Serialize)]
pub struct PriceLevel {
    pub price: i64,
    pub size: i64,
}

#[derive(Debug, Serialize)]
pub struct OrderbookResponse {
    pub asks: Vec<PriceLevel>,
    pub bids: Vec<PriceLevel>,
    pub mid: Option<i64>,
}

pub async fn get_orderbook(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<OrderbookResponse>) {
    let base_mint = state.base_mint.to_string();
    let quote_mint = state.quote_mint.to_string();

    let asks = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT
    (m.mid_price + o."offset" * o.tick_size)::bigint AS price,
    (o.size - o.filled_size)::bigint                 AS remaining_size
FROM orders o
JOIN markets m ON o.market_address = m.market_address
WHERE o.side = 'ask'
  AND o.status IN ('open', 'partiallyfilled')
  AND o.size > o.filled_size
  AND m.base_mint = $1
  AND m.quote_mint = $2
ORDER BY price ASC, remaining_size DESC, o.order_id ASC
LIMIT 12
        "#,
    )
    .bind(&base_mint)
    .bind(&quote_mint)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(price, size)| PriceLevel { price, size })
    .collect();

    let bids = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT
    (m.mid_price + o."offset" * o.tick_size)::bigint AS price,
    (o.size - o.filled_size)::bigint                 AS remaining_size
FROM orders o
JOIN markets m ON o.market_address = m.market_address
WHERE o.side = 'bid'
  AND o.status IN ('open', 'partiallyfilled')
  AND o.size > o.filled_size
  AND m.base_mint = $1
  AND m.quote_mint = $2
ORDER BY price DESC, remaining_size DESC, o.order_id ASC
LIMIT 12
        "#,
    )
    .bind(&base_mint)
    .bind(&quote_mint)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(price, size)| PriceLevel { price, size })
    .collect();

    let mid = sqlx::query_as::<_, (i64,)>(
        "SELECT mid_price FROM markets WHERE base_mint = $1 AND quote_mint = $2 ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(&base_mint)
    .bind(&quote_mint)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .map(|(m,)| m);

    (StatusCode::OK, Json(OrderbookResponse { asks, bids, mid }))
}
