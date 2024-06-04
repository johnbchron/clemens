use std::ops::Deref;

use color_eyre::eyre::{Context, Result};
use sqlx::postgres::PgPoolOptions;

use crate::analysis::AnalyzedTransaction;

pub async fn connect_to_db() -> Result<sqlx::postgres::PgPool> {
  let db_url = std::env::var("DATABASE_URL").expect("failed to get DB url");
  let pool = PgPoolOptions::new()
    .max_connections(5)
    .connect(&db_url)
    .await
    .wrap_err("failed to connect to DB")?;

  Ok(pool)
}

pub async fn migrate(
  client: impl Deref<Target = sqlx::postgres::PgPool>,
) -> Result<()> {
  sqlx::query!(
    r#"
    CREATE TABLE IF NOT EXISTS analyzed_transactions (
      txn_hash VARCHAR PRIMARY KEY,
      block VARCHAR,
      fee_payer VARCHAR,
      bought_tokens JSONB,
      sold_tokens JSONB,
      gas_fee VARCHAR,
      native_delta VARCHAR,
      native_delta_without_gas VARCHAR,
      success BOOLEAN
    );
  "#
  )
  .execute(&*client)
  .await?;
  sqlx::query!(
    r#"
    CREATE TABLE IF NOT EXISTS analyzed_slots (
      slot VARCHAR PRIMARY KEY
    );
  "#
  )
  .execute(&*client)
  .await?;
  tracing::info!("completed db migrations");

  Ok(())
}

pub async fn mark_slot_completed(
  client: impl Deref<Target = sqlx::postgres::PgPool>,
  slot: u64,
) -> Result<()> {
  sqlx::query!(
    r#"
      INSERT INTO analyzed_slots (slot) VALUES ($1)
    "#,
    slot.to_string(),
  )
  .execute(&*client)
  .await?;
  Ok(())
}

pub async fn write_transactions(
  client: impl Deref<Target = sqlx::postgres::PgPool>,
  atxns: Vec<AnalyzedTransaction>,
) -> Result<()> {
  let len = atxns.len();
  let mut db_tx = client.begin().await?;

  for txn in atxns {
    sqlx::query!(
      r#"
        INSERT INTO analyzed_transactions (
            txn_hash, block, fee_payer, bought_tokens, sold_tokens, gas_fee, native_delta, native_delta_without_gas, success
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
      "#,
      txn.txn_hash,
      txn.block.to_string(),
      txn.fee_payer,
      sqlx::types::Json(&txn.bought_tokens) as _,
      sqlx::types::Json(&txn.sold_tokens) as _,
      txn.gas_fee.to_string(),
      txn.native_delta.to_string(),
      txn.native_delta_without_gas.to_string(),
      txn.success
    )
    .execute(&mut *db_tx)
    .await?;
  }
  db_tx.commit().await?;
  tracing::info!("wrote {len} transactions to db");

  Ok(())
}
