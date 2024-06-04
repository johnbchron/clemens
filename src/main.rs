use std::{
  collections::{HashMap, HashSet},
  ops::Deref,
  str::FromStr,
  sync::Arc,
};

use color_eyre::eyre::{Context, Result};
use indicatif::{ParallelProgressIterator, ProgressIterator};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use solana_client::{
  rpc_client::RpcClient,
  rpc_config::{RpcBlockConfig, RpcTransactionConfig},
};
use solana_sdk::{
  commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature,
};
use solana_transaction_status::{
  option_serializer::OptionSerializer,
  EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction,
  UiConfirmedBlock, UiMessage, UiTransactionEncoding,
};

fn option_ser_to_option<T>(input: OptionSerializer<T>) -> Option<T> {
  match input {
    OptionSerializer::Some(inner) => Some(inner),
    _ => None,
  }
}

fn hash_from_encoded_transaction(
  txn: &EncodedTransaction,
) -> Option<Signature> {
  match txn {
    EncodedTransaction::LegacyBinary(a) => {
      EncodedTransaction::LegacyBinary(a.to_owned())
        .decode()
        .map(|t| t.signatures[0])
    }
    EncodedTransaction::Binary(a, b) => {
      EncodedTransaction::Binary(a.to_owned(), *b)
        .decode()
        .map(|t| t.signatures[0])
    }
    EncodedTransaction::Json(d) => Some(
      Signature::from_str(&d.signatures[0])
        .expect("failed to parse signature from signature list"),
    ),
    EncodedTransaction::Accounts(a) => Some(
      Signature::from_str(&a.signatures[0])
        .expect("failed to parse signature from signature list"),
    ),
  }
}

fn account_list_from_encoded_transaction(
  txn: &EncodedTransaction,
) -> Vec<Pubkey> {
  match txn {
    EncodedTransaction::LegacyBinary(a) => {
      EncodedTransaction::LegacyBinary(a.to_owned())
        .decode()
        .map(|t| t.message.static_account_keys().to_vec())
        .unwrap_or_default()
    }
    EncodedTransaction::Binary(a, b) => {
      EncodedTransaction::Binary(a.to_owned(), *b)
        .decode()
        .map(|t| t.message.static_account_keys().to_vec())
        .unwrap_or_default()
    }
    EncodedTransaction::Json(d) => match d.message.clone() {
      UiMessage::Parsed(a) => a
        .account_keys
        .iter()
        .map(|k| {
          Pubkey::from_str(&k.pubkey)
            .expect("failed to parse pubkey from account list")
        })
        .collect::<Vec<_>>(),
      UiMessage::Raw(a) => a
        .account_keys
        .iter()
        .map(|k| {
          Pubkey::from_str(&k)
            .expect("failed to parse pubkey from account list")
        })
        .collect::<Vec<_>>(),
    },
    EncodedTransaction::Accounts(a) => a
      .account_keys
      .iter()
      .map(|k| {
        Pubkey::from_str(&k.pubkey)
          .expect("failed to parse pubkey from account list")
      })
      .collect::<Vec<_>>(),
  }
}

fn fee_payer_from_encoded_transaction(
  txn: &EncodedTransaction,
) -> Option<Pubkey> {
  account_list_from_encoded_transaction(txn).first().cloned()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnalyzedTransaction {
  pub txn_hash:                 String,
  pub block:                    u64,
  pub fee_payer:                String,
  pub bought_tokens:            HashMap<String, u64>,
  pub sold_tokens:              HashMap<String, u64>,
  pub gas_fee:                  u64,
  pub native_delta:             i128,
  pub native_delta_without_gas: i128,
  pub success:                  bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
pub enum AnalysisError {
  #[error("The fee payer could not be identified.")]
  UnableToIdentifyFeePayer,
  #[error("The transaction hash could not be extracted.")]
  UnableToExtractHash,
  #[error("The transaction metadata was not provided by the RPC node.")]
  MissingTransactionMeta,
  #[error("The pre token balances were not provided by the RPC node.")]
  MissingPreTokenBalancesList,
  #[error("The post token balances were not provided by the RPC node.")]
  MissingPostTokenBalancesList,
}

impl TryFrom<EncodedConfirmedTransactionWithStatusMeta>
  for AnalyzedTransaction
{
  type Error = AnalysisError;
  fn try_from(
    txn: EncodedConfirmedTransactionWithStatusMeta,
  ) -> Result<Self, Self::Error> {
    let meta = txn
      .transaction
      .meta
      .ok_or(AnalysisError::MissingTransactionMeta)?;

    let txn_hash = hash_from_encoded_transaction(&txn.transaction.transaction)
      .ok_or(AnalysisError::UnableToExtractHash)?;
    let block = txn.slot;
    let fee_payer =
      fee_payer_from_encoded_transaction(&txn.transaction.transaction)
        .ok_or(AnalysisError::UnableToIdentifyFeePayer)?;
    let gas_fee = meta.fee;
    let success = meta.status.is_ok();

    let pre_native_balance = meta.pre_balances[0];
    let post_native_balance = meta.post_balances[0];
    let native_delta = post_native_balance as i128 - pre_native_balance as i128;
    let native_delta_without_gas = native_delta + gas_fee as i128;

    let all_pre_balances = option_ser_to_option(meta.pre_token_balances)
      .ok_or(AnalysisError::MissingPreTokenBalancesList)?;
    let all_post_balances = option_ser_to_option(meta.post_token_balances)
      .ok_or(AnalysisError::MissingPostTokenBalancesList)?;

    let pre_balances = all_pre_balances.into_iter().filter(|b| {
      matches!(
        option_ser_to_option(b.owner.clone()).map(|p| Pubkey::from_str(&p).expect("could not convert token owner string to `Pubkey`")),
        Some(owner) if owner == fee_payer
      )
    }).map(|b| (Pubkey::from_str(&b.mint).expect("could not convert token mint key string to `Pubkey`"), b.ui_token_amount.amount.parse::<u64>().expect("failed to parse token amount as `u64`"))).collect::<HashMap<_, _>>();
    let post_balances = all_post_balances.into_iter().filter(|b| {
      matches!(
        option_ser_to_option(b.owner.clone()).map(|p| Pubkey::from_str(&p).expect("could not convert token owner string to `Pubkey`")),
        Some(owner) if owner == fee_payer
      )
    }).map(|b| (Pubkey::from_str(&b.mint).expect("could not convert token mint key string to `Pubkey`"), b.ui_token_amount.amount.parse::<u64>().expect("failed to parse token amount as `u64`"))).collect::<HashMap<_, _>>();

    let pre_keys = pre_balances.keys().cloned().collect::<HashSet<_>>();
    let post_keys = post_balances.keys().cloned().collect::<HashSet<_>>();

    let mut bought_tokens = HashMap::new();
    let mut sold_tokens = HashMap::new();

    // tokens in both lists
    for key in pre_keys.intersection(&post_keys) {
      let pre = *pre_balances.get(key).unwrap() as i128;
      let post = *post_balances.get(key).unwrap() as i128;

      let difference = post - pre;
      match difference.cmp(&0) {
        std::cmp::Ordering::Less => {
          sold_tokens.insert(key.to_string(), difference.unsigned_abs() as u64);
        }
        std::cmp::Ordering::Equal => continue,
        std::cmp::Ordering::Greater => {
          bought_tokens.insert(key.to_string(), difference as u64);
        }
      }
    }

    // tokens only in the "pre" list, i.e. completely sold during txn
    for key in pre_keys.difference(&post_keys) {
      sold_tokens.insert(key.to_string(), *pre_balances.get(key).unwrap());
    }

    // tokens only in the "pre" list, i.e. the account had none before the txn
    for key in post_keys.difference(&pre_keys) {
      bought_tokens.insert(key.to_string(), *post_balances.get(key).unwrap());
    }

    Ok(AnalyzedTransaction {
      txn_hash: txn_hash.to_string(),
      block,
      fee_payer: fee_payer.to_string(),
      bought_tokens,
      sold_tokens,
      gas_fee,
      native_delta,
      native_delta_without_gas,
      success,
    })
  }
}

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

fn main() -> color_eyre::eyre::Result<()> {
  color_eyre::install()?;

  let rpc_url = std::env::var("RPC_URL").expect("failed to get RPC url");
  let client = Arc::new(RpcClient::new_with_commitment(
    rpc_url.to_string(),
    CommitmentConfig::confirmed(),
  ));

  let raydium_address =
    Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8")
      .expect("failed to create raydium pubkey");
  // println!("fetching recent signatures...");
  // let mut signatures = client
  //   .get_signatures_for_address(&raydium_address)
  //   .expect("failed to get signatures")
  //   .into_iter()
  //   .filter(|s| s.err.is_none())
  //   .collect::<Vec<_>>();
  // let signatures = signatures.split_off(signatures.len() - 100);
  // println!("finished fetching recent signatures.");

  // let transactions = signatures
  //   .par_iter()
  //   .progress_count(signatures.len() as u64)
  //   .filter_map(|s| {
  //     fetch_txn_from_signature(
  //       &Signature::from_str(&s.signature).unwrap(),
  //       client.clone(),
  //     )
  //     .ok()
  //   })
  //   .collect::<Vec<_>>();
  // println!("finished fetching full transactions.");

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
      account_list_from_encoded_transaction(&t.transaction.transaction)
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

  std::fs::write("analysis.txt", format!("{analysis:#?}")).unwrap();
  std::fs::write(
    "analysis.json",
    serde_json::to_string(&analysis).wrap_err("failed to serialize to json")?,
  )
  .unwrap();

  Ok(())
}
