//! Ordr test setup client — global vault architecture.
//!
//! Flow:
//!   1. Create test mints
//!   2. Create authority ATAs + mint tokens
//!   3. Create bid/ask slab accounts
//!   4. create_market  (discriminator 0) — no vaults in market
//!   5. create_vault   (discriminator 9) — global vault PDA for maker
//!   6. Create vault ATAs for base + quote (client-side ATA creation)
//!   7. deposit        (discriminator 10) — fund vault with tokens
//!   8. place_order    (discriminator 1)  — just intent, no token lock
//!
//! Usage:
//!   RPC_URL=https://api.devnet.solana.com cargo run -- <PROGRAM_ID>

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

const SLAB_HEADER_LEN: usize = 32;
const INNER_NODE_LEN: usize = 16;
const LEAF_NODE_LEN: usize = 88;
const NODE_PAIR_LEN: usize = INNER_NODE_LEN + LEAF_NODE_LEN;

const MARKET_SEED: &[u8] = b"market";
const VAULT_SEED: &[u8] = b"vault";

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
    let rpc_url =
        env::var("RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());

    println!("=== Ordr Devnet Setup (Global Vault) ===");
    println!("RPC:        {rpc_url}");
    println!("Program ID: {program_id}");

    let client = RpcClient::new_with_commitment(&rpc_url, CommitmentConfig::confirmed());

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
    // Step 1: Create test mints
    // ---------------------------------------------------------------
    println!("\n--- Step 1: Creating test mints ---");

    let base_mint = Keypair::new();
    let quote_mint = Keypair::new();

    create_mint(&client, &authority, &base_mint, 0)?;
    println!("Base mint:  {} (0 decimals)", base_mint.pubkey());

    create_mint(&client, &authority, &quote_mint, 0)?;
    println!("Quote mint: {} (0 decimals)", quote_mint.pubkey());

    // ---------------------------------------------------------------
    // Step 2: Create authority ATAs + mint tokens
    // ---------------------------------------------------------------
    println!("\n--- Step 2: Minting test tokens to authority ---");

    let authority_base_ata =
        create_ata_and_mint(&client, &authority, &base_mint.pubkey(), 1_000_000)?;
    println!("Authority base ATA:  {authority_base_ata} (1,000,000 base tokens)");

    let authority_quote_ata =
        create_ata_and_mint(&client, &authority, &quote_mint.pubkey(), 1_000_000)?;
    println!("Authority quote ATA: {authority_quote_ata} (1,000,000 quote tokens)");

    // ---------------------------------------------------------------
    // Step 3: Create bid/ask slab accounts
    // ---------------------------------------------------------------
    println!("\n--- Step 3: Creating bid/ask slab accounts ---");

    let capacity: usize = 16;
    let slab_space = slab_size(capacity);
    println!("Slab capacity: {capacity} orders, {slab_space} bytes");

    let bid_account = Keypair::new();
    let ask_account = Keypair::new();

    create_program_owned_account(&client, &authority, &bid_account, slab_space, &program_id)?;
    println!("Bid account: {}", bid_account.pubkey());

    create_program_owned_account(&client, &authority, &ask_account, slab_space, &program_id)?;
    println!("Ask account: {}", ask_account.pubkey());

    // ---------------------------------------------------------------
    // Step 4: Create market (no vaults — market has 7 accounts now)
    // ---------------------------------------------------------------
    println!("\n--- Step 4: Creating market ---");

    let (market_pda, market_bump) = Pubkey::find_program_address(
        &[
            MARKET_SEED,
            base_mint.pubkey().as_ref(),
            quote_mint.pubkey().as_ref(),
            authority.pubkey().as_ref(),
        ],
        &program_id,
    );
    println!("Market PDA: {market_pda} (bump: {market_bump})");

    let tick_size: u64 = 1;
    let lot_size: u64 = 1;
    let mid_price: u64 = 150;

    // CreateMarketInstructionData: tick_size(8) + lot_size(8) + mid_price(8) + bump(1) + pad(7)
    let mut ix_data = vec![0u8]; // discriminator 0
    ix_data.extend_from_slice(&tick_size.to_le_bytes());
    ix_data.extend_from_slice(&lot_size.to_le_bytes());
    ix_data.extend_from_slice(&mid_price.to_le_bytes());
    ix_data.push(market_bump);
    ix_data.extend_from_slice(&[0u8; 7]);

    let create_market_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),       // 0. authority
            AccountMeta::new(market_pda, false),              // 1. market PDA
            AccountMeta::new_readonly(base_mint.pubkey(), false), // 2. base_mint
            AccountMeta::new_readonly(quote_mint.pubkey(), false), // 3. quote_mint
            AccountMeta::new(bid_account.pubkey(), false),    // 4. bid
            AccountMeta::new(ask_account.pubkey(), false),    // 5. ask
            AccountMeta::new_readonly(system_program::id(), false), // 6. system_program
        ],
        data: ix_data,
    };

    let sig = send_tx(&client, &authority, &[create_market_ix], &[])?;
    println!("Market created! Tx: {sig}");
    println!("Tick: {tick_size}, Lot: {lot_size}, Mid: {mid_price}");

    // ---------------------------------------------------------------
    // Step 5: Create global vault PDA for the maker
    // ---------------------------------------------------------------
    println!("\n--- Step 5: Creating global vault PDA ---");

    let (vault_pda, vault_bump) = Pubkey::find_program_address(
        &[VAULT_SEED, authority.pubkey().as_ref()],
        &program_id,
    );
    println!("Vault PDA: {vault_pda} (bump: {vault_bump})");

    // CreateVaultInstructionData: bump(1)
    let mut vault_ix_data = vec![9u8]; // discriminator 9
    vault_ix_data.push(vault_bump);

    let create_vault_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),             // 0. authority
            AccountMeta::new(vault_pda, false),                     // 1. vault PDA
            AccountMeta::new_readonly(system_program::id(), false), // 2. system_program
        ],
        data: vault_ix_data,
    };

    let sig = send_tx(&client, &authority, &[create_vault_ix], &[])?;
    println!("Vault created! Tx: {sig}");

    // ---------------------------------------------------------------
    // Step 6: Create vault ATAs (vault PDA as owner)
    // ---------------------------------------------------------------
    println!("\n--- Step 6: Creating vault token accounts ---");

    let vault_base_ata = get_associated_token_address(&vault_pda, &base_mint.pubkey());
    let vault_quote_ata = get_associated_token_address(&vault_pda, &quote_mint.pubkey());

    let create_vault_base_ata_ix = create_associated_token_account(
        &authority.pubkey(),
        &vault_pda,
        &base_mint.pubkey(),
        &spl_token::id(),
    );
    let create_vault_quote_ata_ix = create_associated_token_account(
        &authority.pubkey(),
        &vault_pda,
        &quote_mint.pubkey(),
        &spl_token::id(),
    );

    let sig = send_tx(
        &client,
        &authority,
        &[create_vault_base_ata_ix, create_vault_quote_ata_ix],
        &[],
    )?;
    println!("Vault base ATA:  {vault_base_ata}");
    println!("Vault quote ATA: {vault_quote_ata}");
    println!("Vault ATAs created! Tx: {sig}");

    // ---------------------------------------------------------------
    // Step 7: Deposit tokens into vault
    //   Bids need quote: (148*100) + (145*200) = 43,800
    //   Asks need base:  150 + 50 = 200
    //   Deposit a bit extra.
    // ---------------------------------------------------------------
    println!("\n--- Step 7: Depositing tokens into vault ---");

    let base_deposit: u64 = 500;
    let quote_deposit: u64 = 50_000;

    deposit_to_vault(
        &client,
        &authority,
        &program_id,
        &vault_pda,
        &authority_base_ata,
        &vault_base_ata,
        base_deposit,
    )?;
    println!("Deposited {base_deposit} base tokens into vault");

    deposit_to_vault(
        &client,
        &authority,
        &program_id,
        &vault_pda,
        &authority_quote_ata,
        &vault_quote_ata,
        quote_deposit,
    )?;
    println!("Deposited {quote_deposit} quote tokens into vault");

    // ---------------------------------------------------------------
    // Step 8: Place test orders (no vault accounts — just intent)
    // ---------------------------------------------------------------
    println!("\n--- Step 8: Placing test orders ---");
    println!("mid={mid_price}, tick_size={tick_size}, lot_size={lot_size}\n");

    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        -2, 0, 100,
    )?;
    println!("  BID: offset=-2, size=100  → price=148");

    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        -5, 0, 200,
    )?;
    println!("  BID: offset=-5, size=200  → price=145");

    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        3, 1, 150,
    )?;
    println!("  ASK: offset=+3, size=150  → price=153");

    place_order(
        &client, &authority, &program_id, &market_pda,
        &ask_account.pubkey(), &bid_account.pubkey(),
        7, 1, 50,
    )?;
    println!("  ASK: offset=+7, size=50   → price=157");

    // ---------------------------------------------------------------
    // Summary
    // ---------------------------------------------------------------
    println!("\n========================================");
    println!("         Setup Complete");
    println!("========================================");
    println!("Program ID:     {program_id}");
    println!("Market PDA:     {market_pda}");
    println!("Base mint:      {}", base_mint.pubkey());
    println!("Quote mint:     {}", quote_mint.pubkey());
    println!("Bid account:    {}", bid_account.pubkey());
    println!("Ask account:    {}", ask_account.pubkey());
    println!("Vault PDA:      {vault_pda}");
    println!("Vault base ATA: {vault_base_ata}");
    println!("Vault quote ATA:{vault_quote_ata}");
    println!("Mid price:      {mid_price}");
    println!("\nOrderbook:");
    println!("  Bids: offset=-2 (price=148, qty=100), offset=-5 (price=145, qty=200)");
    println!("  Asks: offset=+3 (price=153, qty=150), offset=+7 (price=157, qty=50)");
    println!("\nAdd to your .env.local:");
    println!("NEXT_PUBLIC_PROGRAM_ID={program_id}");
    println!("NEXT_PUBLIC_BASE_MINT={}", base_mint.pubkey());
    println!("NEXT_PUBLIC_QUOTE_MINT={}", quote_mint.pubkey());

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn send_tx(
    client: &RpcClient,
    payer: &Keypair,
    instructions: &[Instruction],
    extra_signers: &[&Keypair],
) -> Result<String> {
    let recent_blockhash = client.get_latest_blockhash()?;
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra_signers);
    let tx = Transaction::new_signed_with_payer(
        instructions,
        Some(&payer.pubkey()),
        &signers,
        recent_blockhash,
    );
    let sig = client.send_and_confirm_transaction(&tx)?;
    Ok(sig.to_string())
}

fn create_mint(client: &RpcClient, payer: &Keypair, mint: &Keypair, decimals: u8) -> Result<()> {
    let rent = client.get_minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN)?;
    let create_ix = system_instruction::create_account(
        &payer.pubkey(),
        &mint.pubkey(),
        rent,
        spl_token::state::Mint::LEN as u64,
        &spl_token::id(),
    );
    let init_ix = token_ix::initialize_mint(
        &spl_token::id(),
        &mint.pubkey(),
        &payer.pubkey(),
        None,
        decimals,
    )?;
    send_tx(client, payer, &[create_ix, init_ix], &[mint])?;
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
    send_tx(client, authority, &[create_ata_ix, mint_to_ix], &[])?;
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
    send_tx(client, payer, &[ix], &[account])?;
    Ok(())
}

fn deposit_to_vault(
    client: &RpcClient,
    authority: &Keypair,
    program_id: &Pubkey,
    vault_pda: &Pubkey,
    authority_ata: &Pubkey,
    vault_ata: &Pubkey,
    amount: u64,
) -> Result<()> {
    // DepositInstructionData: amount(8)
    let mut ix_data = vec![10u8]; // discriminator 10
    ix_data.extend_from_slice(&amount.to_le_bytes());

    let ix = Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),        // 0. authority
            AccountMeta::new_readonly(*vault_pda, false),      // 1. vault PDA
            AccountMeta::new(*authority_ata, false),           // 2. authority_ata (source)
            AccountMeta::new(*vault_ata, false),               // 3. vault_ata (destination)
            AccountMeta::new_readonly(spl_token::id(), false), // 4. token_program
        ],
        data: ix_data,
    };

    let sig = send_tx(client, authority, &[ix], &[])?;
    println!("  Deposit tx: {sig}");
    Ok(())
}

fn place_order(
    client: &RpcClient,
    authority: &Keypair,
    program_id: &Pubkey,
    market: &Pubkey,
    ask: &Pubkey,
    bid: &Pubkey,
    offset: i64,
    side: u8,
    size: u64,
) -> Result<()> {
    // PlaceOrderInstructionData: offset(8) + side(1) + size(8)
    let mut ix_data = vec![1u8]; // discriminator 1
    ix_data.extend_from_slice(&offset.to_le_bytes());
    ix_data.push(side);
    ix_data.extend_from_slice(&size.to_le_bytes());

    let ix = Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),  // 0. authority
            AccountMeta::new_readonly(*market, false),   // 1. market
            AccountMeta::new(*ask, false),               // 2. ask
            AccountMeta::new(*bid, false),               // 3. bid
        ],
        data: ix_data,
    };

    let sig = send_tx(client, authority, &[ix], &[])?;
    println!("  Tx: {sig}");
    Ok(())
}
