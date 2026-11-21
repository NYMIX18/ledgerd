//! ledgerd: a small, correctness-first double-entry ledger service.
//!
//! Module map:
//! - [`money`]   : integer-only `Money` and `Currency` types
//! - [`domain`]  : accounts, entries, transaction validation, request fingerprints
//! - [`repo`]    : all SQL; posting algorithm lives in [`repo::post_transaction_tx`]
//! - [`mpesa`]   : Daraja STK callback parsing and webhook token check
//! - [`worker`]  : background callback processor and reconciliation loop
//! - [`api`]     : axum router, handlers, auth middleware
pub mod api;
pub mod config;
pub mod domain;
pub mod error;
pub mod metrics;
pub mod money;
pub mod mpesa;
pub mod repo;
pub mod worker;
