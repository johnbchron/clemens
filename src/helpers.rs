use std::str::FromStr;

use solana_sdk::{pubkey::Pubkey, signature::Signature};
use solana_transaction_status::{
  option_serializer::OptionSerializer, EncodedTransaction, UiMessage,
};

pub fn option_ser_to_option<T>(input: OptionSerializer<T>) -> Option<T> {
  match input {
    OptionSerializer::Some(inner) => Some(inner),
    _ => None,
  }
}

pub fn hash_from_encoded_transaction(
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

pub fn account_list_from_encoded_transaction(
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

pub fn fee_payer_from_encoded_transaction(
  txn: &EncodedTransaction,
) -> Option<Pubkey> {
  account_list_from_encoded_transaction(txn).first().cloned()
}
