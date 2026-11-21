//! All SQL lives here.
//!
//! The heart of the service is [`post_transaction_tx`]. It takes a *caller-owned*
//! database transaction so that other units of work (e.g. applying an M-Pesa callback)
//! can post to the ledger and update their own rows atomically.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{postgres::PgRow, PgConnection, PgPool, Row};
use uuid::Uuid;

use crate::{
    domain::*,
    error::{is_unique_violation, AppError},
    money::Currency,
};

// ---------------------------------------------------------------- accounts

const ACCOUNT_COLS: &str = "id, code, name, kind, currency, balance_minor, allow_negative, created_at";

fn account_from_row(r: &PgRow) -> Result<Account, AppError> {
    let kind: String = r.try_get("kind")?;
    let currency: String = r.try_get("currency")?;
    Ok(Account {
        id: r.try_get("id")?,
        code: r.try_get("code")?,
        name: r.try_get("name")?,
        kind: kind.parse().map_err(|e: DomainError| anyhow::anyhow!(e))?,
        currency: currency.parse().map_err(|e| anyhow::anyhow!("{e}"))?,
        balance_minor: r.try_get("balance_minor")?,
        allow_negative: r.try_get("allow_negative")?,
        created_at: r.try_get("created_at")?,
    })
}

pub async fn create_account(
    pool: &PgPool,
    code: &str,
    name: &str,
    kind: AccountKind,
    currency: Currency,
    allow_negative: bool,
) -> Result<Account, AppError> {
    let res = sqlx::query(&format!(
        "INSERT INTO accounts (id, code, name, kind, currency, allow_negative)
         VALUES ($1,$2,$3,$4,$5,$6) RETURNING {ACCOUNT_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(code)
    .bind(name)
    .bind(kind.as_str())
    .bind(currency.as_str())
    .bind(allow_negative)
    .fetch_one(pool)
    .await;
    match res {
        Ok(row) => account_from_row(&row),
        Err(e) if is_unique_violation(&e) => Err(AppError::Conflict(format!("account code '{code}' already exists"))),
        Err(e) => Err(e.into()),
    }
}

/// Idempotent bootstrap of well-known system accounts.
pub async fn ensure_account(
    pool: &PgPool,
    code: &str,
    name: &str,
    kind: AccountKind,
    currency: Currency,
    allow_negative: bool,
) -> Result<Account, AppError> {
    let row = sqlx::query(&format!(
        "INSERT INTO accounts (id, code, name, kind, currency, allow_negative)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (code) DO UPDATE SET name = EXCLUDED.name
         RETURNING {ACCOUNT_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(code)
    .bind(name)
    .bind(kind.as_str())
    .bind(currency.as_str())
    .bind(allow_negative)
    .fetch_one(pool)
    .await?;
    account_from_row(&row)
}

pub async fn get_account(pool: &PgPool, id: Uuid) -> Result<Account, AppError> {
    let row = sqlx::query(&format!("SELECT {ACCOUNT_COLS} FROM accounts WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("account"))?;
    account_from_row(&row)
}

pub async fn get_account_by_code(conn: &mut PgConnection, code: &str) -> Result<Account, AppError> {
    let row = sqlx::query(&format!("SELECT {ACCOUNT_COLS} FROM accounts WHERE code = $1"))
        .bind(code)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(AppError::NotFound("account"))?;
    account_from_row(&row)
}

// ------------------------------------------------------------ transactions

#[derive(Debug)]
pub struct PostOutcome {
    pub transaction: TransactionView,
    /// True when this idempotency key had already been used with an identical request.
    pub replayed: bool,
}

/// Convenience wrapper: own the DB transaction, post, commit.
pub async fn post_transaction(pool: &PgPool, key: &str, req: &NewTransaction) -> Result<PostOutcome, AppError> {
    let mut tx = pool.begin().await?;
    let outcome = post_transaction_tx(&mut tx, key, req).await?;
    tx.commit().await?;
    Ok(outcome)
}

/// The posting algorithm. Runs inside the caller's DB transaction (READ COMMITTED).
///
/// 1. Validate the request (pure).
/// 2. Claim the idempotency key (`INSERT .. ON CONFLICT DO NOTHING`). A concurrent request
///    with the same key blocks on the unique index until we commit or roll back.
/// 3. Lock every touched account row in a fixed (id) order, preventing deadlocks.
/// 4. Check currencies and overdraft rules against the locked balances.
/// 5. Insert entries and update cached balances.
/// 6. At COMMIT, a deferred trigger re-verifies debits == credits inside the database.
///
/// On `Err` the caller must drop/rollback its transaction.
pub async fn post_transaction_tx(
    conn: &mut PgConnection,
    key: &str,
    req: &NewTransaction,
) -> Result<PostOutcome, AppError> {
    req.validate()?;
    let hash = req.fingerprint();
    let txn_id = Uuid::new_v4();

    // (2) idempotency claim
    let inserted = sqlx::query(
        "INSERT INTO transactions (id, idempotency_key, request_hash, description, currency, external_ref)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (idempotency_key) DO NOTHING
         RETURNING created_at",
    )
    .bind(txn_id)
    .bind(key)
    .bind(&hash)
    .bind(req.description.trim())
    .bind(req.currency.as_str())
    .bind(&req.external_ref)
    .fetch_optional(&mut *conn)
    .await?;

    let Some(inserted) = inserted else {
        let existing = sqlx::query("SELECT id, request_hash FROM transactions WHERE idempotency_key = $1")
            .bind(key)
            .fetch_one(&mut *conn)
            .await?;
        let existing_hash: String = existing.try_get("request_hash")?;
        if existing_hash != hash {
            return Err(AppError::Conflict(
                "idempotency key was already used with a different request".into(),
            ));
        }
        let id: Uuid = existing.try_get("id")?;
        return Ok(PostOutcome {
            transaction: load_transaction(&mut *conn, id).await?,
            replayed: true,
        });
    };
    let created_at: DateTime<Utc> = inserted.try_get("created_at")?;

    // (3) lock accounts in a deterministic order
    let mut ids: Vec<Uuid> = req.entries.iter().map(|e| e.account_id).collect();
    ids.sort();
    ids.dedup();
    let rows = sqlx::query(
        "SELECT id, kind, currency, balance_minor, allow_negative
           FROM accounts WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() != ids.len() {
        return Err(AppError::NotFound("account"));
    }

    struct Locked {
        kind: AccountKind,
        currency: String,
        balance: i64,
        allow_negative: bool,
    }
    let mut locked: HashMap<Uuid, Locked> = HashMap::new();
    for r in &rows {
        let kind: String = r.try_get("kind")?;
        locked.insert(
            r.try_get("id")?,
            Locked {
                kind: kind.parse().map_err(|e: DomainError| anyhow::anyhow!(e))?,
                currency: r.try_get("currency")?,
                balance: r.try_get("balance_minor")?,
                allow_negative: r.try_get("allow_negative")?,
            },
        );
    }

    // (4) currency + overdraft checks
    let mut delta: HashMap<Uuid, i64> = HashMap::new();
    for e in &req.entries {
        let acc = &locked[&e.account_id];
        if acc.currency != req.currency.as_str() {
            return Err(AppError::Validation(format!(
                "account {} is {} but the transaction is {}",
                e.account_id,
                acc.currency,
                req.currency.as_str()
            )));
        }
        let signed = if e.direction == acc.kind.normal_side() {
            e.amount_minor
        } else {
            -e.amount_minor
        };
        let d = delta.entry(e.account_id).or_insert(0);
        *d = d
            .checked_add(signed)
            .ok_or_else(|| AppError::Validation("amount overflow".into()))?;
    }
    for (id, d) in &delta {
        let acc = &locked[id];
        let new_balance = acc
            .balance
            .checked_add(*d)
            .ok_or_else(|| AppError::Validation("balance overflow".into()))?;
        if new_balance < 0 && !acc.allow_negative {
            return Err(AppError::InsufficientFunds(*id));
        }
    }

    // (5) write entries and cached balances
    for e in &req.entries {
        sqlx::query(
            "INSERT INTO entries (transaction_id, account_id, direction, amount_minor, currency)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(txn_id)
        .bind(e.account_id)
        .bind(e.direction.as_str())
        .bind(e.amount_minor)
        .bind(req.currency.as_str())
        .execute(&mut *conn)
        .await?;
    }
    for (id, d) in &delta {
        sqlx::query("UPDATE accounts SET balance_minor = balance_minor + $2 WHERE id = $1")
            .bind(id)
            .bind(d)
            .execute(&mut *conn)
            .await?;
    }

    Ok(PostOutcome {
        transaction: TransactionView {
            id: txn_id,
            idempotency_key: key.to_string(),
            description: req.description.trim().to_string(),
            currency: req.currency,
            external_ref: req.external_ref.clone(),
            created_at,
            entries: req
                .entries
                .iter()
                .map(|e| EntryView {
                    account_id: e.account_id,
                    direction: e.direction,
                    amount_minor: e.amount_minor,
                })
                .collect(),
        },
        replayed: false,
    })
}

pub async fn load_transaction(conn: &mut PgConnection, id: Uuid) -> Result<TransactionView, AppError> {
    let t = sqlx::query(
        "SELECT id, idempotency_key, description, currency, external_ref, created_at
           FROM transactions WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound("transaction"))?;
    let entries =
        sqlx::query("SELECT account_id, direction, amount_minor FROM entries WHERE transaction_id = $1 ORDER BY id")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    let currency: String = t.try_get("currency")?;
    let mut views = Vec::with_capacity(entries.len());
    for e in entries {
        let dir: String = e.try_get("direction")?;
        views.push(EntryView {
            account_id: e.try_get("account_id")?,
            direction: dir.parse().map_err(|e: DomainError| anyhow::anyhow!(e))?,
            amount_minor: e.try_get("amount_minor")?,
        });
    }
    Ok(TransactionView {
        id: t.try_get("id")?,
        idempotency_key: t.try_get("idempotency_key")?,
        description: t.try_get("description")?,
        currency: currency.parse().map_err(|e| anyhow::anyhow!("{e}"))?,
        external_ref: t.try_get("external_ref")?,
        created_at: t.try_get("created_at")?,
        entries: views,
    })
}

pub async fn get_transaction(pool: &PgPool, id: Uuid) -> Result<TransactionView, AppError> {
    let mut conn = pool.acquire().await?;
    load_transaction(&mut conn, id).await
}

// ------------------------------------------------------------------ M-Pesa

#[derive(Debug, Serialize)]
pub struct PaymentIntent {
    pub id: Uuid,
    pub checkout_request_id: String,
    pub account_id: Uuid,
    pub amount_minor: i64,
    pub msisdn: String,
    pub status: String,
}

/// Record that we asked Daraja to prompt `msisdn` to pay into `account_id`.
/// The wallet must be a KES liability account (money we owe the customer).
pub async fn create_deposit_intent(
    pool: &PgPool,
    checkout_request_id: &str,
    account_id: Uuid,
    amount_minor: i64,
    msisdn: &str,
) -> Result<PaymentIntent, AppError> {
    if amount_minor <= 0 {
        return Err(AppError::Validation("amount_minor must be positive".into()));
    }
    let acc = get_account(pool, account_id).await?;
    if acc.currency != Currency::Kes || acc.kind != AccountKind::Liability {
        return Err(AppError::Validation(
            "deposit target must be a KES liability (wallet) account".into(),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payment_intents (id, checkout_request_id, account_id, amount_minor, msisdn)
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(id)
    .bind(checkout_request_id)
    .bind(account_id)
    .bind(amount_minor)
    .bind(msisdn)
    .execute(pool)
    .await?;
    Ok(PaymentIntent {
        id,
        checkout_request_id: checkout_request_id.to_string(),
        account_id,
        amount_minor,
        msisdn: msisdn.to_string(),
        status: "pending".into(),
    })
}

/// Persist a raw callback. Returns `false` if it was a duplicate delivery.
pub async fn store_callback(
    pool: &PgPool,
    checkout_request_id: &str,
    result_code: i32,
    payload: &serde_json::Value,
) -> Result<bool, AppError> {
    let res = sqlx::query(
        "INSERT INTO mpesa_callbacks (id, checkout_request_id, result_code, payload)
         VALUES ($1,$2,$3,$4) ON CONFLICT (checkout_request_id) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(checkout_request_id)
    .bind(result_code)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

// ---------------------------------------------------------- reconciliation

#[derive(Debug, Serialize)]
pub struct ReconcileReport {
    pub ok: bool,
    /// Currencies where total debits != total credits (should always be empty).
    pub unbalanced_currencies: Vec<String>,
    /// Accounts whose cached balance differs from the sum of their entries (should always be empty).
    pub drifted_accounts: Vec<Uuid>,
}

pub async fn reconcile(pool: &PgPool) -> Result<ReconcileReport, AppError> {
    let unbalanced: Vec<String> = sqlx::query(
        "SELECT currency FROM entries GROUP BY currency
          HAVING COALESCE(SUM(amount_minor) FILTER (WHERE direction='debit'),0)
              <> COALESCE(SUM(amount_minor) FILTER (WHERE direction='credit'),0)",
    )
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| r.try_get("currency"))
    .collect::<Result<_, _>>()?;

    let drifted: Vec<Uuid> = sqlx::query(
        "SELECT a.id
           FROM accounts a
           LEFT JOIN (
                SELECT account_id,
                       SUM(CASE WHEN direction='debit' THEN amount_minor ELSE -amount_minor END)::BIGINT AS net
                  FROM entries GROUP BY account_id
           ) e ON e.account_id = a.id
          WHERE a.balance_minor <> CASE WHEN a.kind IN ('asset','expense')
                                        THEN COALESCE(e.net,0) ELSE -COALESCE(e.net,0) END",
    )
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| r.try_get("id"))
    .collect::<Result<_, _>>()?;

    Ok(ReconcileReport {
        ok: unbalanced.is_empty() && drifted.is_empty(),
        unbalanced_currencies: unbalanced,
        drifted_accounts: drifted,
    })
}
