pub mod analysis;
pub mod db;
pub mod helpers;

use std::{ops::Deref, sync::Arc, time::Duration};

use color_eyre::eyre::{Context, Error, Result};
use rayon::prelude::*;
use solana_client::{
  nonblocking::rpc_client::RpcClient, pubsub_client::PubsubClient,
  rpc_config::RpcBlockConfig, rpc_response::SlotUpdate,
};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_transaction_status::UiConfirmedBlock;

use crate::analysis::AnalyzedTransaction;

async fn fetch_block_with_retry(
  client: impl Deref<Target = RpcClient>,
  slot: u64,
) -> Result<UiConfirmedBlock> {
  let mut retries = 0;
  loop {
    match client
      .get_block_with_config(slot, RpcBlockConfig {
        commitment: Some(CommitmentConfig {
          commitment: solana_sdk::commitment_config::CommitmentLevel::Confirmed,
        }),
        max_supported_transaction_version: Some(0),
        ..Default::default()
      })
      .await
    {
      Ok(block) => {
        tracing::debug!("fetched new block: {slot}");
        return Ok(block);
      }
      Err(e) => {
        if retries < 5 {
          retries += 1;
          tracing::warn!("failed to fetch block {slot}... retrying");
          tokio::time::sleep(Duration::from_secs(5)).await;
          continue;
        } else {
          tracing::error!("failed to fetch block {slot}... too many retries");
          Err(Error::from(e).wrap_err(format!("failed to fetch block {slot}")))?
        }
      }
    }
  }
}

#[tokio::main]
async fn main() -> color_eyre::eyre::Result<()> {
  // setup logging and tracing
  color_eyre::install()?;
  let filter = tracing_subscriber::EnvFilter::try_from_default_env()
    .unwrap_or(tracing_subscriber::EnvFilter::new("clemens=debug"));
  tracing_subscriber::fmt().with_env_filter(filter).init();

  // start up the database client and run migrations
  let db_client = Arc::new(crate::db::connect_to_db().await?);
  crate::db::migrate(db_client.clone()).await?;

  // open a channel through which we send back analyzed transactions
  let (atxn_tx, mut atxn_rx) =
    tokio::sync::mpsc::channel::<AnalyzedTransaction>(10000);

  // spawn a task for receiving slots (blocks) and spawning tasks to handle them
  tokio::spawn({
    let db_client = db_client.clone();
    async move {
      let ws_url = std::env::var("WS_URL").expect("failed to get WS url");
      let rpc_url = std::env::var("RPC_URL").expect("failed to get RPC url");

      // make a channel for receiving slots
      let (slot_tx, mut slot_rx) = tokio::sync::mpsc::channel::<u64>(100);

      // this handler is for the subscriber to call, and it sends back updated
      // slots through the channel
      let handler = move |su: SlotUpdate| match su {
        SlotUpdate::Frozen { slot, .. } => {
          futures::executor::block_on(slot_tx.send(slot)).unwrap()
        }
        _ => (),
      };

      // build the slot update subscription
      let _subscription =
        PubsubClient::slot_updates_subscribe(&ws_url, handler)
          .wrap_err("failed to subscribe to block updates")?;
      tracing::info!("waiting for slot updates...");

      // start the RPC client
      let client = Arc::new(RpcClient::new(rpc_url));

      // continually receive blocks from the channel
      while let Some(slot) = slot_rx.recv().await {
        tracing::trace!("notified of newly completed slot: {slot}");
        let (atxn_tx, client) = (atxn_tx.clone(), client.clone());

        // spawn a task for each block, to fetch it and analyze its transactions
        tokio::spawn({
          let db_client = db_client.clone();
          async move {
            let result = async move {
              tokio::time::sleep(Duration::from_secs(20)).await;

              let block = fetch_block_with_retry(client, slot).await?;

              for atxn in crate::helpers::transactions_from_block(block)
                .into_par_iter()
                .filter_map(|t| AnalyzedTransaction::try_from(t).ok())
                .collect::<Vec<_>>()
                .into_iter()
                .filter(|a| {
                  a.success
                    && (!a.bought_tokens.is_empty()
                      || !a.sold_tokens.is_empty())
                })
              {
                atxn_tx
                  .send(atxn)
                  .await
                  .wrap_err("failed to send transaction to channel")?;
              }
              crate::db::mark_slot_completed(db_client, slot)
                .await
                .wrap_err("failed to mark slot completed in db")?;

              Ok::<(), Error>(())
            }
            .await;
            match result {
              Err(e) => {
                tracing::error!("{e:#}");
              }
              _ => (),
            };
          }
        });
      }

      #[allow(unreachable_code)]
      Ok::<(), Error>(())
    }
  });

  let mut pool = Vec::with_capacity(100);
  while let Some(atxn) = atxn_rx.recv().await {
    pool.push(atxn);
    if pool.len() == 100 {
      tokio::spawn({
        let db_client = db_client.clone();
        let pool = pool.clone();
        async move { crate::db::write_transactions(db_client, pool.clone()).await }
      });
      pool.clear();
    }
  }

  // let client = Arc::new(RpcClient::new_with_commitment(
  //   rpc_url.to_string(),
  //   CommitmentConfig::confirmed(),
  // ));

  // let raydium_address =
  //   Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8")
  //     .expect("failed to create raydium pubkey");

  // let current_block = client.get_block_height().unwrap();
  // print!("fetching block slots...");
  // let block_slots = client
  //   .get_blocks(current_block - 5, Some(current_block))
  //   .unwrap();
  // println!(" done.");
  // println!("fetching blocks...");
  // let blocks = block_slots
  //   .iter()
  //   .progress_count(block_slots.len() as _)
  //   .map(|s| fetch_block_from_signature(*s, client.clone()).unwrap())
  //   .collect::<Vec<_>>();
  // println!("finished fetching blocks");

  // println!("filtering transactions...");
  // let transactions = blocks
  //   .into_iter()
  //   .flat_map(crate::helpers::transactions_from_block)
  //   .collect::<Vec<_>>()
  //   .into_par_iter()
  //   .progress()
  //   .filter(|t| {
  //     crate::helpers::account_list_from_encoded_transaction(
  //       &t.transaction.transaction,
  //     )
  //     .into_iter()
  //     .position(|p| p == raydium_address)
  //     .is_some()
  //   })
  //   .collect::<Vec<_>>();
  // println!("finished filtering transactions.");

  // let len = transactions.len() as u64;
  // println!("analyzing transactions...");
  // let analysis = transactions
  //   .into_par_iter()
  //   .progress_count(len)
  //   .filter_map(|t| AnalyzedTransaction::try_from(t).ok())
  //   .filter(|a| {
  //     a.success && (!a.bought_tokens.is_empty() || !a.sold_tokens.is_empty())
  //   })
  //   .collect::<Vec<_>>();
  // println!("finished analysis");

  // std::fs::write(
  //   "analysis.json",
  //   serde_json::to_string(&analysis).wrap_err("failed to serialize to
  // json")?, )
  // .unwrap();

  Ok(())
}
