pub mod analysis;
pub mod helpers;

use std::{ops::Deref, str::FromStr, sync::Arc};

use color_eyre::eyre::{Context, Result};
use indicatif::{ParallelProgressIterator, ProgressIterator};
use rayon::prelude::*;
use solana_client::{rpc_client::RpcClient, rpc_config::RpcBlockConfig};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use solana_transaction_status::{
  EncodedConfirmedTransactionWithStatusMeta, UiConfirmedBlock,
};

use crate::analysis::AnalyzedTransaction;

fn fetch_block_from_signature(
  slot: u64,
  client: impl Deref<Target = RpcClient>,
) -> Result<UiConfirmedBlock> {
  let cache_path = format!("/tmp/block_{}", slot);
  if let Ok(cached_file) = std::fs::read(&cache_path) {
    if let Ok(parsed_txn) =
      serde_json::from_slice::<UiConfirmedBlock>(&cached_file)
    {
      return Ok(parsed_txn);
    }
  }

  let block = client
    .get_block_with_config(slot, RpcBlockConfig {
      max_supported_transaction_version: Some(0),
      ..Default::default()
    })
    .wrap_err("failed to fetch block")?;

  std::fs::write(
    &cache_path,
    serde_json::to_string(&block).wrap_err("failed to serialize block")?,
  )
  .wrap_err("failed to write to cache")?;

  Ok(block)
}

#[tokio::main]
async fn main() -> color_eyre::eyre::Result<()> {
  color_eyre::install()?;
  let filter = tracing_subscriber::EnvFilter::try_from_default_env()
    .unwrap_or(tracing_subscriber::EnvFilter::new("clemens=debug"));
  tracing_subscriber::fmt().with_env_filter(filter).init();

  let rpc_url = std::env::var("RPC_URL").expect("failed to get RPC url");
  let client = Arc::new(RpcClient::new_with_commitment(
    rpc_url.to_string(),
    CommitmentConfig::confirmed(),
  ));

  let raydium_address =
    Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8")
      .expect("failed to create raydium pubkey");

  let current_block = client.get_block_height().unwrap();
  print!("fetching block slots...");
  let block_slots = client
    .get_blocks(current_block - 500, Some(current_block))
    .unwrap();
  println!(" done.");
  println!("fetching blocks...");
  let blocks = block_slots
    .iter()
    .progress_count(block_slots.len() as _)
    .map(|s| fetch_block_from_signature(*s, client.clone()).unwrap())
    .collect::<Vec<_>>();
  println!("finished fetching blocks");

  println!("filtering transactions...");
  let transactions = blocks
    .into_iter()
    .flat_map(|b| {
      b.transactions.unwrap().into_iter().map(move |t| {
        EncodedConfirmedTransactionWithStatusMeta {
          slot:        b.block_height.unwrap(),
          transaction: t,
          block_time:  b.block_time,
        }
      })
    })
    .collect::<Vec<_>>()
    .into_par_iter()
    .progress()
    .filter(|t| {
      crate::helpers::account_list_from_encoded_transaction(
        &t.transaction.transaction,
      )
      .into_iter()
      .position(|p| p == raydium_address)
      .is_some()
    })
    .collect::<Vec<_>>();
  println!("finished filtering transactions.");

  let len = transactions.len() as u64;
  println!("analyzing transactions...");
  let analysis = transactions
    .into_par_iter()
    .progress_count(len)
    .filter_map(|t| AnalyzedTransaction::try_from(t).ok())
    .filter(|a| {
      a.success && (!a.bought_tokens.is_empty() || !a.sold_tokens.is_empty())
    })
    .collect::<Vec<_>>();
  println!("finished analysis");

  std::fs::write(
    "analysis.json",
    serde_json::to_string(&analysis).wrap_err("failed to serialize to json")?,
  )
  .unwrap();

  Ok(())
}
