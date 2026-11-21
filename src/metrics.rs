use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};

/// Dependency-free Prometheus-style counters.
#[derive(Default)]
pub struct Metrics {
    pub transactions_posted: AtomicU64,
    pub transactions_replayed: AtomicU64,
    pub transactions_rejected: AtomicU64,
    pub callbacks_received: AtomicU64,
    pub callbacks_duplicate: AtomicU64,
    pub callbacks_processed: AtomicU64,
    pub callbacks_retried: AtomicU64,
    pub callbacks_dead: AtomicU64,
    pub reconcile_drifted_accounts: AtomicI64,
    pub reconcile_unbalanced_currencies: AtomicI64,
}

impl Metrics {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut line = |name: &str, help: &str, kind: &str, v: i64| {
            out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {v}\n"));
        };
        let c = |a: &AtomicU64| a.load(Relaxed) as i64;
        line(
            "ledger_transactions_posted_total",
            "Transactions committed",
            "counter",
            c(&self.transactions_posted),
        );
        line(
            "ledger_transactions_replayed_total",
            "Idempotent replays served",
            "counter",
            c(&self.transactions_replayed),
        );
        line(
            "ledger_transactions_rejected_total",
            "Transactions rejected",
            "counter",
            c(&self.transactions_rejected),
        );
        line(
            "ledger_mpesa_callbacks_received_total",
            "M-Pesa callbacks stored",
            "counter",
            c(&self.callbacks_received),
        );
        line(
            "ledger_mpesa_callbacks_duplicate_total",
            "Duplicate M-Pesa callbacks ignored",
            "counter",
            c(&self.callbacks_duplicate),
        );
        line(
            "ledger_mpesa_callbacks_processed_total",
            "M-Pesa callbacks applied",
            "counter",
            c(&self.callbacks_processed),
        );
        line(
            "ledger_mpesa_callbacks_retried_total",
            "M-Pesa callback retries scheduled",
            "counter",
            c(&self.callbacks_retried),
        );
        line(
            "ledger_mpesa_callbacks_dead_total",
            "M-Pesa callbacks moved to dead state",
            "counter",
            c(&self.callbacks_dead),
        );
        line(
            "ledger_reconcile_drifted_accounts",
            "Accounts whose cached balance differs from entries",
            "gauge",
            self.reconcile_drifted_accounts.load(Relaxed),
        );
        line(
            "ledger_reconcile_unbalanced_currencies",
            "Currencies where debits != credits",
            "gauge",
            self.reconcile_unbalanced_currencies.load(Relaxed),
        );
        out
    }
}
