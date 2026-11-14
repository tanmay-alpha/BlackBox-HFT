use crate::repositories::limit;
use crate::*;
use sqlx::{Postgres, Transaction};

/// A small locked deletion batch. Keep this transaction only for the bounded object-delete
/// calls, acknowledge successes/errors, then commit. Dropping rolls back all acknowledgements.
/// Row locks fence completion and concurrent collectors; callers must not delete objects from
/// the unlocked diagnostic `gc_due` snapshot.
pub struct GcBatch {
    transaction: Transaction<'static, Postgres>,
    rows: Vec<BlobGc>,
}
impl GcBatch {
    pub fn rows(&self) -> &[BlobGc] {
        &self.rows
    }
    pub async fn acknowledge(&mut self, backend: &str, key: &str) -> Result<()> {
        self.member(backend, key)?;
        sqlx::query("DELETE FROM bb.blob_gc WHERE storage_backend=$1 AND storage_key=$2")
            .bind(backend)
            .bind(key)
            .execute(&mut *self.transaction)
            .await?;
        Ok(())
    }
    pub async fn failed(&mut self, backend: &str, key: &str, message: &str) -> Result<()> {
        self.member(backend, key)?;
        sqlx::query("UPDATE bb.blob_gc SET attempts=attempts+1,last_error=$3,not_before=now()+interval '5 minutes' WHERE storage_backend=$1 AND storage_key=$2")
            .bind(backend).bind(key).bind(message).execute(&mut *self.transaction).await?;
        Ok(())
    }
    fn member(&self, backend: &str, key: &str) -> Result<()> {
        if self
            .rows
            .iter()
            .any(|r| r.storage_backend == backend && r.storage_key == key)
        {
            Ok(())
        } else {
            Err(Error::NotFound)
        }
    }
    pub async fn commit(self) -> Result<()> {
        self.transaction.commit().await?;
        Ok(())
    }
}
impl Database {
    pub async fn claim_gc(&self, count: i64) -> Result<GcBatch> {
        let mut transaction = self.pool.begin().await?;
        let rows=sqlx::query_as("SELECT g.* FROM bb.blob_gc g WHERE g.not_before<=now() AND NOT EXISTS (SELECT 1 FROM bb.run_artifacts a WHERE a.storage_backend=g.storage_backend AND a.storage_key=g.storage_key) ORDER BY g.not_before,g.storage_backend,g.storage_key LIMIT $1 FOR UPDATE OF g SKIP LOCKED")
            .bind(limit(count)?).fetch_all(&mut *transaction).await?;
        Ok(GcBatch { transaction, rows })
    }
}
