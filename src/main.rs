use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{net::SocketAddr, str::FromStr};
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ops_rust_test2=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:class_manager.db".to_owned());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());

    let options = SqliteConnectOptions::from_str(&database_url)
        .with_context(|| format!("invalid DATABASE_URL: {database_url}"))?
        .create_if_missing(true)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .context("connect database")?;

    ops_rust_test2::init_db(&pool)
        .await
        .context("initialize database")?;

    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("invalid BIND_ADDR: {bind_addr}"))?;
    let listener = TcpListener::bind(addr).await.context("bind server")?;

    println!("Class management system listening on http://{addr}");
    axum::serve(listener, ops_rust_test2::app(pool))
        .await
        .context("serve http")
}
