//! gRPC-based indexer using Yellowstone Geyser plugin.
//!
//! Replaces RPC polling with real-time account change streaming.
//! Subscribes to all accounts owned by the Ordr program and pushes
//! slab/market updates to the database as they happen.`

use std::collections::HashMap;

use anyhow::{Context, Result};
use futures::stream::StreamExt;
use sqlx::PgPool;
use tracing::{error, info, warn};
use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient};
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts,
};
use crate::indexer::parser::{self, ParsedMarket, MARKET_LEN};
use crate::indexer::sync;

/// Configuration for the gRPC subscriber.
pub struct GrpcConfig {
    pub endpoint: String,
    pub x_token: Option<String>,
    pub program_id: String,
    pub pool: PgPool,
}

/// Starts the gRPC subscriber with automatic reconnection.
pub async fn run_grpc_indexer(config: GrpcConfig) -> Result<()> {
    loop {
        info!("Connecting to Yellowstone gRPC at {}", config.endpoint);

        match run_stream(&config).await {
            Ok(()) => {
                warn!("gRPC stream ended, reconnecting...");
            }
            Err(e) => {
                error!("gRPC stream error: {:?}, reconnecting in 5s...", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_stream(config: &GrpcConfig) -> Result<()> {
    
    let mut client_builder = GeyserGrpcClient::build_from_shared(config.endpoint.clone())?;

    if let Some(ref token) = config.x_token {
        client_builder = client_builder.x_token(Some(token.clone()))?;
    }

    let tls_config = ClientTlsConfig::new().with_native_roots();
    client_builder = client_builder.tls_config(tls_config)?;

    let mut client = client_builder.connect().await?;
    info!("Connected to {}", config.endpoint);

    // Subscribe to all accounts owned by our program
    let mut accounts_filter = HashMap::new();
    accounts_filter.insert(
        "ordr_accounts".to_string(),
        SubscribeRequestFilterAccounts {
            account: vec![],
            owner: vec![config.program_id.clone()],
            filters: vec![],
            nonempty_txn_signature: None,
        },
    );

    let request = SubscribeRequest {
        accounts: accounts_filter,
        commitment: Some(CommitmentLevel::Confirmed as i32),
        ..Default::default()
    };

    let (_subscribe_tx, mut stream) = client.subscribe_with_request(Some(request)).await?;
    info!(
        "Subscribed to account updates for program {}",
        config.program_id
    );

    while let Some(message_result) = stream.next().await {
        let message = message_result?;

        if let Some(update) = message.update_oneof {
            match update {
                UpdateOneof::Account(account_update) => {
                    if let Some(account_info) = account_update.account {
                        let pubkey = bs58::encode(&account_info.pubkey).into_string();
                        let data = &account_info.data;
                        let slot = account_update.slot;

                        if let Err(e) =
                            handle_account_update(&config.pool, &pubkey, data, slot, &config.program_id).await
                        {
                            error!("Failed to handle update for {}: {:?}", pubkey, e);
                        }
                    }
                }
                _ => {} // Ignore slots, blocks, pings
            }
        }
    }

    Ok(())
}

/// Routes account updates based on data size.
///
/// Market = 192 bytes
/// Slab = 32 + capacity * 104 (variable, always > 192)
/// VaultState = 40 bytes
async fn handle_account_update(
    pool: &PgPool,
    pubkey: &str,
    data: &[u8],
    slot: u64,
    program_id: &str,
) -> Result<()> {
    const VAULT_SIZE: usize = 40;
    const SLAB_HEADER_SIZE: usize = 32;
    const NODE_PAIR_SIZE: usize = 104;

    match data.len() {
        VAULT_SIZE => {
            // VaultState — skip for now
        }
        MARKET_LEN => {
            // Market account update
            handle_market_update(pool, pubkey, data, program_id).await?;
        }
        len if len >= SLAB_HEADER_SIZE && (len - SLAB_HEADER_SIZE) % NODE_PAIR_SIZE == 0 => {
            // Slab account update
            handle_slab_update(pool, pubkey, data).await?;
        }
        _ => {
            warn!("Unknown account size {} for {}", data.len(), pubkey);
        }
    }

    Ok(())
}

/// Parses market account data and upserts to DB.
async fn handle_market_update(
    pool: &PgPool,
    pubkey: &str,
    data: &[u8],
    program_id: &str,
) -> Result<()> {
    let market = parser::parse_market(data).context("Failed to parse market")?;

    // Derive vault PDA from authority
    let vault_address = derive_vault_address(&market.authority, program_id);

    crate::db::queries::upsert_market(
        pool,
        pubkey,
        &market.authority,
        &market.base_mint,
        &market.quote_mint,
        &vault_address,
        &market.bid_address,
        &market.ask_address,
        market.tick_size as i64,
        market.lot_size as i64,
        market.mid_price as i64,
        market.bump as i16,
    )
    .await
    .context("Failed to upsert market")?;

    info!(
        "Market {} updated (mid={}, authority={})",
        pubkey, market.mid_price, market.authority
    );

    Ok(())
}

/// Parses slab account data and syncs orders to DB.
async fn handle_slab_update(pool: &PgPool, pubkey: &str, data: &[u8]) -> Result<()> {
    // Find which market this slab belongs to and which side
    let market_info = sqlx::query_as::<_, (String, i64, i64, String, String)>(
        r#"
        SELECT market_address, mid_price, tick_size, bid_address, ask_address
        FROM markets
        WHERE bid_address = $1 OR ask_address = $1
        LIMIT 1
        "#,
    )
    .bind(pubkey)
    .fetch_optional(pool)
    .await?;

    let Some((market_address, mid_price, tick_size, bid_address, _ask_address)) = market_info
    else {
        warn!("No market found for slab {}", pubkey);
        return Ok(());
    };

    let side = if pubkey == bid_address {
        crate::types::Side::Bid
    } else {
        crate::types::Side::Ask
    };

    sync::sync_slab_to_db(pool, data, &market_address, mid_price, tick_size, side).await?;

    info!(
        "Synced {:?} slab {} for market {}",
        side, pubkey, market_address
    );

    Ok(())
}

/// Derives vault PDA address from authority and program ID.
fn derive_vault_address(authority: &str, program_id: &str) -> String {
    use solana_pubkey::Pubkey;

    let Ok(authority_pk) = authority.parse::<Pubkey>() else {
        return String::new();
    };
    let Ok(program_pk) = program_id.parse::<Pubkey>() else {
        return String::new();
    };

    let (vault_pda, _) =
        Pubkey::find_program_address(&[b"vault", authority_pk.as_ref()], &program_pk);

    vault_pda.to_string()
}