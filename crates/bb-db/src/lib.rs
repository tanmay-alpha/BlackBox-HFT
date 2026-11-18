//! PostgreSQL 18 persistence foundations. All SQL is schema-qualified and runtime checked.
//!
//! Construct [`DatabaseConfig`] from injected URLs (recommended), or explicitly call
//! `from_env`. Run migrations through the direct URL before opening the application pool.
//! The pool has no minimum connections and no background application polling.
//! Queue completion accepts only already verified/uploaded artifact metadata; object storage
//! and scientific hash computation remain the caller's responsibility.
mod api_reads;
mod demo_version;
mod gc;
mod keys;
mod models;
mod queue;
mod repositories;
mod research;
pub use api_reads::*;
pub use gc::*;
pub use keys::*;
pub use models::*;
pub use queue::*;
pub use repositories::*;
pub use research::*;

use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use std::time::Duration;
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("immutable scientific record")]
    Immutable,
    #[error("illegal job transition")]
    IllegalTransition,
    #[error("identity or version conflict")]
    Conflict,
    #[error("resource not found")]
    NotFound,
    #[error("worker no longer holds the current lease")]
    LeaseLost,
    #[error("nondeterminism detected for an existing run")]
    Nondeterminism,
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("quota exceeded: {0}")]
    Quota(&'static str),
    #[error("database operation failed: {0}")]
    Database(sqlx::Error),
    #[error("migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
}
impl From<sqlx::Error> for Error {
    fn from(value: sqlx::Error) -> Self {
        match value.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("BB001") => Self::Immutable,
            Some("BB002") => Self::IllegalTransition,
            Some("23505") => Self::Conflict,
            _ => Self::Database(value),
        }
    }
}

// Deliberately no Debug implementation: connection URLs contain credentials.
#[derive(Clone)]
pub struct DatabaseConfig {
    pub database_url: String,
    pub database_url_unpooled: String,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
}
impl DatabaseConfig {
    pub fn from_urls(pooled: impl Into<String>, direct: impl Into<String>) -> Self {
        Self {
            database_url: pooled.into(),
            database_url_unpooled: direct.into(),
            max_connections: 5,
            acquire_timeout: Duration::from_secs(15),
        }
    }
    /// Opt-in environment support. No dotenv loading or implicit environment access.
    pub fn from_env() -> Result<Self> {
        let pooled = std::env::var("DATABASE_URL")
            .map_err(|_| Error::Invalid("DATABASE_URL is required".into()))?;
        let direct = std::env::var("DATABASE_URL_UNPOOLED")
            .map_err(|_| Error::Invalid("DATABASE_URL_UNPOOLED is required".into()))?;
        Ok(Self::from_urls(pooled, direct))
    }
    pub async fn connect(&self) -> Result<PgPool> {
        if !(1..=5).contains(&self.max_connections) {
            return Err(Error::Invalid(
                "max_connections must be between 1 and 5".into(),
            ));
        }
        Ok(PgPoolOptions::new()
            .min_connections(0)
            .max_connections(self.max_connections)
            .acquire_timeout(self.acquire_timeout)
            .idle_timeout(Duration::from_secs(60))
            .max_lifetime(Duration::from_secs(300))
            .connect(&self.database_url)
            .await?)
    }
    pub async fn migrate(&self) -> Result<()> {
        migrate(&self.database_url_unpooled).await
    }
}

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
pub async fn migrate(unpooled_url: &str) -> Result<()> {
    let mut conn = PgConnection::connect(unpooled_url).await?;
    MIGRATOR.run(&mut conn).await?;
    conn.close().await?;
    Ok(())
}

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}
impl Database {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}
