// cd setup-client

// RPC_URL=https://api.devnet.solana.com cargo run --bin place_test_order -- \
//   --program GHyuDLbmPP6MKZjn1naSSfX9bNqN5FDL6wCd6nUqzgWs \
//   --market bDou5Jxso9ctvE67X7pneyyTU2XX82gXGmsMZxSi6RP \
//   --bid 2z1EZ8FStWc5eXBA3cK83e3RSU5zDfjcFxvpGqHJ2ry8 \
//   --ask EwA2eK5YmZE2AXfRHuQr4Jpa7K8h9Mk9Wmfegs7QwGSp \
//   --base-vault 4Quu8WkWWJqEhvZi1tRDyyWGeMnjPzyDdRTqddpGuoSH \
//   --quote-vault 6McnmAKS2vYV7LH48JCRDUER4VZ8bvbwut7ygM2jHiCe \
//   --base-ata AyZPoUJo2XLNx4M9ryscTrDtvGuZTRGtn8oyYarRzcxN \
//   --quote-ata G2cnmH48uDNW9ghdcq4ZdLKV7JYT86ef1W3J4yBajkMA \
//   --side ask \
//   --offset 6 \
//   --size 30

use std::env;
use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Signer,
    transaction::Transaction,
};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 19 {
        eprintln!("Usage: cargo run --bin place_test_order -- \\");
        eprintln!("  --program <PROGRAM_ID> \\");
        eprintln!("  --market <MARKET_PDA> \\");
        eprintln!("  --bid <BID_ACCOUNT> \\");
        eprintln!("  --ask <ASK_ACCOUNT> \\");
        eprintln!("  --base-vault <BASE_VAULT> \\");
        eprintln!("  --quote-vault <QUOTE_VAULT> \\");
        eprintln!("  --base-ata <YOUR_BASE_ATA> \\");
        eprintln!("  --quote-ata <YOUR_QUOTE_ATA> \\");
        eprintln!("  --side <bid|ask> \\");
        eprintln!("  --offset <OFFSET> \\");
        eprintln!("  --size <SIZE>");
        std::process::exit(1);
    }

    // Parse args
    let mut program_id = String::new();
    let mut market = String::new();
    let mut bid = String::new();
    let mut ask = String::new();
    let mut base_vault = String::new();
    let mut quote_vault = String::new();
    let mut base_ata = String::new();
    let mut quote_ata = String::new();
    let mut side: u8 = 0;
    let mut offset: i64 = 0;
    let mut size: u64 = 0;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--program" => { program_id = args[i + 1].clone(); i += 2; }
            "--market" => { market = args[i + 1].clone(); i += 2; }
            "--bid" => { bid = args[i + 1].clone(); i += 2; }
            "--ask" => { ask = args[i + 1].clone(); i += 2; }
            "--base-vault" => { base_vault = args[i + 1].clone(); i += 2; }
            "--quote-vault" => { quote_vault = args[i + 1].clone(); i += 2; }
            "--base-ata" => { base_ata = args[i + 1].clone(); i += 2; }
            "--quote-ata" => { quote_ata = args[i + 1].clone(); i += 2; }
            "--side" => {
                side = if args[i + 1] == "bid" { 0 } else { 1 };
                i += 2;
            }
            "--offset" => { offset = args[i + 1].parse()?; i += 2; }
            "--size" => { size = args[i + 1].parse()?; i += 2; }
            _ => { i += 1; }
        }
    }

    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    let client = RpcClient::new_with_commitment(&rpc_url, CommitmentConfig::confirmed());

    let keypair_path = dirs::home_dir().unwrap().join(".config/solana/id.json");
    let authority = solana_sdk::signature::read_keypair_file(&keypair_path)
        .map_err(|e| anyhow::anyhow!("Failed to read keypair: {e}"))?;

    let side_str = if side == 0 { "BID" } else { "ASK" };
    println!("Placing {side_str}: offset={offset}, size={size}");
    println!("Market: {market}");

    let program_pubkey: Pubkey = program_id.parse()?;
    let market_pubkey: Pubkey = market.parse()?;
    let bid_pubkey: Pubkey = bid.parse()?;
    let ask_pubkey: Pubkey = ask.parse()?;
    let base_vault_pubkey: Pubkey = base_vault.parse()?;
    let quote_vault_pubkey: Pubkey = quote_vault.parse()?;
    let base_ata_pubkey: Pubkey = base_ata.parse()?;
    let quote_ata_pubkey: Pubkey = quote_ata.parse()?;

    // Build instruction data: [discriminator(1)][offset(8)][side(1)][size(8)]
    let mut ix_data = vec![1u8];
    ix_data.extend_from_slice(&offset.to_le_bytes());
    ix_data.push(side);
    ix_data.extend_from_slice(&size.to_le_bytes());

    let ix = Instruction {
        program_id: program_pubkey,
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new_readonly(market_pubkey, false),
            AccountMeta::new(ask_pubkey, false),
            AccountMeta::new(bid_pubkey, false),
            AccountMeta::new(base_vault_pubkey, false),
            AccountMeta::new(quote_vault_pubkey, false),
            AccountMeta::new(base_ata_pubkey, false),
            AccountMeta::new(quote_ata_pubkey, false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ],
        data: ix_data,
    };

    let recent_blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );

    let sig = client.send_and_confirm_transaction(&tx)?;
    println!("Order placed! Tx: {sig}");

    Ok(())
}