use anyhow::Result;
use sqlx::PgPool;
use tracing::debug;

use crate::types::{IndexedOrder, OrderStatus, Side};

/// Upserts a market's state into the markets table.
/// Called by the indexer when it discovers or updates a maker's market account.
pub async fn upsert_market(
    pool: &PgPool,
    market_address: &str,
    authority: &str,
    base_mint: &str,
    quote_mint: &str,
    base_vault: &str,
    quote_vault: &str,
    bid_address: &str,
    ask_address: &str,
    tick_size: i64,
    lot_size: i64,
    mid_price: i64,
    bump: i16,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO markets (
            market_address, authority, base_mint, quote_mint,
            base_vault, quote_vault, bid_address, ask_address,
            tick_size, lot_size, mid_price, bump, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NOW())
        ON CONFLICT (market_address) DO UPDATE SET
            mid_price = EXCLUDED.mid_price,
            updated_at = NOW()
        "#,
    )
    .bind(market_address)
    .bind(authority)
    .bind(base_mint)
    .bind(quote_mint)
    .bind(base_vault)
    .bind(quote_vault)
    .bind(bid_address)
    .bind(ask_address)
    .bind(tick_size)
    .bind(lot_size)
    .bind(mid_price)
    .bind(bump)
    .execute(pool)
    .await?;

    debug!("Upserted market {market_address}");
    Ok(())
}

/// Upserts an order into the orders table.
///
/// Called by the sync module after parsing a critbit slab.
/// Uses ON CONFLICT to update filled_size and status if the order already exists.
pub async fn upsert_order(pool: &PgPool, order: &IndexedOrder) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO orders (
            market_address, order_id, owner, side, "offset",
            size, filled_size, status, mid_price, tick_size, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
        ON CONFLICT (market_address, order_id, side) DO UPDATE SET
            filled_size = EXCLUDED.filled_size,
            status = EXCLUDED.status,
            mid_price = EXCLUDED.mid_price,
            updated_at = NOW()
        "#,
    )
    .bind(&order.market_address)
    .bind(order.order_id)
    .bind(&order.owner)
    .bind(&order.side)
    .bind(order.offset)
    .bind(order.size)
    .bind(order.filled_size)
    .bind(&order.status)
    .bind(order.mid_price)
    .bind(order.tick_size)
    .execute(pool)
    .await?;

    Ok(())
}

/// Batch upserts multiple orders in a single transaction.
///
/// More efficient than individual upserts when syncing an entire slab.
pub async fn batch_upsert_orders(pool: &PgPool, orders: &[IndexedOrder]) -> Result<()> {
    if orders.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    for order in orders {
        sqlx::query(
            r#"
            INSERT INTO orders (
                market_address, order_id, owner, side, "offset",
                size, filled_size, status, mid_price, tick_size, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
            ON CONFLICT (market_address, order_id, side) DO UPDATE SET
                filled_size = EXCLUDED.filled_size,
                status = EXCLUDED.status,
                mid_price = EXCLUDED.mid_price,
                updated_at = NOW()
            "#,
        )
        .bind(&order.market_address)
        .bind(order.order_id)
        .bind(&order.owner)
        .bind(&order.side)
        .bind(order.offset)
        .bind(order.size)
        .bind(order.filled_size)
        .bind(&order.status)
        .bind(order.mid_price)
        .bind(order.tick_size)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    debug!("Batch upserted {} orders", orders.len());
    Ok(())
}

/// Deletes orders from the database that are no longer present in the on-chain slab.
///
/// Called during sync: the parser extracts all active order IDs from the slab,
/// and any orders in the DB for that market that are NOT in that set get deleted.
pub async fn delete_stale_orders(
    pool: &PgPool,
    market_address: &str,
    side: Side,
    active_order_ids: &[i64],
) -> Result<u64> {
    if active_order_ids.is_empty() {
        // If no active orders, delete all orders for this market+side.
        let result = sqlx::query(
            r#"
            DELETE FROM orders
            WHERE market_address = $1 AND side = $2
            "#,
        )
        .bind(market_address)
        .bind(&side)
        .execute(pool)
        .await?;
        return Ok(result.rows_affected());
    }

    let result = sqlx::query(
        r#"
        DELETE FROM orders
        WHERE market_address = $1
          AND side = $2
          AND order_id != ALL($3)
        "#,
    )
    .bind(market_address)
    .bind(&side)
    .bind(active_order_ids)
    .execute(pool)
    .await?;

    let deleted = result.rows_affected();
    if deleted > 0 {
        debug!("Deleted {deleted} stale orders from {market_address}");
    }
    Ok(deleted)
}

/// Fetches the best ask across all markets (lowest actual price).
///
/// Used by the matching engine to find the global best ask.
pub async fn get_best_ask(pool: &PgPool) -> Result<Option<IndexedOrder>> {
    let row = sqlx::query_as::<_, (String, i64, String, i64, i64, i64, i64, i64)>(
        r#"
        SELECT market_address, order_id, owner, "offset",
               size, filled_size, mid_price, tick_size
        FROM orders
        WHERE side = 'ask'
          AND status IN ('open', 'partiallyfilled')
          AND size > filled_size
        ORDER BY (mid_price + "offset" * tick_size) ASC, order_id ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| IndexedOrder {
        market_address: r.0,
        order_id: r.1,
        owner: r.2,
        side: Side::Ask,
        offset: r.3,
        size: r.4,
        filled_size: r.5,
        status: OrderStatus::Open,
        mid_price: r.6,
        tick_size: r.7,
    }))
}

/// Fetches the best bid across all markets (highest actual price).
///
/// Used by the matching engine to find the global best bid.
pub async fn get_best_bid(pool: &PgPool) -> Result<Option<IndexedOrder>> {
    let row = sqlx::query_as::<_, (String, i64, String, i64, i64, i64, i64, i64)>(
        r#"
        SELECT market_address, order_id, owner, "offset",
               size, filled_size, mid_price, tick_size
        FROM orders
        WHERE side = 'bid'
          AND status IN ('open', 'partiallyfilled')
          AND size > filled_size
        ORDER BY (mid_price + "offset" * tick_size) DESC, order_id ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| IndexedOrder {
        market_address: r.0,
        order_id: r.1,
        owner: r.2,
        side: Side::Bid,
        offset: r.3,
        size: r.4,
        filled_size: r.5,
        status: OrderStatus::Open,
        mid_price: r.6,
        tick_size: r.7,
    }))
}