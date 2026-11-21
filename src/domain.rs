use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::money::Currency;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("description must be 1-500 characters")]
    BadDescription,
    #[error("a transaction needs between 2 and 64 entries")]
    BadEntryCount,
    #[error("entry amounts must be positive")]
    NonPositiveAmount,
    #[error("transaction is unbalanced: debits={debits} credits={credits}")]
    Unbalanced { debits: i128, credits: i128 },
    #[error("unknown account kind '{0}'")]
    UnknownKind(String),
    #[error("unknown direction '{0}'")]
    UnknownDirection(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Debit,
    Credit,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Debit => "debit",
            Direction::Credit => "credit",
        }
    }
}

impl FromStr for Direction {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "debit" => Ok(Direction::Debit),
            "credit" => Ok(Direction::Credit),
            o => Err(DomainError::UnknownDirection(o.into())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

impl AccountKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountKind::Asset => "asset",
            AccountKind::Liability => "liability",
            AccountKind::Equity => "equity",
            AccountKind::Revenue => "revenue",
            AccountKind::Expense => "expense",
        }
    }

    /// The side that *increases* this kind of account.
    pub fn normal_side(&self) -> Direction {
        match self {
            AccountKind::Asset | AccountKind::Expense => Direction::Debit,
            AccountKind::Liability | AccountKind::Equity | AccountKind::Revenue => Direction::Credit,
        }
    }
}

impl FromStr for AccountKind {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "asset" => Ok(AccountKind::Asset),
            "liability" => Ok(AccountKind::Liability),
            "equity" => Ok(AccountKind::Equity),
            "revenue" => Ok(AccountKind::Revenue),
            "expense" => Ok(AccountKind::Expense),
            o => Err(DomainError::UnknownKind(o.into())),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: AccountKind,
    pub currency: Currency,
    pub balance_minor: i64,
    pub allow_negative: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewEntry {
    pub account_id: Uuid,
    pub direction: Direction,
    pub amount_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTransaction {
    pub description: String,
    pub currency: Currency,
    pub entries: Vec<NewEntry>,
    #[serde(default)]
    pub external_ref: Option<String>,
}

impl NewTransaction {
    /// Pure, DB-free validation: shape, positivity and the double-entry invariant.
    pub fn validate(&self) -> Result<(), DomainError> {
        let len = self.description.trim().chars().count();
        if len == 0 || len > 500 {
            return Err(DomainError::BadDescription);
        }
        if self.entries.len() < 2 || self.entries.len() > 64 {
            return Err(DomainError::BadEntryCount);
        }
        let (mut debits, mut credits) = (0i128, 0i128);
        for e in &self.entries {
            if e.amount_minor <= 0 {
                return Err(DomainError::NonPositiveAmount);
            }
            match e.direction {
                Direction::Debit => debits += e.amount_minor as i128,
                Direction::Credit => credits += e.amount_minor as i128,
            }
        }
        if debits != credits {
            return Err(DomainError::Unbalanced { debits, credits });
        }
        Ok(())
    }

    /// Stable SHA-256 of the request, independent of entry order.
    /// Used to tell "same request retried" from "same key, different request".
    pub fn fingerprint(&self) -> String {
        let mut canon = self.clone();
        canon
            .entries
            .sort_by_key(|e| (e.account_id, e.direction, e.amount_minor));
        let bytes = serde_json::to_vec(&canon).expect("NewTransaction is always serializable");
        hex::encode(Sha256::digest(bytes))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryView {
    pub account_id: Uuid,
    pub direction: Direction,
    pub amount_minor: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionView {
    pub id: Uuid,
    pub idempotency_key: String,
    pub description: String,
    pub currency: Currency,
    pub external_ref: Option<String>,
    pub created_at: DateTime<Utc>,
    pub entries: Vec<EntryView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(acc: Uuid, d: Direction, amt: i64) -> NewEntry {
        NewEntry {
            account_id: acc,
            direction: d,
            amount_minor: amt,
        }
    }

    fn txn(entries: Vec<NewEntry>) -> NewTransaction {
        NewTransaction {
            description: "test".into(),
            currency: Currency::Kes,
            entries,
            external_ref: None,
        }
    }

    #[test]
    fn balanced_transaction_is_valid() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(
            txn(vec![entry(a, Direction::Debit, 100), entry(b, Direction::Credit, 100)])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn unbalanced_is_rejected() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let err = txn(vec![entry(a, Direction::Debit, 100), entry(b, Direction::Credit, 99)])
            .validate()
            .unwrap_err();
        assert_eq!(
            err,
            DomainError::Unbalanced {
                debits: 100,
                credits: 99
            }
        );
    }

    #[test]
    fn zero_and_negative_amounts_rejected() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(
            txn(vec![entry(a, Direction::Debit, 0), entry(b, Direction::Credit, 0)]).validate(),
            Err(DomainError::NonPositiveAmount)
        );
        assert_eq!(
            txn(vec![entry(a, Direction::Debit, -5), entry(b, Direction::Credit, -5)]).validate(),
            Err(DomainError::NonPositiveAmount)
        );
    }

    #[test]
    fn needs_at_least_two_entries() {
        let a = Uuid::new_v4();
        assert_eq!(
            txn(vec![entry(a, Direction::Debit, 1)]).validate(),
            Err(DomainError::BadEntryCount)
        );
    }

    #[test]
    fn no_i64_overflow_when_summing() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let t = txn(vec![
            entry(a, Direction::Debit, i64::MAX),
            entry(b, Direction::Debit, i64::MAX),
            entry(c, Direction::Credit, i64::MAX),
        ]);
        assert!(matches!(t.validate(), Err(DomainError::Unbalanced { .. })));
    }

    #[test]
    fn fingerprint_ignores_entry_order_but_not_content() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let t1 = txn(vec![entry(a, Direction::Debit, 100), entry(b, Direction::Credit, 100)]);
        let t2 = txn(vec![entry(b, Direction::Credit, 100), entry(a, Direction::Debit, 100)]);
        let t3 = txn(vec![entry(a, Direction::Debit, 101), entry(b, Direction::Credit, 101)]);
        assert_eq!(t1.fingerprint(), t2.fingerprint());
        assert_ne!(t1.fingerprint(), t3.fingerprint());
    }

    #[test]
    fn normal_sides() {
        assert_eq!(AccountKind::Asset.normal_side(), Direction::Debit);
        assert_eq!(AccountKind::Liability.normal_side(), Direction::Credit);
    }
}
