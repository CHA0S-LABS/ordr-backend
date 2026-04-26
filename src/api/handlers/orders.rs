use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::AppState;

#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    pub owner: Option<String>,
    /// If "true", returns all statuses (filled + cancelled). Otherwise open only.
    pub history: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OrderRow {
    pub order_id: i64,
    pub side: String,
    pub offset: i64,
    pub size: i64,
    pub filled_size: i64,
    pub status: String,
    pub mid_price: i64,
    pub tick_size: i64,
}

pub async fn get_orders(
    State(state): State<Arc<AppState>>,
    Query(q): Query<OrdersQuery>,
) -> (StatusCode, Json<Vec<OrderRow>>) {
    let base_mint = state.base_mint.to_string();
    let quote_mint = state.quote_mint.to_string();
    let history = q.history.as_deref() == Some("true");

    let status_filter = if history {
        "'open','partiallyfilled','filled','cancelled'"
    } else {
        "'open','partiallyfilled'"
    };

    let sql = format!(
        r#"
        SELECT o.order_id, o.side::text, o."offset", o.size, o.filled_size,
               o.status::text, m.mid_price, o.tick_size
        FROM orders o
        JOIN markets m ON o.market_address = m.market_address
        WHERE o.status IN ({status_filter})
          AND m.base_mint = $1
          AND m.quote_mint = $2
          {owner_clause}
        ORDER BY o.updated_at DESC
        LIMIT 200
        "#,
        status_filter = status_filter,
        owner_clause = if q.owner.is_some() {
            "AND o.owner = $3"
        } else {
            ""
        },
    );

    #[allow(clippy::type_complexity)]
    let rows: Vec<(i64, String, i64, i64, i64, String, i64, i64)> = if let Some(owner) = &q.owner {
        sqlx::query_as(&sql)
            .bind(&base_mint)
            .bind(&quote_mint)
            .bind(owner)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
    } else {
        sqlx::query_as(&sql)
            .bind(&base_mint)
            .bind(&quote_mint)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
    };

    let result = rows
        .into_iter()
        .map(
            |(order_id, side, offset, size, filled_size, status, mid_price, tick_size)| OrderRow {
                order_id,
                side,
                offset,
                size,
                filled_size,
                status,
                mid_price,
                tick_size,
            },
        )
        .collect();

    (StatusCode::OK, Json(result))
}
