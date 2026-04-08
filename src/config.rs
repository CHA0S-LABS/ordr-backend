use anyhow::{Context, Result};

/// Application configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Solana RPC HTTP endpoint (e.g. https://api.devnet.solana.com)
    pub rpc_url: String,

    /// Solana RPC WebSocket endpoint (e.g. wss://api.devnet.solana.com)
    pub ws_url: String,

    /// Neon Postgres connection string
    pub database_url: String,

    /// Ordr program ID (base58-encoded pubkey)
    pub program_id: String,

    /// Base token mint (base58-encoded pubkey, e.g. SOL mint)
    pub base_mint: String,

    /// Quote token mint (base58-encoded pubkey, e.g. USDC mint)
    pub quote_mint: String,

    /// Polling interval in milliseconds for fallback polling mode
    pub poll_interval_ms: u64,
}

impl Config {
    /// Loads configuration from environment variables.
    ///
    /// Required env vars:
    ///   RPC_URL, WS_URL, DATABASE_URL, PROGRAM_ID, BASE_MINT, QUOTE_MINT
    ///
    /// Optional:
    ///   POLL_INTERVAL_MS (default: 2000)
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            rpc_url: std::env::var("RPC_URL").context("RPC_URL not set")?,
            ws_url: std::env::var("WS_URL").context("WS_URL not set")?,
            database_url: std::env::var("DATABASE_URL").context("DATABASE_URL not set")?,
            program_id: std::env::var("PROGRAM_ID").context("PROGRAM_ID not set")?,
            base_mint: std::env::var("BASE_MINT").context("BASE_MINT not set")?,
            quote_mint: std::env::var("QUOTE_MINT").context("QUOTE_MINT not set")?,
            poll_interval_ms: std::env::var("POLL_INTERVAL_MS")
                .unwrap_or_else(|_| "2000".to_string())
                .parse()
                .context("POLL_INTERVAL_MS must be a number")?,
        })
    }
}
