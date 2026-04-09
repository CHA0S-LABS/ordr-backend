//! Matcher module.
//!
//! Given a taker order, queries the global orderbook from the database,
//! walks through the best available maker orders and produces a fill plan.
//!
//! Priority: price first, then size (larger preferred), then time (earlier order_id).
//! The matcher is stateless — reads from DB, returns a fill plan, modifies nothing.

use anyhow::{bail, Result};
use sqlx::PgPool;
use tracing::{debug, info};

use crate::engine::fill_plan::{Fill, FillPlan, TakerOrder};
use crate::types::Side;

/// A maker order row from the database with all fields needed to construct a fill.
#[derive(Debug)]
struct MakerOrderRow {
    market_address: String,
    order_id: i64,
    owner: String,
    side: Side,
    offset: i64,
    size: i64,
    filled_size: i64,
    mid_price: i64,
    tick_size: i64,
    bid_address: String,
    ask_address: String,
    base_vault: String,
    quote_vault: String,
}

impl MakerOrderRow {
    /// Computes the actual price: mid_price + offset * tick_size.
    fn actual_price(&self) -> i64 {
        self.mid_price + self.offset * self.tick_size
    }

    /// Remaining unfilled size.
    fn remaining(&self) -> u64 {
        (self.size - self.filled_size).max(0) as u64
    }
}

/// Matches a taker order against the global orderbook.
///
/// For a taker BID (buying base):
///   - Walks the ask side, cheapest first, largest size second
///   - Fills until taker size is met or asks exhausted or price limit hit
///
/// For a taker ASK (selling base):
///   - Walks the bid side, most expensive first, largest size second
///   - Fills until taker size is met or bids exhausted or price limit hit
///
/// Returns a FillPlan with zero or more fills.
pub async fn match_taker_order(pool: &PgPool, taker_order: TakerOrder) -> Result<FillPlan> {
    if taker_order.size == 0 {
        bail!("Taker order size must be > 0");
    }

    let mut plan = FillPlan::new(taker_order.clone());

    let maker_orders = fetch_counterparty_orders(pool, &taker_order).await?;

    if maker_orders.is_empty() {
        info!(
            "No counterparty orders available for taker {:?}",
            taker_order.side
        );
        return Ok(plan);
    }

    debug!(
        "Found {} counterparty orders for taker {:?} size={}",
        maker_orders.len(),
        taker_order.side,
        taker_order.size
    );

    for order in &maker_orders {
        if plan.remaining() == 0 {
            break;
        }

        let maker_price = order.actual_price();

        // Price protection check (skip if no limit_price = market order).
        if let Some(limit) = taker_order.limit_price {
            match taker_order.side {
                // Taker is buying: reject if ask price > taker's limit price.
                Side::Bid => {
                    if maker_price as u64 > limit {
                        debug!(
                            "Ask price {} exceeds taker limit {}, stopping",
                            maker_price, limit
                        );
                        break;
                    }
                }
                // Taker is selling: reject if bid price < taker's limit price.
                Side::Ask => {
                    if (maker_price as u64) < limit {
                        debug!(
                            "Bid price {} below taker limit {}, stopping",
                            maker_price, limit
                        );
                        break;
                    }
                }
            }
        }

        let maker_remaining = order.remaining();
        if maker_remaining == 0 {
            continue;
        }

        // Fill the lesser of what's remaining on each side.
        let fill_size = plan.remaining().min(maker_remaining);
        let quote_amount = fill_size * maker_price as u64;

        plan.add_fill(Fill {
            market_address: order.market_address.clone(),
            order_id: order.order_id,
            maker_side: order.side,
            fill_size,
            price: maker_price as u64,
            quote_amount,
            bid_address: order.bid_address.clone(),
            ask_address: order.ask_address.clone(),
            base_vault: order.base_vault.clone(),
            quote_vault: order.quote_vault.clone(),
            maker_owner: order.owner.clone(),
        });

        debug!(
            "Fill: {} units @ {} from market {} order {}",
            fill_size, maker_price, order.market_address, order.order_id
        );
    }

    info!(
        "Fill plan: {} legs, {}/{} filled, avg_price={:?}, fully_filled={}",
        plan.fills.len(),
        plan.total_filled,
        taker_order.size,
        plan.avg_price,
        plan.fully_filled
    );

    Ok(plan)
}

/// Fetches counterparty maker orders sorted by priority:
///   1. Best price first
///   2. Largest remaining size second
///   3. Earliest order_id third (time priority)
///
/// For a taker BID: fetches asks, cheapest first.
/// For a taker ASK: fetches bids, most expensive first.
///
/// Joins with markets table to get vault/slab addresses needed for
/// transaction construction.
async fn fetch_counterparty_orders(
    pool: &PgPool,
    taker_order: &TakerOrder,
) -> Result<Vec<MakerOrderRow>> {
    let rows = match taker_order.side {
        // Taker is buying → match against asks (cheapest first, largest size, earliest id).
        Side::Bid => {
            sqlx::query_as::<_, (
                String, i64, String, i64, i64, i64, i64, i64,
                String, String, String, String,
            )>(
                r#"
                SELECT
                    o.market_address,
                    o.order_id,
                    o.owner,
                    o."offset",
                    o.size,
                    o.filled_size,
                    o.mid_price,
                    o.tick_size,
                    m.bid_address,
                    m.ask_address,
                    m.base_vault,
                    m.quote_vault
                FROM orders o
                JOIN markets m ON o.market_address = m.market_address
                WHERE o.side = 'ask'
                  AND o.status IN ('open', 'partiallyfilled')
                  AND o.size > o.filled_size
                ORDER BY
                    (o.mid_price + o."offset" * o.tick_size) ASC,
                    (o.size - o.filled_size) DESC,
                    o.order_id ASC
                "#,
            )
            .fetch_all(pool)
            .await?
        }
        // Taker is selling → match against bids (most expensive first, largest size, earliest id).
        Side::Ask => {
            sqlx::query_as::<_, (
                String, i64, String, i64, i64, i64, i64, i64,
                String, String, String, String,
            )>(
                r#"
                SELECT
                    o.market_address,
                    o.order_id,
                    o.owner,
                    o."offset",
                    o.size,
                    o.filled_size,
                    o.mid_price,
                    o.tick_size,
                    m.bid_address,
                    m.ask_address,
                    m.base_vault,
                    m.quote_vault
                FROM orders o
                JOIN markets m ON o.market_address = m.market_address
                WHERE o.side = 'bid'
                  AND o.status IN ('open', 'partiallyfilled')
                  AND o.size > o.filled_size
                ORDER BY
                    (o.mid_price + o."offset" * o.tick_size) DESC,
                    (o.size - o.filled_size) DESC,
                    o.order_id ASC
                "#,
            )
            .fetch_all(pool)
            .await?
        }
    };

    let orders: Vec<MakerOrderRow> = rows
        .into_iter()
        .map(|r| {
            let side = match taker_order.side {
                Side::Bid => Side::Ask,
                Side::Ask => Side::Bid,
            };

            MakerOrderRow {
                market_address: r.0,
                order_id: r.1,
                owner: r.2,
                offset: r.3,
                size: r.4,
                filled_size: r.5,
                mid_price: r.6,
                tick_size: r.7,
                bid_address: r.8,
                ask_address: r.9,
                base_vault: r.10,
                quote_vault: r.11,
                side,
            }
        })
        .collect();

    Ok(orders)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fill_plan::FillPlan;

    #[test]
    fn fill_plan_remaining_tracks_correctly() {
        let taker = TakerOrder {
            side: Side::Bid,
            size: 500,
            limit_price: Some(200),
            taker_base_ata: "test".to_string(),
            taker_quote_ata: "test".to_string(),
            taker: "test".to_string(),
        };

        let mut plan = FillPlan::new(taker);
        assert_eq!(plan.remaining(), 500);
        assert!(!plan.fully_filled);

        plan.add_fill(Fill {
            market_address: "m1".to_string(),
            order_id: 1,
            maker_side: Side::Ask,
            fill_size: 150,
            price: 153,
            quote_amount: 150 * 153,
            bid_address: "b".to_string(),
            ask_address: "a".to_string(),
            base_vault: "bv".to_string(),
            quote_vault: "qv".to_string(),
            maker_owner: "o".to_string(),
        });

        assert_eq!(plan.remaining(), 350);
        assert_eq!(plan.total_filled, 150);
        assert!(!plan.fully_filled);

        plan.add_fill(Fill {
            market_address: "m2".to_string(),
            order_id: 2,
            maker_side: Side::Ask,
            fill_size: 350,
            price: 157,
            quote_amount: 350 * 157,
            bid_address: "b".to_string(),
            ask_address: "a".to_string(),
            base_vault: "bv".to_string(),
            quote_vault: "qv".to_string(),
            maker_owner: "o".to_string(),
        });

        assert_eq!(plan.remaining(), 0);
        assert_eq!(plan.total_filled, 500);
        assert!(plan.fully_filled);
        assert!(plan.avg_price.is_some());
    }

    #[test]
    fn fill_plan_market_order_no_limit() {
        let taker = TakerOrder {
            side: Side::Bid,
            size: 100,
            limit_price: None, // market order
            taker_base_ata: "test".to_string(),
            taker_quote_ata: "test".to_string(),
            taker: "test".to_string(),
        };

        let plan = FillPlan::new(taker);
        assert_eq!(plan.remaining(), 100);
        assert!(plan.taker_order.limit_price.is_none());
    }

    #[test]
    fn fill_plan_partial_fill() {
        let taker = TakerOrder {
            side: Side::Ask,
            size: 1000,
            limit_price: Some(140),
            taker_base_ata: "test".to_string(),
            taker_quote_ata: "test".to_string(),
            taker: "test".to_string(),
        };

        let mut plan = FillPlan::new(taker);

        plan.add_fill(Fill {
            market_address: "m1".to_string(),
            order_id: 1,
            maker_side: Side::Bid,
            fill_size: 300,
            price: 149,
            quote_amount: 300 * 149,
            bid_address: "b".to_string(),
            ask_address: "a".to_string(),
            base_vault: "bv".to_string(),
            quote_vault: "qv".to_string(),
            maker_owner: "o".to_string(),
        });

        assert_eq!(plan.remaining(), 700);
        assert!(!plan.fully_filled);
        assert_eq!(plan.fills.len(), 1);
    }
}