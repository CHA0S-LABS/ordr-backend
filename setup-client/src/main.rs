//! Ordr test setup client.
//!
//! Creates test mints, a market, sets a mid price, and places sample orders
//! so the indexer has data to pick up. Run against devnet.
//!
//! Usage:
//!   RPC_URL=https://api.devnet.solana.com cargo run -- <PROGRAM_ID>
//!
//! Prerequisites:
//!   - Funded keypair at ~/.config/solana/id.json
//!   - Program deployed to devnet

use std::env;

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    system_program,
    transaction::Transaction,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token::instruction as token_ix;

use solana_program::program_pack::Pack;

/// On-chain slab sizing constants (must match the program).
const SLAB_HEADER_LEN: usize = 32;
const INNER_NODE_LEN: usize = 16;
const LEAF_NODE_LEN: usize = 88;
const NODE_PAIR_LEN: usize = INNER_NODE_LEN + LEAF_NODE_LEN; // 104

/// Market PDA seed prefix.
const MARKET_SEED: &[u8] = b"market";

fn slab_size(capacity: usize) -> usize {
    SLAB_HEADER_LEN + capacity * NODE_PAIR_LEN
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: cargo run -- <PROGRAM_ID>");
        std::process::exit(1);
    }

    let program_id: Pubkey = args[1]
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid PROGRAM_ID: {e}"))?;
    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());

    println!("=== Ordr Devnet Setup ===");
    println!("RPC:        {rpc_url}");
    println!("Program ID: {program_id}");

    let client = RpcClient::new_with_commitment(&rpc_url, CommitmentConfig::confirmed());

    // Load the default Solana keypair as the authority.
    let keypair_path = dirs::home_dir()
        .unwrap()
        .join(".config/solana/id.json");
    let authority = solana_sdk::signature::read_keypair_file(&keypair_path)
        .map_err(|e| anyhow::anyhow!("Failed to read keypair: {e}"))?;

    println!("Authority:  {}", authority.pubkey());

    let balance = client.get_balance(&authority.pubkey())?;
    println!("Balance:    {:.4} SOL", balance as f64 / 1e9);

    if balance < 2_000_000_000 {
        println!("Low balance — requesting airdrop...");
        let sig = client.request_airdrop(&authority.pubkey(), 5_000_000_000)?;
        loop {
            if client.confirm_transaction(&sig).unwrap_or(false) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        println!("Airdrop confirmed");
    }

    // ---------------------------------------------------------------
    // Step 1: Create test mints (0 decimals for simple math)
    // ---------------------------------------------------------------
    println!("\n--- Step 1: Creating test mints ---");

    let base_mint = Keypair::new();
    let quote_mint = Keypair::new();

    // 0 decimals: 1 token = 1 raw unit. No decimal confusion.
    create_mint(&client, &authority, &base_mint, 0)?;
    println!("Base mint:  {} (0 decimals)", base_mint.pubkey());

    create_mint(&client, &authority, &quote_mint, 0)?;
    println!("Quote mint: {} (0 decimals)", quote_mint.pubkey());

    // ---------------------------------------------------------------
    // Step 2: Create ATAs and mint test tokens to authority
    // ---------------------------------------------------------------
    println!("\n--- Step 2: Minting test tokens ---");

    let authority_base_ata =
        create_ata_and_mint(&client, &authority, &base_mint.pubkey(), 1_000_000)?;
    println!("Authority base ATA:  {authority_base_ata} (1,000,000 tokens)");

    let authority_quote_ata =
        create_ata_and_mint(&client, &authority, &quote_mint.pubkey(), 1_000_000)?;
    println!("Authority quote ATA: {authority_quote_ata} (1,000,000 tokens)");

    // ---------------------------------------------------------------
    // Step 3: Derive market PDA
    // ---------------------------------------------------------------
    println!("\n--- Step 3: Deriving market PDA ---");

    let (market_pda, bump) = Pubkey::find_program_address(
        &[
            MARKET_SEED,
            base_mint.pubkey().as_ref(),
            quote_mint.pubkey().as_ref(),
            authority.pubkey().as_ref(),
        ],
        &program_id,
    );
    println!("Market PDA: {market_pda}");
    println!("Bump:       {bump}");

    // ---------------------------------------------------------------
    // Step 4: Create bid and ask slab accounts (owned by program)
    // ---------------------------------------------------------------
    println!("\n--- Step 4: Creating bid/ask slab accounts ---");

    let capacity: usize = 16;
    let slab_space = slab_size(capacity);
    println!("Slab capacity: {capacity} orders");
    println!("Slab size:     {slab_space} bytes");

    let bid_account = Keypair::new();
    let ask_account = Keypair::new();

    create_program_owned_account(&client, &authority, &bid_account, slab_space, &program_id)?;
    println!("Bid account: {}", bid_account.pubkey());

    create_program_owned_account(&client, &authority, &ask_account, slab_space, &program_id)?;
    println!("Ask account: {}", ask_account.pubkey());

    // ---------------------------------------------------------------
    // Step 5: Create vault token accounts (authority = market PDA)
    // ---------------------------------------------------------------
    println!("\n--- Step 5: Creating vault token accounts ---");

    let base_vault = Keypair::new();
    let quote_vault = Keypair::new();

    create_vault_token_account(&client, &authority, &base_vault, &base_mint.pubkey(), &market_pda)?;
    println!("Base vault:  {}", base_vault.pubkey());

    create_vault_token_account(&client, &authority, &quote_vault, &quote_mint.pubkey(), &market_pda)?;
    println!("Quote vault: {}", quote_vault.pubkey());

    // ---------------------------------------------------------------
    // Step 6: Create market (tick_size=1, lot_size=1)
    // ---------------------------------------------------------------
    println!("\n--- Step 6: Creating market ---");

    let tick_size: u64 = 1;
    let lot_size: u64 = 1;
    println!("Tick size: {tick_size}");
    println!("Lot size:  {lot_size}");

    // Instruction data: [discriminator(1)][tick_size(8)][lot_size(8)][bump(1)][_pad(7)]
    let mut ix_data = vec![0u8]; // discriminator = 0 for create_market
    ix_data.extend_from_slice(&tick_size.to_le_bytes());
    ix_data.extend_from_slice(&lot_size.to_le_bytes());
    ix_data.push(bump);
    ix_data.extend_from_slice(&[0u8; 7]);

    let create_market_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(market_pda, false),
            AccountMeta::new_readonly(base_mint.pubkey(), false),
            AccountMeta::new_readonly(quote_mint.pubkey(), false),
            AccountMeta::new_readonly(base_vault.pubkey(), false),
            AccountMeta::new_readonly(quote_vault.pubkey(), false),
            AccountMeta::new(bid_account.pubkey(), false),
            AccountMeta::new(ask_account.pubkey(), false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: ix_data,
    };

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[create_market_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    let sig = client.send_and_confirm_transaction(&tx)?;
    println!("Market created! Tx: {sig}");

    // ---------------------------------------------------------------
    // Step 7: Set mid price via update_mid
    // ---------------------------------------------------------------
    println!("\n--- Step 7: Setting mid price ---");

    // mid = 150 (with tick_size=1 and 0 decimals, this means price=150 tokens)
    let mid_price: u64 = 150;
    println!("Mid price: {mid_price}");

    let mut update_mid_data = vec![5u8]; // discriminator = 5
    update_mid_data.extend_from_slice(&mid_price.to_le_bytes());

    let update_mid_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(market_pda, false),
        ],
        data: update_mid_data,
    };

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[update_mid_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    client.send_and_confirm_transaction(&tx)?;
    println!("Mid price set!");

    // ---------------------------------------------------------------
    // Step 8: Place test orders
    // ---------------------------------------------------------------
    println!("\n--- Step 8: Placing test orders ---");
    println!("mid=150, tick_size=1, lot_size=1\n");

    // Bid 1: offset=-2 → price = 150 + (-2 * 1) = 148
    //   Locks: 148 * 100 = 14,800 quote tokens
    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        &base_vault.pubkey(), &quote_vault.pubkey(),
        &authority_base_ata, &authority_quote_ata,
        -2, 0, 100,
    )?;
    println!("  BID: offset=-2, size=100, price=148, locked=14,800 quote");

    // Bid 2: offset=-5 → price = 150 + (-5 * 1) = 145
    //   Locks: 145 * 200 = 29,000 quote tokens
    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        &base_vault.pubkey(), &quote_vault.pubkey(),
        &authority_base_ata, &authority_quote_ata,
        -5, 0, 200,
    )?;
    println!("  BID: offset=-5, size=200, price=145, locked=29,000 quote");

    // Ask 1: offset=+3 → price = 150 + (3 * 1) = 153
    //   Locks: 150 base tokens
    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        &base_vault.pubkey(), &quote_vault.pubkey(),
        &authority_base_ata, &authority_quote_ata,
        3, 1, 150,
    )?;
    println!("  ASK: offset=+3, size=150, price=153, locked=150 base");

    // Ask 2: offset=+7 → price = 150 + (7 * 1) = 157
    //   Locks: 50 base tokens
    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        &base_vault.pubkey(), &quote_vault.pubkey(),
        &authority_base_ata, &authority_quote_ata,
        7, 1, 50,
    )?;
    println!("  ASK: offset=+7, size=50, price=157, locked=50 base");

    // ---------------------------------------------------------------
    // Summary
    // ---------------------------------------------------------------
    println!("\n========================================");
    println!("         Setup Complete");
    println!("========================================");
    println!("\nProgram ID:   {program_id}");
    println!("Market PDA:   {market_pda}");
    println!("Base mint:    {}", base_mint.pubkey());
    println!("Quote mint:   {}", quote_mint.pubkey());
    println!("Bid account:  {}", bid_account.pubkey());
    println!("Ask account:  {}", ask_account.pubkey());
    println!("Base vault:   {}", base_vault.pubkey());
    println!("Quote vault:  {}", quote_vault.pubkey());
    println!("Mid price:    {mid_price}");
    println!("Tick size:    {tick_size}");
    println!("Lot size:     {lot_size}");
    println!("\nOrderbook:");
    println!("  Bids: offset=-2 (price=148, qty=100), offset=-5 (price=145, qty=200)");
    println!("  Asks: offset=+3 (price=153, qty=150), offset=+7 (price=157, qty=50)");
    println!("\nAdd to your .env:");
    println!("PROGRAM_ID={program_id}");
    println!("BASE_MINT={}", base_mint.pubkey());
    println!("QUOTE_MINT={}", quote_mint.pubkey());

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn create_mint(
    client: &RpcClient,
    payer: &Keypair,
    mint: &Keypair,
    decimals: u8,
) -> Result<()> {
    let rent = client.get_minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN)?;

    let create_account_ix = system_instruction::create_account(
        &payer.pubkey(),
        &mint.pubkey(),
        rent,
        spl_token::state::Mint::LEN as u64,
        &spl_token::id(),
    );

    let init_mint_ix = token_ix::initialize_mint(
        &spl_token::id(),
        &mint.pubkey(),
        &payer.pubkey(),
        None,
        decimals,
    )?;

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[create_account_ix, init_mint_ix],
        Some(&payer.pubkey()),
        &[payer, mint],
        recent_blockhash,
    );

    client.send_and_confirm_transaction(&tx)?;
    Ok(())
}

fn create_ata_and_mint(
    client: &RpcClient,
    authority: &Keypair,
    mint: &Pubkey,
    amount: u64,
) -> Result<Pubkey> {
    let ata = get_associated_token_address(&authority.pubkey(), mint);

    let create_ata_ix = create_associated_token_account(
        &authority.pubkey(),
        &authority.pubkey(),
        mint,
        &spl_token::id(),
    );

    let mint_to_ix = token_ix::mint_to(
        &spl_token::id(),
        mint,
        &ata,
        &authority.pubkey(),
        &[],
        amount,
    )?;

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[create_ata_ix, mint_to_ix],
        Some(&authority.pubkey()),
        &[authority],
        recent_blockhash,
    );

    client.send_and_confirm_transaction(&tx)?;
    Ok(ata)
}

fn create_program_owned_account(
    client: &RpcClient,
    payer: &Keypair,
    account: &Keypair,
    space: usize,
    owner: &Pubkey,
) -> Result<()> {
    let rent = client.get_minimum_balance_for_rent_exemption(space)?;

    let ix = system_instruction::create_account(
        &payer.pubkey(),
        &account.pubkey(),
        rent,
        space as u64,
        owner,
    );

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[payer, account],
        recent_blockhash,
    );

    client.send_and_confirm_transaction(&tx)?;
    Ok(())
}

fn create_vault_token_account(
    client: &RpcClient,
    payer: &Keypair,
    vault: &Keypair,
    mint: &Pubkey,
    vault_authority: &Pubkey,
) -> Result<()> {
    let space = spl_token::state::Account::LEN;
    let rent = client.get_minimum_balance_for_rent_exemption(space)?;

    let create_account_ix = system_instruction::create_account(
        &payer.pubkey(),
        &vault.pubkey(),
        rent,
        space as u64,
        &spl_token::id(),
    );

    let init_account_ix = token_ix::initialize_account(
        &spl_token::id(),
        &vault.pubkey(),
        mint,
        vault_authority,
    )?;

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[create_account_ix, init_account_ix],
        Some(&payer.pubkey()),
        &[payer, vault],
        recent_blockhash,
    );

    client.send_and_confirm_transaction(&tx)?;
    Ok(())
}

fn place_order(
    client: &RpcClient,
    authority: &Keypair,
    program_id: &Pubkey,
    market: &Pubkey,
    ask: &Pubkey,
    bid: &Pubkey,
    base_vault: &Pubkey,
    quote_vault: &Pubkey,
    authority_base_ata: &Pubkey,
    authority_quote_ata: &Pubkey,
    offset: i64,
    side: u8,
    size: u64,
) -> Result<()> {
    // PlaceOrderInstructionData is repr(C, packed): offset(i64=8) + side(u8=1) + size(u64=8) = 17 bytes
    let mut ix_data = vec![1u8]; // discriminator = 1 for place_order
    ix_data.extend_from_slice(&offset.to_le_bytes());
    ix_data.push(side);
    ix_data.extend_from_slice(&size.to_le_bytes());

    let ix = Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new_readonly(*market, false),
            AccountMeta::new(*ask, false),
            AccountMeta::new(*bid, false),
            AccountMeta::new(*base_vault, false),
            AccountMeta::new(*quote_vault, false),
            AccountMeta::new(*authority_base_ata, false),
            AccountMeta::new(*authority_quote_ata, false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ],
        data: ix_data,
    };

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&authority.pubkey()),
        &[authority],
        recent_blockhash,
    );

    let sig = client.send_and_confirm_transaction(&tx)?;
    println!("  Tx: {sig}");
    Ok(())
}