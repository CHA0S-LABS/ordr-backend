//! Transaction builder.
//!
//! Takes a FillPlan and the market's mints, constructs a complete
//! `match_taker_order` instruction  with all
//! accounts fully resolved, including maker owner ATAs.

use anyhow::Result;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address;

use crate::engine::fill_plan::FillPlan;
use crate::types::Side;

/// Discriminator for match_taker_order instruction.
const MATCH_TAKER_ORDER_DISCRIMINATOR: u8 = 8;

/// SPL Token program ID.
const TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// Builds a fully resolved `match_taker_order` instruction from a fill plan.
///
/// All accounts are resolved — no placeholders. The returned instruction
/// is ready to be wrapped in a Transaction, signed, and submitted.
///
/// Returns None if the fill plan has no fills.
///
/// # Arguments
/// * `program_id` - The Ordr program ID
/// * `fill_plan` - The fill plan from the matcher
/// * `base_mint` - Base token mint (needed to derive maker ATAs)
/// * `quote_mint` - Quote token mint (needed to derive maker ATAs)
pub fn build_match_taker_order_ix(
    program_id: &Pubkey,
    fill_plan: &FillPlan,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
) -> Result<Option<Instruction>> {
    if fill_plan.fills.is_empty() {
        return Ok(None);
    }

    let taker_order = &fill_plan.taker_order;

    let taker: Pubkey = taker_order
        .taker
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid taker pubkey: {e}"))?;
    let taker_base_ata: Pubkey = taker_order
        .taker_base_ata
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid taker_base_ata: {e}"))?;
    let taker_quote_ata: Pubkey = taker_order
        .taker_quote_ata
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid taker_quote_ata: {e}"))?;
    let token_program: Pubkey = TOKEN_PROGRAM_ID
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid token program: {e}"))?;

    // Fixed accounts.
    let mut accounts = vec![
        AccountMeta::new(taker, true),                   // 0. taker (signer)
        AccountMeta::new(taker_base_ata, false),         // 1. taker_base_ata
        AccountMeta::new(taker_quote_ata, false),        // 2. taker_quote_ata
        AccountMeta::new_readonly(token_program, false), // 3. token_program
    ];

    // Remaining accounts: 5 per fill leg, fully resolved.
    for fill in &fill_plan.fills {
        let maker_market: Pubkey = fill
            .market_address
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid market address: {e}"))?;

        // Taker buying → fill from ask slab
        // Taker selling → fill from bid slab
        let maker_slab: Pubkey = match taker_order.side {
            Side::Bid => fill
                .ask_address
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid ask address: {e}"))?,
            Side::Ask => fill
                .bid_address
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid bid address: {e}"))?,
        };

        let maker_authority: Pubkey = fill
            .maker_authority
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid maker authority: {e}"))?;

        // Derive global vault PDA: ["vault", authority] under the ordr program.
        let (vault_pda, _) =
            Pubkey::find_program_address(&[b"vault", maker_authority.as_ref()], program_id);

        let vault_base_ata = get_associated_token_address(&vault_pda, base_mint);
        println!("vault ka base ata: {}", vault_base_ata);
        let vault_quote_ata = get_associated_token_address(&vault_pda, quote_mint);
        println!("vault ka quote ata: {}", vault_quote_ata);

        accounts.push(AccountMeta::new_readonly(maker_market, false)); // [i*5+0]
        accounts.push(AccountMeta::new(maker_slab, false)); // [i*5+1]
        accounts.push(AccountMeta::new_readonly(vault_pda, false)); // [i*5+2]
        accounts.push(AccountMeta::new(vault_base_ata, false)); // [i*5+3]
        accounts.push(AccountMeta::new(vault_quote_ata, false)); // [i*5+4]
    }

    // Instruction data:
    // [discriminator(1)][taker_side(1)][total_size(8)][max_price(8)][num_fills(1)]
    let mut ix_data = vec![MATCH_TAKER_ORDER_DISCRIMINATOR];

    let side_byte: u8 = match taker_order.side {
        Side::Bid => 0,
        Side::Ask => 1,
    };
    ix_data.push(side_byte);

    ix_data.extend_from_slice(&taker_order.size.to_le_bytes());

    let max_price = match taker_order.limit_price {
        Some(p) => p,
        None => match taker_order.side {
            Side::Bid => u64::MAX,
            Side::Ask => 0,
        },
    };
    ix_data.extend_from_slice(&max_price.to_le_bytes());

    ix_data.push(fill_plan.fills.len() as u8);

    Ok(Some(Instruction {
        program_id: *program_id,
        accounts,
        data: ix_data,
    }))
}
