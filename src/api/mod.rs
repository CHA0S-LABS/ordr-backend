use axum::{routing::get, Router};
use sqlx::PgPool;
use std::sync::Arc;

use crate::api::handlers::health::health;
use crate::api::handlers::makers::get_makers;
pub mod handlers;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

pub async fn run(pool: PgPool, addr: &str) {
    let state = Arc::new(AppState { pool });

    let app = Router::new()
        .route("/health", get(health))
        .route("/makers", get(get_makers))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
