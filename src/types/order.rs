use serde::{Deserialize, Serialize};

/// Side of the order — mirrors the on-chain Side enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "order_side", rename_all = "lowercase")]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    /// Converts from the on-chain u8 representation.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Side::Bid),
            1 => Some(Side::Ask),
            _ => None,
        }
    }
}

/// Status of an order — mirrors the on-chain OrderStatus enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "order_status", rename_all = "lowercase")]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
}

impl OrderStatus {
    /// Converts from the on-chain u8 representation.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(OrderStatus::Open),
            1 => Some(OrderStatus::PartiallyFilled),
            2 => Some(OrderStatus::Filled),
            3 => Some(OrderStatus::Cancelled),
            _ => None,
        }
    }
}

/// An order extracted from a maker's on-chain critbit slab,
/// enriched with the maker's market account address.
///
/// This is the canonical representation stored in the database
/// and used by the matching engine to build the global orderbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedOrder {
    /// Unique order ID assigned by the on-chain program (monotonically increasing per slab).
    pub order_id: i64,

    /// Base58-encoded pubkey of the maker's market account that holds this order.
    pub market_address: String,

    /// Base58-encoded pubkey of the trader who placed this order.
    pub owner: String,

    /// Side of the order (Bid or Ask).
    pub side: Side,

    /// Signed offset in ticks from the maker's mid price.
    /// actual_price = mid + offset * tick_size
    pub offset: i64,

    /// Total order size in base token units.
    pub size: i64,

    /// Amount already filled.
    pub filled_size: i64,

    /// Current order status.
    pub status: OrderStatus,

    /// The maker's current mid price at the time of indexing.
    /// Used by the engine to compute the actual price.
    pub mid_price: i64,

    /// The market's tick size. actual_price = mid_price + offset * tick_size.
    pub tick_size: i64,
}

impl IndexedOrder {
    /// Computes the actual price in quote token units.
    /// actual_price = mid_price + offset * tick_size
    pub fn actual_price(&self) -> Option<i64> {
        self.offset
            .checked_mul(self.tick_size)?
            .checked_add(self.mid_price)
    }

    /// Returns the remaining unfilled size.
    pub fn remaining(&self) -> i64 {
        self.size - self.filled_size
    }
}
