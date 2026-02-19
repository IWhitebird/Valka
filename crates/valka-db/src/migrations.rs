use sqlx::{Connection, PgConnection};
use tracing::info;

/// Runs database migrations using a direct connection.
///
/// Uses a single `PgConnection` (not a pool) so this works correctly even
/// when the runtime pool goes through PgBouncer in transaction mode.
/// In cluster deployments, this should be called by a single init service
/// while server nodes set `skip_migrations = true`.
pub async fn run_migrations(database_url: &str) -> Result<(), sqlx::Error> {
    info!("Connecting to database for migrations...");
    let mut conn = PgConnection::connect(database_url).await?;

    info!("Running database migrations...");
    sqlx::migrate!("./migrations")
        .run(&mut conn)
        .await
        .map_err(|e| -> sqlx::Error { e.into() })?;

    info!("Database migrations completed");
    Ok(())
}
