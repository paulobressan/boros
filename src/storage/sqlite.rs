use std::path::Path;

use anyhow::{Error, Result};
use chrono::Utc;
use sqlx::{sqlite::SqliteRow, FromRow, Row};

use super::{Transaction, TransactionStatus};

pub struct SqliteStorage {
    db: sqlx::sqlite::SqlitePool,
}

impl SqliteStorage {
    pub async fn new(path: &Path) -> Result<Self> {
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let db = sqlx::sqlite::SqlitePoolOptions::new().connect(&url).await?;

        Ok(Self { db })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("src/storage/migrations")
            .run(&self.db)
            .await?;

        Ok(())
    }

    #[cfg(test)]
    pub async fn ephemeral() -> Result<Self> {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await?;

        let out = Self { db };
        out.migrate().await?;

        Ok(out)
    }
}

impl FromRow<'_, SqliteRow> for Transaction {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        let status: &str = row.try_get("status")?;
        let priority: u32 = row.try_get("priority")?;

        Ok(Self {
            id: row.try_get("id")?,
            raw: row.try_get("raw")?,
            status: status
                .parse()
                .map_err(|err: Error| sqlx::Error::Decode(err.into()))?,
            priority: priority
                .try_into()
                .map_err(|err: Error| sqlx::Error::Decode(err.into()))?,

            dependencies: None,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

pub struct SqliteTransaction {
    sqlite: SqliteStorage,
}

impl SqliteTransaction {
    pub fn new(sqlite: SqliteStorage) -> Self {
        Self { sqlite }
    }

    pub async fn create(&self, txs: &Vec<Transaction>) -> Result<()> {
        let mut db_tx = self.sqlite.db.begin().await?;

        for tx in txs {
            let status = tx.status.clone().to_string();
            let priority: u32 = tx.priority.clone().try_into()?;

            sqlx::query!(
                r#"
                    INSERT INTO tx (
                        id,
                        raw,
                        status,
                        priority,
                        created_at,
                        updated_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                "#,
                tx.id,
                tx.raw,
                status,
                priority,
                tx.created_at,
                tx.updated_at
            )
            .execute(&mut *db_tx)
            .await?;

            if let Some(dependencies) = &tx.dependencies {
                for required_id in dependencies {
                    sqlx::query!(
                        r#"
                            INSERT INTO tx_dependence (
                                dependent_id,
                                required_id
                            )
                            VALUES ($1, $2)
                        "#,
                        tx.id,
                        required_id,
                    )
                    .execute(&mut *db_tx)
                    .await?;
                }
            }
        }

        db_tx.commit().await?;
        Ok(())
    }

    async fn next(&self, status: TransactionStatus) -> Result<Option<Transaction>> {
        let transaction = sqlx::query_as::<_, Transaction>(
            r#"
                    SELECT
                    	id,
                    	raw,
                    	status,
                    	priority,
                    	created_at,
                    	updated_at
                    FROM
                    	tx
                    WHERE
                    	tx.status = $1
                    ORDER BY
                    	priority,
                    	created_at ASC
                    LIMIT 1;
            "#,
        )
        .bind(status.to_string())
        .fetch_optional(&self.sqlite.db)
        .await?;

        Ok(transaction)
    }

    async fn update(&self, tx: &Transaction) -> Result<()> {
        let status = tx.status.to_string();
        let updated_at = Utc::now();

        sqlx::query!(
            r#"
                UPDATE
                	tx
                SET
                	raw = $1,
                	status = $2,
                	updated_at = $3
                WHERE
                	id = $4;
            "#,
            tx.raw,
            status,
            updated_at,
            tx.id,
        )
        .execute(&self.sqlite.db)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::{Transaction, TransactionStatus};

    use super::{SqliteStorage, SqliteTransaction};

    async fn mock_sqlite() -> SqliteTransaction {
        let sqlite_storage = SqliteStorage::ephemeral().await.unwrap();
        SqliteTransaction::new(sqlite_storage)
    }

    #[tokio::test]
    async fn it_should_create_tx() {
        let storage = mock_sqlite().await;
        let transaction = Transaction::default();

        let result = storage.create(&vec![transaction]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn it_should_create_tx_with_dependencies() {
        let storage = mock_sqlite().await;
        let mut transaction_1 = Transaction::default();
        transaction_1.id = "hex1".into();

        let mut transaction_2 = Transaction::default();
        transaction_2.id = "hex2".into();
        transaction_2.dependencies = Some(vec![transaction_1.id.clone()]);

        let result = storage.create(&vec![transaction_1, transaction_2]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn it_should_fail_create_tx_with_invalid_dependencies() {
        let storage = mock_sqlite().await;

        let mut transaction = Transaction::default();
        transaction.dependencies = Some(vec!["something".into()]);

        let result = storage.create(&vec![transaction]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn it_should_find_next_transaction() {
        let storage = mock_sqlite().await;
        let transaction = Transaction::default();

        storage.create(&vec![transaction]).await.unwrap();

        let result = storage.next(TransactionStatus::Pending).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[tokio::test]
    async fn it_should_update_transaction_valid() {
        let storage = mock_sqlite().await;

        let transaction = Transaction::default();
        storage.create(&vec![transaction]).await.unwrap();

        let mut transaction = Transaction::default();
        transaction.status = TransactionStatus::Validated;
        let result = storage.update(&transaction).await;
        assert!(result.is_ok());

        let result = storage.next(TransactionStatus::Validated).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }
}
