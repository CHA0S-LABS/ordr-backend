//! Account subscriber.
//!
//! Discovers all maker market accounts for the configured base/quote pair,
//! caches them, and polls their bid/ask slab accounts for changes.
//! Only syncs to DB when slab data actually changes (hash-based detection).

use std::collections::HashMap;

use anyhow::{Context, Result};
use solana_account_decoder::UiAccountEncoding;
use solana_client::{
    rpc_client::RpcClient,
    rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
    rpc_filter::{Memcmp, RpcFilterType},
};
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use sqlx::PgPool;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::queries;
use crate::indexer::parser::{parse_market, ParsedMarket, MARKET_LEN};
use crate::indexer::sync;
use crate::ws::WsMessage;

/// Tracks a discovered maker market and its associated slab accounts.
#[derive(Debug, Clone)]
pub struct TrackedMarket {
    pub market_address: String,
    pub market: ParsedMarket,
}

/// Holds cached state for the polling indexer to avoid redundant work.
struct IndexerState {
    /// Cached markets — discovered once, refreshed periodically.
    markets: HashMap<String, TrackedMarket>,

    /// Hash of the last-seen slab data per account address.
    /// Key = slab account address (bid or ask), Value = hash of raw bytes.
    /// Only re-syncs to DB when the hash changes.
    slab_hashes: HashMap<String, u64>,

    /// Counter to trigger periodic market re-discovery.
    /// Re-discovers every N poll cycles to pick up new markets.
    poll_count: u64,
}

impl IndexerState {
    fn new() -> Self {
        Self {
            markets: HashMap::new(),
            slab_hashes: HashMap::new(),
            poll_count: 0,
        }
    }

    /// Returns true if the slab data has changed since last check.
    /// Updates the stored hash if changed.
    fn has_changed(&mut self, address: &str, data: &[u8]) -> bool {
        let new_hash = fnv1a_hash(data);
        let old_hash = self.slab_hashes.get(address).copied();

        if old_hash == Some(new_hash) {
            return false;
        }

        self.slab_hashes.insert(address.to_string(), new_hash);
        true
    }
}

/// FNV-1a 64-bit hash for fast change detection.
/// Not cryptographic — just needs to reliably detect byte-level changes.
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Re-discovery interval: re-scan for new markets every N poll cycles.
const REDISCOVERY_INTERVAL: u64 = 5;

/// Discovers all maker market PDAs for the given base/quote pair
/// by scanning program accounts with memcmp filters on base_mint and quote_mint.
pub fn discover_markets(config: &Config) -> Result<HashMap<String, TrackedMarket>> {
    let rpc = RpcClient::new_with_commitment(&config.rpc_url, CommitmentConfig::confirmed());

    let program_id: Pubkey = config.program_id.parse().context("Invalid PROGRAM_ID")?;

    let base_mint_bytes = bs58::decode(&config.base_mint)
        .into_vec()
        .context("Invalid BASE_MINT")?;
    let quote_mint_bytes = bs58::decode(&config.quote_mint)
        .into_vec()
        .context("Invalid QUOTE_MINT")?;

    let filters = vec![
        RpcFilterType::DataSize(MARKET_LEN as u64),
        RpcFilterType::Memcmp(Memcmp::new_raw_bytes(0, base_mint_bytes)),
        RpcFilterType::Memcmp(Memcmp::new_raw_bytes(32, quote_mint_bytes)),
    ];

    let account_config = RpcProgramAccountsConfig {
        filters: Some(filters),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            commitment: Some(CommitmentConfig::confirmed()),
            ..Default::default()
        },
        ..Default::default()
    };

    let accounts = rpc
        .get_program_accounts_with_config(&program_id, account_config)
        .context("Failed to fetch program accounts")?;

    let mut markets = HashMap::new();

    for (pubkey, account) in &accounts {
        match parse_market(&account.data) {
            Ok(market) => {
                let address = pubkey.to_string();
                markets.insert(
                    address.clone(),
                    TrackedMarket {
                        market_address: address,
                        market,
                    },
                );
            }
            Err(e) => {
                warn!("Failed to parse market account {}: {}", pubkey, e);
            }
        }
    }

    Ok(markets)
}

/// Runs the polling-based indexer loop with caching and change detection.
///
/// - Markets are discovered once at startup and cached. Re-discovery happens
///   every REDISCOVERY_INTERVAL poll cycles to pick up new markets.
/// - Slab data is hashed on each poll. Only changed slabs trigger a DB sync.
/// - Market account data is also fetched each cycle to detect mid price updates.
pub async fn run_polling_indexer(
    config: Config,
    pool: PgPool,
    ws_tx: tokio::sync::broadcast::Sender<WsMessage>,
) -> Result<()> {
    let poll_interval = tokio::time::Duration::from_millis(config.poll_interval_ms);

    info!(
        "Starting polling indexer (interval: {}ms, rediscovery every {} cycles)",
        config.poll_interval_ms, REDISCOVERY_INTERVAL,
    );

    let mut state = IndexerState::new();

    // Initial discovery.
    match discover_async(&config).await {
        Ok(markets) => {
            info!("Discovered {} market(s)", markets.len());
            for (addr, tracked) in &markets {
                info!(
                    "  {} (authority: {}, mid: {})",
                    addr, tracked.market.authority, tracked.market.mid_price
                );
            }
            state.markets = markets;
        }
        Err(e) => {
            error!("Initial discovery failed: {e:#}");
        }
    }

    loop {
        state.poll_count += 1;

        // Periodic re-discovery to pick up new markets.
        if state.poll_count % REDISCOVERY_INTERVAL == 0 {
            match discover_async(&config).await {
                Ok(markets) => {
                    let new_count = markets
                        .keys()
                        .filter(|k| !state.markets.contains_key(*k))
                        .count();
                    if new_count > 0 {
                        info!("Re-discovery found {new_count} new market(s)");
                    }
                    state.markets = markets;
                }
                Err(e) => {
                    warn!("Re-discovery failed, using cached markets: {e:#}");
                }
            }
        }

        if state.markets.is_empty() {
            debug!("No markets cached, waiting...");
            tokio::time::sleep(poll_interval).await;
            continue;
        }

        // Collect market info for the blocking fetch task.
        let markets_snapshot: Vec<(String, TrackedMarket)> = state
            .markets
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let rpc_url = config.rpc_url.clone();

        // Fetch all account data in one blocking task.
        let fetch_result =
            tokio::task::spawn_blocking(move || fetch_all_accounts(&rpc_url, &markets_snapshot))
                .await
                .context("Slab fetch task panicked")?;

        match fetch_result {
            Ok(fetched) => {
                let mut synced = 0;
                let mut skipped = 0;

                for entry in &fetched {
                    let market_changed =
                        state.has_changed(&entry.market_address, &entry.market_data);
                    let bid_changed = state.has_changed(&entry.bid_address, &entry.bid_data);
                    let ask_changed = state.has_changed(&entry.ask_address, &entry.ask_data);

                    if !bid_changed && !ask_changed && !market_changed {
                        skipped += 1;
                        continue;
                    }

                    // Re-parse market to get fresh mid price.
                    let fresh_market = match parse_market(&entry.market_data) {
                        Ok(m) => m,
                        Err(e) => {
                            error!("Failed to parse market {}: {e:#}", entry.market_address);
                            continue;
                        }
                    };

                    if market_changed {
                        debug!(
                            "Market {} mid updated to {}",
                            entry.market_address, fresh_market.mid_price
                        );

                        let vault_address = match fresh_market.authority.parse::<Pubkey>() {
                            Ok(authority_pk) => {
                                let program_pk: Pubkey =
                                    config.program_id.parse().unwrap_or_default();
                                let (vault_pda, _) = Pubkey::find_program_address(
                                    &[b"vault", authority_pk.as_ref()],
                                    &program_pk,
                                );
                                vault_pda.to_string()
                            }
                            Err(_) => String::new(),
                        };

                        if let Err(e) = crate::db::queries::upsert_market(
                            &pool,
                            &entry.market_address,
                            &fresh_market.authority,
                            &fresh_market.base_mint,
                            &fresh_market.quote_mint,
                            &vault_address,
                            &fresh_market.bid_address,
                            &fresh_market.ask_address,
                            fresh_market.tick_size as i64,
                            fresh_market.lot_size as i64,
                            fresh_market.mid_price as i64,
                            fresh_market.bump as i16,
                        )
                        .await
                        {
                            error!("Failed to upsert market: {e:#}");
                        }
                    }

                    if bid_changed {
                        debug!("Bid slab changed for {}", entry.market_address);
                        if let Err(e) = sync::sync_slab_to_db(
                            &pool,
                            &entry.bid_data,
                            &entry.market_address,
                            fresh_market.mid_price as i64,
                            fresh_market.tick_size as i64,
                            crate::types::Side::Bid,
                        )
                        .await
                        {
                            error!("Failed to sync bids: {e:#}");
                        }
                    }

                    if ask_changed {
                        debug!("Ask slab changed for {}", entry.market_address);
                        if let Err(e) = sync::sync_slab_to_db(
                            &pool,
                            &entry.ask_data,
                            &entry.market_address,
                            fresh_market.mid_price as i64,
                            fresh_market.tick_size as i64,
                            crate::types::Side::Ask,
                        )
                        .await
                        {
                            error!("Failed to sync asks: {e:#}");
                        }
                    }

                    synced += 1;
                }
                if synced > 0 {
                    // Broadcast fresh orderbook snapshot to all WS clients
                    match queries::get_orderbook_snapshot(
                        &pool,
                        &config.base_mint,
                        &config.quote_mint,
                    )
                    .await
                    {
                        Ok(snap) => {
                            let _ = ws_tx.send(WsMessage::Orderbook(snap));
                        }
                        Err(e) => warn!("Failed to fetch orderbook for broadcast: {e:#}"),
                    }
                    info!(
                        "Poll #{}: {synced} market(s) synced, {skipped} unchanged",
                        state.poll_count
                    );
                } else {
                    info!(
                        "Poll #{}: no changes ({skipped} market(s) unchanged)",
                        state.poll_count
                    );
                }
            }
            Err(e) => {
                error!("Slab fetch error: {e:#}");
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Runs discovery in a blocking task.
async fn discover_async(config: &Config) -> Result<HashMap<String, TrackedMarket>> {
    let config_clone = config.clone();
    tokio::task::spawn_blocking(move || discover_markets(&config_clone))
        .await
        .context("Discovery task panicked")?
}

/// Data fetched for a single market in one poll cycle.
struct FetchedMarketData {
    market_address: String,
    bid_address: String,
    ask_address: String,
    market_data: Vec<u8>,
    bid_data: Vec<u8>,
    ask_data: Vec<u8>,
}

/// Fetches market + bid + ask account data for all tracked markets.
fn fetch_all_accounts(
    rpc_url: &str,
    markets: &[(String, TrackedMarket)],
) -> Result<Vec<FetchedMarketData>> {
    let rpc = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());

    let mut results = Vec::with_capacity(markets.len());

    for (address, tracked) in markets {
        let market_pubkey: Pubkey = address.parse().context("Invalid market address")?;
        let bid_pubkey: Pubkey = tracked
            .market
            .bid_address
            .parse()
            .context("Invalid bid address")?;
        let ask_pubkey: Pubkey = tracked
            .market
            .ask_address
            .parse()
            .context("Invalid ask address")?;

        let market_account = rpc
            .get_account(&market_pubkey)
            .context("Failed to fetch market account")?;
        let bid_account = rpc
            .get_account(&bid_pubkey)
            .context("Failed to fetch bid slab")?;
        let ask_account = rpc
            .get_account(&ask_pubkey)
            .context("Failed to fetch ask slab")?;

        results.push(FetchedMarketData {
            market_address: address.clone(),
            bid_address: tracked.market.bid_address.clone(),
            ask_address: tracked.market.ask_address.clone(),
            market_data: market_account.data,
            bid_data: bid_account.data,
            ask_data: ask_account.data,
        });
    }

    Ok(results)
}
