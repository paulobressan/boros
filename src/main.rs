use std::{env, error::Error, path, sync::Arc};

use anyhow::Result;
use dotenv::dotenv;
use serde::Deserialize;
use storage::sqlite::{SqliteStorage, SqliteTransaction};
use tokio::try_join;
use tracing::Level;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod pipeline;
mod server;
mod storage;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let env_filter = EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .with_env_var("RUST_LOG")
        .from_env_lossy();

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(env_filter)
        .init();

    let config = Config::new().expect("invalid config file");

    let storage = Arc::new(SqliteStorage::new(path::Path::new(&config.storage.db_path)).await?);
    storage.migrate().await?;

    let _tx_storage = SqliteTransaction::new(storage.clone());

    let pipeline = pipeline::run();
    let server = server::run();

    try_join!(pipeline, server)?;

    Ok(())
}

#[derive(Deserialize)]
struct ConfigStorage {
    db_path: String,
}

#[derive(Deserialize)]
struct Config {
    storage: ConfigStorage,
}

impl Config {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let config = config::Config::builder()
            .add_source(
                config::File::with_name(&env::var("BOROS_CONFIG").unwrap_or("boros.toml".into()))
                    .required(false),
            )
            .add_source(config::Environment::with_prefix("boros").separator("_"))
            .build()?
            .try_deserialize()?;

        Ok(config)
    }
}
