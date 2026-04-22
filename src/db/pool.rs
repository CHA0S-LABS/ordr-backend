use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::info;

/// Creates a connection pool to the Neon Postgres database.

pub async fn create_pool(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(60))
        .idle_timeout(std::time::Duration::from_secs(300))
        .test_before_acquire(true)
        .connect(database_url)
        .await?;

    info!("Connected to Neon Postgres");
    Ok(pool)
}
