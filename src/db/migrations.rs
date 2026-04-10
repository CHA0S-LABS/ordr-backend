use anyhow::Result;
use sqlx::PgPool;
use tracing::info;

/// Runs database migrations to create the required tables.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    // Create custom enum types for side and status.
    sqlx::query(
        r#"
        DO $$ BEGIN
            CREATE TYPE order_side AS ENUM ('bid', 'ask');
        EXCEPTION
            WHEN duplicate_object THEN null;
        END $$;
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        DO $$ BEGIN
            CREATE TYPE order_status AS ENUM ('open', 'partiallyfilled', 'filled', 'cancelled');
        EXCEPTION
            WHEN duplicate_object THEN null;
        END $$;
        "#,
    )
    .execute(pool)
    .await?;

    // Markets table — tracks each maker's market account and its current state.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS markets (
            market_address  TEXT PRIMARY KEY,
            authority       TEXT NOT NULL,
            base_mint       TEXT NOT NULL,
            quote_mint      TEXT NOT NULL,
            base_vault      TEXT NOT NULL,
            quote_vault     TEXT NOT NULL,
            bid_address     TEXT NOT NULL,
            ask_address     TEXT NOT NULL,
            tick_size       BIGINT NOT NULL,
            lot_size        BIGINT NOT NULL,
            mid_price       BIGINT NOT NULL DEFAULT 0,
            bump            SMALLINT NOT NULL,
            updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Orders table — every active order across all maker books.
    // Composite key: (market_address, order_id, side) uniquely identifies an order.
    // Bid and ask slabs have independent next_id counters, so order_id alone
    // is only unique within a single slab. Adding side to the PK prevents collisions.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS orders (
            market_address  TEXT NOT NULL,
            order_id        BIGINT NOT NULL,
            owner           TEXT NOT NULL,
            side            order_side NOT NULL,
            "offset"        BIGINT NOT NULL,
            size            BIGINT NOT NULL,
            filled_size     BIGINT NOT NULL DEFAULT 0,
            status          order_status NOT NULL DEFAULT 'open',
            mid_price       BIGINT NOT NULL DEFAULT 0,
            tick_size       BIGINT NOT NULL,
            updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

            PRIMARY KEY (market_address, order_id, side)
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Index for fast best-price queries by the engine.
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_orders_side_offset
        ON orders (side, "offset");
        "#,
    )
    .execute(pool)
    .await?;

    // Index for fast lookup by market when syncing.
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_orders_market
        ON orders (market_address);
        "#,
    )
    .execute(pool)
    .await?;

    info!("Database migrations complete");
    Ok(())
}
