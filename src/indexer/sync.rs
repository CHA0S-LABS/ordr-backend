//! Sync module.
//!
//! Takes a parsed slab snapshot and reconciles it against the database.
//! New/updated orders are upserted, orders that no longer exist on-chain
//! (cancelled or filled and removed from the slab) are deleted from the DB.

use anyhow::Result;
use sqlx::PgPool;
use tracing::debug;

use crate::db::queries;
use crate::indexer::parser;
use crate::types::Side;

/// Parses all orders from a raw slab account data buffer and syncs
/// them to the database.
///
/// This is a full-snapshot sync:
/// 1. Parse all active orders from the slab.
/// 2. Batch upsert all parsed orders (inserts new, updates changed).
/// 3. Delete any orders in the DB for this market+side that are NOT
///    in the current slab snapshot (they were cancelled or fully filled
///    and removed on-chain).
pub async fn sync_slab_to_db(
    pool: &PgPool,
    slab_data: &[u8],
    market_address: &str,
    mid_price: i64,
    tick_size: i64,
    side: Side,
) -> Result<()> {
    // Parse all active orders from the raw slab bytes.
    let orders = parser::parse_slab_orders(slab_data, market_address, mid_price, tick_size)?;

    let order_count = orders.len();

    // Collect active order IDs for the stale-deletion step.
    let active_ids: Vec<i64> = orders.iter().map(|o| o.order_id).collect();

    // Batch upsert all current orders.
    queries::batch_upsert_orders(pool, &orders).await?;

    // Delete orders that are no longer in the slab.
    let deleted = queries::delete_stale_orders(pool, market_address, side, &active_ids).await?;

    if order_count > 0 || deleted > 0 {
        debug!(
            "Synced {market_address} {:?}: {order_count} active, {deleted} removed",
            side
        );
    }

    Ok(())
}
