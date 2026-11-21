//! Background work: apply stored M-Pesa callbacks to the ledger, and reconcile.
//!
//! Callback lifecycle (see README for the diagram):
//! `received` -> `processed`            (ledger posted atomically with the status change)
//! `received` -> `received` (retry)     (e.g. the callback beat the intent; exponential backoff)
//! `received` -> `dead`                 (retries exhausted; needs a human)

use std::sync::atomic::Ordering::Relaxed;

use serde_json::Value;
use sqlx::{PgConnection, PgPool, Row};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::{
    api::AppState,
    config::Config,
    domain::{Direction, NewEntry, NewTransaction},
    error::AppError,
    metrics::Metrics,
    money::Currency,
    mpesa, repo,
};

pub const MPESA_CLEARING: &str = "MPESA_CLEARING";

/// Claim and process at most one due callback. Returns `true` if a row was claimed.
///
/// The claim uses `FOR UPDATE SKIP LOCKED`, so any number of workers/instances can run
/// this concurrently without double-processing or blocking each other.
pub async fn process_one_callback(pool: &PgPool, cfg: &Config, metrics: &Metrics) -> Result<bool, AppError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id, checkout_request_id, attempts, payload
           FROM mpesa_callbacks
          WHERE status = 'received' AND next_attempt_at <= now()
          ORDER BY next_attempt_at
          LIMIT 1
          FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else { return Ok(false) };

    let id: Uuid = row.try_get("id")?;
    let checkout: String = row.try_get("checkout_request_id")?;
    let attempts: i32 = row.try_get("attempts")?;
    let payload: Value = row.try_get("payload")?;

    match apply_callback(&mut tx, &checkout, &payload).await {
        Ok(()) => {
            sqlx::query(
                "UPDATE mpesa_callbacks SET status='processed', processed_at=now(), last_error=NULL WHERE id=$1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            // Ledger posting + intent update + callback status commit together, or not at all.
            tx.commit().await?;
            metrics.callbacks_processed.fetch_add(1, Relaxed);
        }
        Err(e) => {
            // Roll back any partial work, then record the failure in a fresh statement.
            drop(tx);
            tracing::warn!(callback = %id, attempts, error = %e, "callback processing failed");
            schedule_retry(pool, id, attempts, &e.to_string(), cfg.max_callback_attempts, metrics).await?;
        }
    }
    Ok(true)
}

/// Business logic for one callback, executed inside the caller's DB transaction.
async fn apply_callback(conn: &mut PgConnection, checkout: &str, payload: &Value) -> Result<(), AppError> {
    let cb = mpesa::parse_stk_callback(payload).map_err(|e| AppError::Validation(e.to_string()))?;

    // Out-of-order delivery: the callback can arrive before our intent row is visible.
    // Returning an error here triggers a retry with backoff.
    let intent = sqlx::query(
        "SELECT id, account_id, amount_minor, status FROM payment_intents
          WHERE checkout_request_id = $1 FOR UPDATE",
    )
    .bind(checkout)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound("payment intent"))?;

    let intent_id: Uuid = intent.try_get("id")?;
    let account_id: Uuid = intent.try_get("account_id")?;
    let expected: i64 = intent.try_get("amount_minor")?;
    let status: String = intent.try_get("status")?;

    if status != "pending" {
        return Ok(()); // already settled; nothing to do (safe under redelivery)
    }

    if cb.result_code != 0 {
        mark_intent(conn, intent_id, "failed", Some(&cb.result_desc)).await?;
        return Ok(());
    }

    if cb.amount_minor != Some(expected) {
        // Never credit an amount that differs from what we asked for.
        mark_intent(conn, intent_id, "failed", Some("amount_mismatch")).await?;
        return Ok(());
    }

    let clearing = repo::get_account_by_code(conn, MPESA_CLEARING).await?;
    let req = NewTransaction {
        description: format!("M-Pesa deposit {}", cb.receipt.as_deref().unwrap_or(checkout)),
        currency: Currency::Kes,
        external_ref: cb.receipt.clone(),
        entries: vec![
            NewEntry {
                account_id: clearing.id,
                direction: Direction::Debit,
                amount_minor: expected,
            },
            NewEntry {
                account_id,
                direction: Direction::Credit,
                amount_minor: expected,
            },
        ],
    };
    // Deterministic idempotency key: even if this code path ran twice, the ledger posts once.
    repo::post_transaction_tx(conn, &format!("mpesa:{checkout}"), &req).await?;
    mark_intent(conn, intent_id, "completed", None).await?;
    Ok(())
}

async fn mark_intent(conn: &mut PgConnection, id: Uuid, status: &str, reason: Option<&str>) -> Result<(), AppError> {
    sqlx::query("UPDATE payment_intents SET status=$2, failure_reason=$3 WHERE id=$1")
        .bind(id)
        .bind(status)
        .bind(reason)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn schedule_retry(
    pool: &PgPool,
    id: Uuid,
    attempts: i32,
    error: &str,
    max_attempts: i32,
    metrics: &Metrics,
) -> Result<(), AppError> {
    let next = attempts + 1;
    if next >= max_attempts {
        sqlx::query("UPDATE mpesa_callbacks SET status='dead', attempts=$2, last_error=$3 WHERE id=$1")
            .bind(id)
            .bind(next)
            .bind(error)
            .execute(pool)
            .await?;
        metrics.callbacks_dead.fetch_add(1, Relaxed);
        tracing::error!(callback = %id, "callback moved to dead state after {next} attempts");
    } else {
        // 2s, 4s, 8s ... capped at 5 minutes.
        let backoff_secs = (2f64.powi(next)).min(300.0);
        sqlx::query(
            "UPDATE mpesa_callbacks
                SET attempts=$2, last_error=$3, next_attempt_at = now() + make_interval(secs => $4)
              WHERE id=$1",
        )
        .bind(id)
        .bind(next)
        .bind(error)
        .bind(backoff_secs)
        .execute(pool)
        .await?;
        metrics.callbacks_retried.fetch_add(1, Relaxed);
    }
    Ok(())
}

/// Run reconciliation and publish the result as gauges. Returns whether the books are clean.
pub async fn run_reconcile(pool: &PgPool, metrics: &Metrics) -> Result<bool, AppError> {
    let report = repo::reconcile(pool).await?;
    metrics
        .reconcile_drifted_accounts
        .store(report.drifted_accounts.len() as i64, Relaxed);
    metrics
        .reconcile_unbalanced_currencies
        .store(report.unbalanced_currencies.len() as i64, Relaxed);
    if !report.ok {
        tracing::error!(?report, "RECONCILIATION FAILED: ledger invariants violated");
    }
    Ok(report.ok)
}

/// Spawn the long-running worker loop.
pub fn spawn(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick: u64 = 0;
        loop {
            loop {
                match process_one_callback(&state.pool, &state.cfg, &state.metrics).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(e) => {
                        tracing::error!(error = %e, "worker error");
                        break;
                    }
                }
            }
            tick += 1;
            if tick.is_multiple_of(30) {
                if let Err(e) = run_reconcile(&state.pool, &state.metrics).await {
                    tracing::error!(error = %e, "reconcile error");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(state.cfg.worker_interval_ms)).await;
        }
    })
}
