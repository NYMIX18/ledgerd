//! Integer-only money. There is deliberately no `f64` anywhere in this module.
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    Kes,
    Usd,
    Eur,
}

impl Currency {
    pub fn as_str(&self) -> &'static str {
        match self {
            Currency::Kes => "KES",
            Currency::Usd => "USD",
            Currency::Eur => "EUR",
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Currency {
    type Err = MoneyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "KES" => Ok(Currency::Kes),
            "USD" => Ok(Currency::Usd),
            "EUR" => Ok(Currency::Eur),
            other => Err(MoneyError::UnknownCurrency(other.to_string())),
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    #[error("unknown currency '{0}'")]
    UnknownCurrency(String),
    #[error("currency mismatch: {0} vs {1}")]
    CurrencyMismatch(Currency, Currency),
    #[error("amount must not be negative")]
    Negative,
    #[error("arithmetic overflow")]
    Overflow,
}

/// A non-negative amount in minor units (cents) tagged with its currency.
/// Mixing currencies is a compile-time-visible error path, not a silent bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money {
    minor: i64,
    currency: Currency,
}

impl Money {
    pub fn new(minor: i64, currency: Currency) -> Result<Self, MoneyError> {
        if minor < 0 {
            return Err(MoneyError::Negative);
        }
        Ok(Self { minor, currency })
    }

    pub fn minor(&self) -> i64 {
        self.minor
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    pub fn checked_add(self, other: Money) -> Result<Money, MoneyError> {
        self.same_currency(other)?;
        let minor = self.minor.checked_add(other.minor).ok_or(MoneyError::Overflow)?;
        Ok(Money { minor, ..self })
    }

    pub fn checked_sub(self, other: Money) -> Result<Money, MoneyError> {
        self.same_currency(other)?;
        let minor = self.minor.checked_sub(other.minor).ok_or(MoneyError::Overflow)?;
        Money::new(minor, self.currency)
    }

    fn same_currency(&self, other: Money) -> Result<(), MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, other.currency));
        }
        Ok(())
    }
}

impl fmt::Display for Money {
    /// `KES 1,234.50`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let major = (self.minor / 100).to_string();
        let mut grouped = String::new();
        for (i, ch) in major.chars().enumerate() {
            if i > 0 && (major.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        write!(f, "{} {}.{:02}", self.currency, grouped, self.minor % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_negative() {
        assert_eq!(Money::new(-1, Currency::Kes), Err(MoneyError::Negative));
    }

    #[test]
    fn adds_same_currency() {
        let a = Money::new(150, Currency::Kes).unwrap();
        let b = Money::new(250, Currency::Kes).unwrap();
        assert_eq!(a.checked_add(b).unwrap().minor(), 400);
    }

    #[test]
    fn refuses_to_mix_currencies() {
        let a = Money::new(1, Currency::Kes).unwrap();
        let b = Money::new(1, Currency::Usd).unwrap();
        assert_eq!(
            a.checked_add(b),
            Err(MoneyError::CurrencyMismatch(Currency::Kes, Currency::Usd))
        );
    }

    #[test]
    fn detects_overflow_and_underflow() {
        let max = Money::new(i64::MAX, Currency::Usd).unwrap();
        let one = Money::new(1, Currency::Usd).unwrap();
        assert_eq!(max.checked_add(one), Err(MoneyError::Overflow));
        assert_eq!(one.checked_sub(max), Err(MoneyError::Negative));
    }

    #[test]
    fn formats_with_grouping() {
        assert_eq!(Money::new(123_450, Currency::Kes).unwrap().to_string(), "KES 1,234.50");
        assert_eq!(Money::new(5, Currency::Usd).unwrap().to_string(), "USD 0.05");
        assert_eq!(
            Money::new(100_000_000, Currency::Eur).unwrap().to_string(),
            "EUR 1,000,000.00"
        );
    }

    #[test]
    fn currency_round_trips() {
        for c in [Currency::Kes, Currency::Usd, Currency::Eur] {
            assert_eq!(c.as_str().parse::<Currency>().unwrap(), c);
        }
        assert!("XXX".parse::<Currency>().is_err());
    }
}
