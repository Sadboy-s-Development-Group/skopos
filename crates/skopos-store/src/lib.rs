use chrono::{DateTime, Utc};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::{path::Path, str::FromStr};

mod network;
mod usage;

pub use network::{NetworkOutage, NetworkSample, NetworkStatusCounts};
pub use usage::{InsertUsageResult, UpsertOutcome, UsageModelTotal, UsageTotals};

#[derive(Clone)]
pub struct SkoposStore {
    pool: SqlitePool,
}

impl SkoposStore {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await?;

        Ok(Self { pool })
    }

    pub async fn connect_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(&path.as_ref().to_string_lossy())?
                .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn migrate(&self) -> anyhow::Result<()> {
        const MIGRATIONS: &[&str] = &[
            include_str!("../migrations/001_initial.sql"),
            include_str!("../migrations/002_network.sql"),
        ];
        for migration in MIGRATIONS {
            for statement in migration.split(';') {
                let statement = statement.trim();
                if !statement.is_empty() {
                    sqlx::query(statement).execute(&self.pool).await?;
                }
            }
        }

        Ok(())
    }
}

/// Parse an RFC 3339 timestamp into a UTC `DateTime`. Shared by the row
/// decoders in the `usage` and `network` modules.
pub(crate) fn parse_rfc3339(raw: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(raw)?.with_timezone(&Utc))
}
