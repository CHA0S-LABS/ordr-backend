//! Full engine test: match → build tx → submit to devnet.
//!
//! Usage:
//!   cargo run --bin test_engine
//!
//! Requires .env with DATABASE_URL, RPC_URL, PROGRAM_ID, BASE_MINT, QUOTE_MINT.

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    signature::Signer,
    transaction::Transaction,
};

use ordr_matching_engine::engine::fill_plan::{FillPlan, TakerOrder};
use ordr_matching_engine::engine::matcher::match_taker_order;
use ordr_matching_engine::engine::transaction::build_match_taker_order_ix;
use ordr_matching_engine::types::Side;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let rpc_url = std::env::var("RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    let program_id: Pubkey = std::env::var("PROGRAM_ID").expect("PROGRAM_ID not set").parse()?;
    let base_mint: Pubkey = std::env::var("BASE_MINT").expect("BASE_MINT not set").parse()?;
    let quote_mint: Pubkey = std::env::var("QUOTE_MINT").expect("QUOTE_MINT not set").parse()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;

    let client = RpcClient::new_with_commitment(&rpc_url, CommitmentConfig::confirmed());

    let keypair_path = dirs::home_dir().unwrap().join(".config/solana/id.json");
    let taker_keypair = solana_sdk::signature::read_keypair_file(&keypair_path)
        .map_err(|e| anyhow::anyhow!("Failed to read keypair: {e}"))?;

    let taker_pubkey = taker_keypair.pubkey();
    let taker_base_ata = spl_associated_token_account::get_associated_token_address(
        &taker_pubkey,
        &base_mint,
    );
    let taker_quote_ata = spl_associated_token_account::get_associated_token_address(
        &taker_pubkey,
        &quote_mint,
    );

    println!("=== Ordr Engine Test ===");
    println!("Taker:      {taker_pubkey}");
    println!("Program:    {program_id}");
    println!("Base mint:  {base_mint}");
    println!("Quote mint: {quote_mint}");
    println!("Taker base ATA:  {taker_base_ata}");
    println!("Taker quote ATA: {taker_quote_ata}");

    // ---------------------------------------------------------------
    // Step 1: Run the matching engine
    // ---------------------------------------------------------------
    println!("\n--- Step 1: Running engine (market buy 200) ---");

    let taker_order = TakerOrder {
        side: Side::Bid,
        size: 200,
        limit_price: None,
        taker_base_ata: taker_base_ata.to_string(),
        taker_quote_ata: taker_quote_ata.to_string(),
        taker: taker_pubkey.to_string(),
    };

    let plan = match_taker_order(&pool, taker_order).await?;
    print_plan(&plan);

    if plan.fills.is_empty() {
        println!("No fills — nothing to submit.");
        return Ok(());
    }

    // ---------------------------------------------------------------
    // Step 2: Build the transaction (fully resolved, no placeholders)
    // ---------------------------------------------------------------
    println!("\n--- Step 2: Building transaction ---");

    let ix = build_match_taker_order_ix(&program_id, &plan, &base_mint, &quote_mint)?
        .expect("Expected instruction from non-empty fill plan");

    println!("  Accounts: {} total (4 fixed + {} remaining)",
        ix.accounts.len(), plan.fills.len() * 5);
    println!("  Data: {} bytes", ix.data.len());

    for (i, fill) in plan.fills.iter().enumerate() {
        println!(
            "  Leg {}: {} units @ {} from market {}...",
            i + 1, fill.fill_size, fill.price, &fill.market_address[..8]
        );
    }

    // ---------------------------------------------------------------
    // Step 3: Submit to devnet
    // ---------------------------------------------------------------
    println!("\n--- Step 3: Submitting to devnet ---");

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&taker_pubkey),
        &[&taker_keypair],
        recent_blockhash,
    );

    match client.send_and_confirm_transaction(&tx) {
        Ok(sig) => {
            println!("SUCCESS! Tx: {sig}");
            println!("https://explorer.solana.com/tx/{sig}?cluster=devnet");
        }
        Err(e) => {
            println!("FAILED: {e}");
            println!("\nPossible causes:");
            println!("  - match_taker_order (discriminator 8) not deployed");
            println!("  - On-chain state changed since engine queried");
            println!("  - Insufficient token balance");
            println!("  - Account validation failed on-chain");
        }
    }

    // ---------------------------------------------------------------
    // Step 4: Check post-trade orderbook
    // ---------------------------------------------------------------
    println!("\n--- Step 4: Post-trade orderbook ---");

    let rows = sqlx::query_as::<_, (i64, String, i64, i64, i64, i64)>(
        r#"
        SELECT order_id, side::text, "offset", size, filled_size,
               (mid_price + "offset" * tick_size) as actual_price
        FROM orders
        ORDER BY side, (mid_price + "offset" * tick_size)
        "#,
    )
    .fetch_all(&pool)
    .await?;

    for row in &rows {
        let remaining = row.3 - row.4;
        println!(
            "  #{} {:>3} | price={:>3} | size={:>3} | filled={:>3} | remaining={:>3}",
            row.0, row.1, row.5, row.3, row.4, remaining
        );
    }

    Ok(())
}

fn print_plan(plan: &FillPlan) {
    println!("  Side: {:?} | Size: {} | Limit: {:?}",
        plan.taker_order.side, plan.taker_order.size, plan.taker_order.limit_price);
    println!("  Filled: {}/{} | Fully filled: {}",
        plan.total_filled, plan.taker_order.size, plan.fully_filled);
    println!("  Avg price: {:?}", plan.avg_price);
    println!("  Total quote: {}", plan.total_quote);
    for (i, fill) in plan.fills.iter().enumerate() {
        println!(
            "    Leg {}: {} units @ {} from market {} (order #{})",
            i + 1, fill.fill_size, fill.price, &fill.market_address[..8], fill.order_id
        );
    }
    if plan.fills.is_empty() {
        println!("    (no fills)");
    }
}