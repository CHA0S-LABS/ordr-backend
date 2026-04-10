//! Fill plan types.
//!
//! A fill plan is the output of the matching engine: a list of individual
//! fills that together satisfy (fully or partially) a taker's order.
//! Each fill targets one specific maker order on one specific maker market.
//! The plan can span multiple makers across the global orderbook.

use serde::{Deserialize, Serialize};

use crate::types::Side;

/// A taker's order request submitted to the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakerOrder {
    /// Taker's side: Bid = taker is buying base, Ask = taker is selling base.
    pub side: Side,

    /// Total size the taker wants to fill (in base token units).
    pub size: u64,

    /// Slippage protection (optional).
    /// For bids: max price the taker will pay.
    /// For asks: min price the taker will accept.
    pub limit_price: Option<u64>,

    /// Taker's base token account (base58 pubkey).
    pub taker_base_ata: String,

    /// Taker's quote token account (base58 pubkey).
    pub taker_quote_ata: String,

    /// Taker's wallet pubkey (base58).
    pub taker: String,
}

/// A single fill against one maker order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    /// The maker's market account address.
    pub market_address: String,

    /// The maker order being filled against.
    pub order_id: i64,

    /// Side of the maker order (opposite of the taker's side).
    pub maker_side: Side,

    /// Size being filled in this leg (in base token units).
    pub fill_size: u64,

    /// The actual price of the maker order.
    pub price: u64,

    /// Quote tokens involved in this fill: fill_size * price.
    pub quote_amount: u64,

    /// The maker's market bid slab address.
    pub bid_address: String,

    /// The maker's market ask slab address.
    pub ask_address: String,

    /// The maker's base vault address.
    pub base_vault: String,

    /// The maker's quote vault address.
    pub quote_vault: String,

    /// Owner of the maker order.
    pub maker_owner: String,
}

/// The complete fill plan returned by the matching engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillPlan {
    /// The original taker order request.
    pub taker_order: TakerOrder,

    /// Ordered list of fills. Each fill targets one maker order.
    /// Execute in order — earlier fills are better prices.
    pub fills: Vec<Fill>,

    /// Total base tokens filled across all legs.
    pub total_filled: u64,

    /// Total quote tokens involved across all legs.
    pub total_quote: u64,

    /// Whether the taker order was fully filled.
    pub fully_filled: bool,

    /// Average fill price (total_quote / total_filled).
    /// None if no fills.
    pub avg_price: Option<f64>,
}

impl FillPlan {
    /// Creates an empty fill plan for a taker order.
    pub fn new(taker_order: TakerOrder) -> Self {
        Self {
            taker_order,
            fills: Vec::new(),
            total_filled: 0,
            total_quote: 0,
            fully_filled: false,
            avg_price: None,
        }
    }

    /// Adds a fill to the plan and updates totals.
    pub fn add_fill(&mut self, fill: Fill) {
        self.total_filled += fill.fill_size;
        self.total_quote += fill.quote_amount;
        self.fills.push(fill);

        self.fully_filled = self.total_filled >= self.taker_order.size;
        self.avg_price = if self.total_filled > 0 {
            Some(self.total_quote as f64 / self.total_filled as f64)
        } else {
            None
        };
    }

    /// How much size still needs to be filled.
    pub fn remaining(&self) -> u64 {
        self.taker_order.size.saturating_sub(self.total_filled)
    }
}
