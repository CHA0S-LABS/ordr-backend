//! Critbit slab parser.
//!
//! Deserializes raw on-chain account data from maker bid/ask slab accounts
//! into `IndexedOrder` structs. The layouts here mirror the on-chain `repr(C)`
//! structs exactly so we can cast bytes directly.

use crate::types::{IndexedOrder, OrderStatus, Side};
use anyhow::{bail, Context, Result};

// On chain struct sizes (must match the program's repr(C) layouts)

const SLAB_HEADER_LEN: usize = 32;
const INNER_NODE_LEN: usize = 16;
const LEAF_NODE_LEN: usize = 88;
const NODE_PAIR_LEN: usize = INNER_NODE_LEN + LEAF_NODE_LEN;

// Leaf node field offsets within a single LeafNode (88 bytes total)

// key:         [u64; 2]   bytes  0..16
// id:          u64        bytes 16..24
// owner:       [u8; 32]   bytes 24..56
// offset:      i64        bytes 56..64
// size:        u64        bytes 64..72
// filled_size: u64        bytes 72..80
// side:        u8         byte  80
// status:      u8         byte  81
// _pad:        [u8; 2]    bytes 82..84
// next_free:   u32        bytes 84..88

/// Parsed representation of the slab header.
#[derive(Debug)]
pub struct ParsedSlabHeader {
    pub side: u8,
    pub next_id: u64,
    pub root: u32,
    pub leaf_count: u32,
}

/// Parses the slab header from raw account data.
pub fn parse_slab_header(data: &[u8]) -> Result<ParsedSlabHeader> {
    if data.len() < SLAB_HEADER_LEN {
        bail!("Slab data too short for header: {} bytes", data.len());
    }

    Ok(ParsedSlabHeader {
        side: data[0],
        // bytes 8..16 = next_id (skip 7 bytes of padding after side)
        next_id: u64::from_le_bytes(data[8..16].try_into()?),
        // bytes 16..20 = root
        root: u32::from_le_bytes(data[16..20].try_into()?),
        // bytes 24..28 = free_leaf_head (skip free_inner_head at 20..24)
        // bytes 28..32 = leaf_count
        leaf_count: u32::from_le_bytes(data[28..32].try_into()?),
    })
}

/// Computes the capacity (max number of orders) from account data length.
pub fn slab_capacity(data_len: usize) -> usize {
    data_len.saturating_sub(SLAB_HEADER_LEN) / NODE_PAIR_LEN
}

/// Parses all active leaf nodes from a raw slab account data buffer.
///
/// Scans the entire leaf arena and collects orders where `id != 0`
/// (freed slots have id zeroed). Returns them enriched with the
/// market address and current mid/tick from the market state.
///
/// # Arguments
/// * `data` - Raw account data bytes of the bid or ask slab account
/// * `market_address` - Base58 pubkey of the maker's market account
/// * `mid_price` - Current mid price from the market state
/// * `tick_size` - Tick size from the market state
pub fn parse_slab_orders(
    data: &[u8],
    market_address: &str,
    mid_price: i64,
    tick_size: i64,
) -> Result<Vec<IndexedOrder>> {
    let capacity = slab_capacity(data.len());
    if capacity == 0 {
        return Ok(vec![]);
    }

    let header = parse_slab_header(data)?;
    if header.leaf_count == 0 {
        return Ok(vec![]);
    }

    let mut orders = Vec::with_capacity(header.leaf_count as usize);

    // Leaf arena starts after the header + inner node arena.
    let inner_arena_len = capacity * INNER_NODE_LEN;
    let leaf_arena_start = SLAB_HEADER_LEN + inner_arena_len;

    for i in 0..capacity {
        let leaf_offset = leaf_arena_start + i * LEAF_NODE_LEN;
        let leaf_end = leaf_offset + LEAF_NODE_LEN;

        if leaf_end > data.len() {
            break;
        }

        let leaf_data = &data[leaf_offset..leaf_end];

        // id at bytes 16..24 — skip free slots (id == 0).
        let id = u64::from_le_bytes(
            leaf_data[16..24]
                .try_into()
                .context("Failed to parse leaf id")?,
        );

        if id == 0 {
            continue;
        }

        // owner at bytes 24..56
        let owner_bytes: [u8; 32] = leaf_data[24..56]
            .try_into()
            .context("Failed to parse leaf owner")?;
        let owner = bs58::encode(&owner_bytes).into_string();

        // offset at bytes 56..64
        let offset = i64::from_le_bytes(
            leaf_data[56..64]
                .try_into()
                .context("Failed to parse leaf offset")?,
        );

        // size at bytes 64..72
        let size = u64::from_le_bytes(
            leaf_data[64..72]
                .try_into()
                .context("Failed to parse leaf size")?,
        );

        // filled_size at bytes 72..80
        let filled_size = u64::from_le_bytes(
            leaf_data[72..80]
                .try_into()
                .context("Failed to parse leaf filled_size")?,
        );

        // side at byte 80
        let side_byte = leaf_data[80];
        let side = Side::from_u8(side_byte).context("Invalid side byte")?;

        // status at byte 81
        let status_byte = leaf_data[81];
        let status = OrderStatus::from_u8(status_byte).context("Invalid status byte")?;

        orders.push(IndexedOrder {
            order_id: id as i64,
            market_address: market_address.to_string(),
            owner,
            side,
            offset,
            size: size as i64,
            filled_size: filled_size as i64,
            status,
            mid_price,
            tick_size,
        });
    }

    Ok(orders)
}

/// On-chain Market struct field offsets (192 bytes total).
///
/// Used to extract mid_price, tick_size, and account pubkeys
/// from the market PDA account data.
///
/// Layout:
///   base_mint:   [u8; 32]  bytes   0..32
///   quote_mint:  [u8; 32]  bytes  32..64
///   authority:   [u8; 32]  bytes  64..96
///   bid:         [u8; 32]  bytes  96..128
///   ask:         [u8; 32]  bytes 128..160
///   tick_size:   u64       bytes 160..168
///   mid:         u64       bytes 168..176
///   lot_size:    u64       bytes 176..184
///   bump:        u8        byte  184
///   _padding:    [u8; 7]   bytes 185..192
pub const MARKET_LEN: usize = 192;

/// Parsed market state extracted from raw account data.
#[derive(Debug, Clone)]
pub struct ParsedMarket {
    pub base_mint: String,
    pub quote_mint: String,
    pub authority: String,
    pub bid_address: String,
    pub ask_address: String,
    pub tick_size: u64,
    pub mid_price: u64,
    pub lot_size: u64,
    pub bump: u8,
    pub base_decimals: u8,
}

/// Parses a market account's raw data into a `ParsedMarket`.
pub fn parse_market(data: &[u8]) -> Result<ParsedMarket> {
    if data.len() < MARKET_LEN {
        bail!(
            "Market data too short: {} bytes (expected {})",
            data.len(),
            MARKET_LEN
        );
    }

    let pubkey = |start: usize| -> String { bs58::encode(&data[start..start + 32]).into_string() };

    let u64_at = |start: usize| -> Result<u64> {
        Ok(u64::from_le_bytes(data[start..start + 8].try_into()?))
    };

    Ok(ParsedMarket {
        base_mint: pubkey(0),
        quote_mint: pubkey(32),
        authority: pubkey(64),
        bid_address: pubkey(96),
        ask_address: pubkey(128),
        tick_size: u64_at(160)?,
        mid_price: u64_at(168)?,
        lot_size: u64_at(176)?,
        bump: data[184],
        base_decimals: data[185],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slab_capacity_matches_onchain() {
        // On-chain: slab_size(cap) = 32 + cap * 104
        assert_eq!(slab_capacity(32 + 4 * 104), 4);
        assert_eq!(slab_capacity(32 + 1 * 104), 1);
        assert_eq!(slab_capacity(32), 0);
        assert_eq!(slab_capacity(0), 0);
    }

    #[test]
    fn parse_empty_slab_returns_no_orders() {
        let data = vec![0u8; 32 + 4 * 104]; // capacity 4, all zeros
        let orders = parse_slab_orders(&data, "SomeMarket", 1000, 1).unwrap();
        assert!(orders.is_empty());
    }

    #[test]
    fn market_len_matches() {
        // Market struct: 5 * 32 (pubkeys) + 3 * 8 (u64s) + 1 (bump) + 7 (pad) = 192
        assert_eq!(MARKET_LEN, 5 * 32 + 3 * 8 + 1 + 7);
    }
}
