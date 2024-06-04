use std::{
  collections::{HashMap, HashSet},
  str::FromStr,
};

use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta;

use crate::helpers::{
  fee_payer_from_encoded_transaction, hash_from_encoded_transaction,
  option_ser_to_option,
};

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

  #[tracing::instrument]
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

    let pre_balances = all_pre_balances
      .into_iter()
      .filter(|b| {
        option_ser_to_option(b.owner.clone()).map(|p| {
          Pubkey::from_str(&p)
            .expect("could not convert token owner string to `Pubkey`")
        }) == Some(fee_payer)
      })
      .map(|b| {
        (
          Pubkey::from_str(&b.mint)
            .expect("could not convert token mint key string to `Pubkey`"),
          b.ui_token_amount
            .amount
            .parse::<u64>()
            .expect("failed to parse token amount as `u64`"),
        )
      })
      .collect::<HashMap<_, _>>();
    let post_balances = all_post_balances
      .into_iter()
      .filter(|b| {
        option_ser_to_option(b.owner.clone()).map(|p| {
          Pubkey::from_str(&p)
            .expect("could not convert token owner string to `Pubkey`")
        }) == Some(fee_payer)
      })
      .map(|b| {
        (
          Pubkey::from_str(&b.mint)
            .expect("could not convert token mint key string to `Pubkey`"),
          b.ui_token_amount
            .amount
            .parse::<u64>()
            .expect("failed to parse token amount as `u64`"),
        )
      })
      .collect::<HashMap<_, _>>();

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
