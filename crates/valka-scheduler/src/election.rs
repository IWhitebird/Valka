use sqlx::PgPool;
use tracing::info;

const ADVISORY_LOCK_ID: i64 = 0x56414C4B41; // "VALKA" in hex

/// PG advisory lock-based leader election for the scheduler.
pub struct SchedulerElection {
    pool: PgPool,
    is_leader: bool,
}

impl SchedulerElection {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            is_leader: false,
        }
    }

    /// Try to acquire the advisory lock. Non-blocking.
    pub async fn try_acquire(&mut self) -> Result<bool, sqlx::Error> {
        let row: (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
            .bind(ADVISORY_LOCK_ID)
            .fetch_one(&self.pool)
            .await?;

        self.is_leader = row.0;
        if self.is_leader {
            info!("Acquired scheduler leadership");
        }
        Ok(self.is_leader)
    }

    /// Release the advisory lock.
    pub async fn release(&mut self) -> Result<(), sqlx::Error> {
        if self.is_leader {
            sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(ADVISORY_LOCK_ID)
                .execute(&self.pool)
                .await?;
            self.is_leader = false;
            info!("Released scheduler leadership");
        }
        Ok(())
    }

    pub fn is_leader(&self) -> bool {
        self.is_leader
    }
}
