//! Integration tests. Each `#[sqlx::test]` gets its own freshly migrated Postgres database.
//! Requires `DATABASE_URL` to point at a Postgres server where the user may create databases.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ledgerd::{
    api::{router, AppState},
    config::Config,
    domain::*,
    error::AppError,
    money::Currency,
    repo, worker,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

// ------------------------------------------------------------------ helpers

async fn acct(pool: &PgPool, code: &str, kind: AccountKind, allow_negative: bool) -> Account {
    repo::create_account(pool, code, code, kind, Currency::Kes, allow_negative)
        .await
        .unwrap()
}

fn transfer(from: Uuid, to: Uuid, amount: i64) -> NewTransaction {
    NewTransaction {
        description: "transfer".into(),
        currency: Currency::Kes,
        external_ref: None,
        // a wallet (liability) is *debited* when it pays out and *credited* when it receives
        entries: vec![
            NewEntry {
                account_id: from,
                direction: Direction::Debit,
                amount_minor: amount,
            },
            NewEntry {
                account_id: to,
                direction: Direction::Credit,
                amount_minor: amount,
            },
        ],
    }
}

/// Put money into a wallet from an equity "seed" account.
async fn fund(pool: &PgPool, seed: &Account, wallet: &Account, amount: i64) {
    let key = format!("fund-{}", Uuid::new_v4());
    repo::post_transaction(pool, &key, &transfer(seed.id, wallet.id, amount))
        .await
        .unwrap();
}

async fn balance(pool: &PgPool, id: Uuid) -> i64 {
    repo::get_account(pool, id).await.unwrap().balance_minor
}

// -------------------------------------------------------------------- tests

#[sqlx::test(migrations = "./migrations")]
async fn transfer_moves_balances(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let alice = acct(&pool, "ALICE", AccountKind::Liability, false).await;
    let bob = acct(&pool, "BOB", AccountKind::Liability, false).await;
    fund(&pool, &seed, &alice, 10_000).await;

    repo::post_transaction(&pool, "t1", &transfer(alice.id, bob.id, 2_500))
        .await
        .unwrap();

    assert_eq!(balance(&pool, alice.id).await, 7_500);
    assert_eq!(balance(&pool, bob.id).await, 2_500);
    assert!(repo::reconcile(&pool).await.unwrap().ok);
}

#[sqlx::test(migrations = "./migrations")]
async fn same_key_same_request_is_replayed_not_reapplied(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let alice = acct(&pool, "ALICE", AccountKind::Liability, false).await;
    let bob = acct(&pool, "BOB", AccountKind::Liability, false).await;
    fund(&pool, &seed, &alice, 10_000).await;

    let req = transfer(alice.id, bob.id, 1_000);
    let first = repo::post_transaction(&pool, "pay-42", &req).await.unwrap();
    let second = repo::post_transaction(&pool, "pay-42", &req).await.unwrap();

    assert!(!first.replayed);
    assert!(second.replayed);
    assert_eq!(first.transaction.id, second.transaction.id);
    assert_eq!(balance(&pool, bob.id).await, 1_000, "must be applied exactly once");
}

#[sqlx::test(migrations = "./migrations")]
async fn same_key_different_request_conflicts(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let alice = acct(&pool, "ALICE", AccountKind::Liability, false).await;
    let bob = acct(&pool, "BOB", AccountKind::Liability, false).await;
    fund(&pool, &seed, &alice, 10_000).await;

    repo::post_transaction(&pool, "k", &transfer(alice.id, bob.id, 100))
        .await
        .unwrap();
    let err = repo::post_transaction(&pool, "k", &transfer(alice.id, bob.id, 200))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Conflict(_)));
    assert_eq!(balance(&pool, bob.id).await, 100);
}

#[sqlx::test(migrations = "./migrations")]
async fn overdraft_is_rejected_and_leaves_no_trace(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let alice = acct(&pool, "ALICE", AccountKind::Liability, false).await;
    let bob = acct(&pool, "BOB", AccountKind::Liability, false).await;
    fund(&pool, &seed, &alice, 500).await;

    let err = repo::post_transaction(&pool, "too-much", &transfer(alice.id, bob.id, 501))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::InsufficientFunds(id) if id == alice.id));
    assert_eq!(balance(&pool, alice.id).await, 500);

    // The failed attempt rolled back entirely, including its idempotency key.
    let retry = repo::post_transaction(&pool, "too-much", &transfer(alice.id, bob.id, 500)).await;
    assert!(retry.is_ok());
}

#[sqlx::test(migrations = "./migrations")]
async fn unbalanced_and_cross_currency_are_rejected(pool: PgPool) {
    let a = acct(&pool, "A", AccountKind::Equity, true).await;
    let b = acct(&pool, "B", AccountKind::Liability, false).await;
    let usd = repo::create_account(&pool, "USD1", "usd", AccountKind::Liability, Currency::Usd, false)
        .await
        .unwrap();

    let mut bad = transfer(a.id, b.id, 100);
    bad.entries[1].amount_minor = 99;
    assert!(matches!(
        repo::post_transaction(&pool, "u", &bad).await,
        Err(AppError::Validation(_))
    ));

    let cross = transfer(a.id, usd.id, 100);
    assert!(matches!(
        repo::post_transaction(&pool, "x", &cross).await,
        Err(AppError::Validation(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn database_itself_rejects_unbalanced_entries(pool: PgPool) {
    // Bypass the application layer entirely and try to write garbage.
    let a = acct(&pool, "A", AccountKind::Equity, true).await;
    let mut tx = pool.begin().await.unwrap();
    let txn_id = Uuid::new_v4();
    sqlx::query("INSERT INTO transactions (id, idempotency_key, request_hash, description, currency) VALUES ($1,'raw','h','d','KES')")
        .bind(txn_id).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO entries (transaction_id, account_id, direction, amount_minor, currency) VALUES ($1,$2,'debit',100,'KES')")
        .bind(txn_id).bind(a.id).execute(&mut *tx).await.unwrap();
    let commit = tx.commit().await;
    assert!(
        commit.is_err(),
        "deferred trigger must reject an unbalanced transaction at COMMIT"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn ledger_rows_are_append_only(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let w = acct(&pool, "W", AccountKind::Liability, false).await;
    fund(&pool, &seed, &w, 100).await;
    assert!(sqlx::query("UPDATE entries SET amount_minor = 1")
        .execute(&pool)
        .await
        .is_err());
    assert!(sqlx::query("DELETE FROM entries").execute(&pool).await.is_err());
    assert!(sqlx::query("DELETE FROM transactions").execute(&pool).await.is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_requests_with_same_key_post_exactly_once(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let alice = acct(&pool, "ALICE", AccountKind::Liability, false).await;
    let bob = acct(&pool, "BOB", AccountKind::Liability, false).await;
    fund(&pool, &seed, &alice, 10_000).await;

    let req = transfer(alice.id, bob.id, 100);
    let mut handles = vec![];
    for _ in 0..25 {
        let (pool, req) = (pool.clone(), req.clone());
        handles.push(tokio::spawn(async move {
            repo::post_transaction(&pool, "race", &req).await
        }));
    }
    let mut created = 0;
    let mut replayed = 0;
    for h in handles {
        let out = h.await.unwrap().unwrap();
        if out.replayed {
            replayed += 1
        } else {
            created += 1
        }
    }
    assert_eq!(created, 1);
    assert_eq!(replayed, 24);
    assert_eq!(balance(&pool, bob.id).await, 100);
}

#[sqlx::test(migrations = "./migrations")]
async fn opposing_concurrent_transfers_do_not_deadlock_or_lose_money(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let a = acct(&pool, "A", AccountKind::Liability, false).await;
    let b = acct(&pool, "B", AccountKind::Liability, false).await;
    fund(&pool, &seed, &a, 5_000).await;
    fund(&pool, &seed, &b, 5_000).await;

    let mut handles = vec![];
    for i in 0..60 {
        let pool = pool.clone();
        let (from, to) = if i % 2 == 0 { (a.id, b.id) } else { (b.id, a.id) };
        handles.push(tokio::spawn(async move {
            repo::post_transaction(&pool, &format!("swap-{i}"), &transfer(from, to, 7)).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }
    assert_eq!(balance(&pool, a.id).await + balance(&pool, b.id).await, 10_000);
    assert!(repo::reconcile(&pool).await.unwrap().ok);
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_spends_cannot_overdraw(pool: PgPool) {
    let seed = acct(&pool, "SEED", AccountKind::Equity, true).await;
    let a = acct(&pool, "A", AccountKind::Liability, false).await;
    let sink = acct(&pool, "SINK", AccountKind::Liability, false).await;
    fund(&pool, &seed, &a, 1_000).await;

    let mut handles = vec![];
    for i in 0..30 {
        let (pool, from, to) = (pool.clone(), a.id, sink.id);
        handles.push(tokio::spawn(async move {
            repo::post_transaction(&pool, &format!("spend-{i}"), &transfer(from, to, 100)).await
        }));
    }
    let mut ok = 0;
    for h in handles {
        if h.await.unwrap().is_ok() {
            ok += 1
        }
    }
    assert_eq!(ok, 10, "exactly balance/amount spends may succeed");
    assert_eq!(balance(&pool, a.id).await, 0);
    assert_eq!(balance(&pool, sink.id).await, 1_000);
}

// ----------------------------------------------------------------- M-Pesa

fn stk_payload(checkout: &str, code: i64, amount: f64) -> Value {
    if code == 0 {
        json!({"Body":{"stkCallback":{
            "MerchantRequestID":"m1","CheckoutRequestID":checkout,"ResultCode":0,
            "ResultDesc":"The service request is processed successfully.",
            "CallbackMetadata":{"Item":[
                {"Name":"Amount","Value":amount},
                {"Name":"MpesaReceiptNumber","Value":"RCPT123"}]}}}})
    } else {
        json!({"Body":{"stkCallback":{
            "MerchantRequestID":"m1","CheckoutRequestID":checkout,"ResultCode":code,"ResultDesc":"cancelled"}}})
    }
}

async fn mpesa_setup(pool: &PgPool) -> Account {
    repo::ensure_account(
        pool,
        worker::MPESA_CLEARING,
        "clearing",
        AccountKind::Asset,
        Currency::Kes,
        true,
    )
    .await
    .unwrap();
    acct(pool, "WALLET", AccountKind::Liability, false).await
}

#[sqlx::test(migrations = "./migrations")]
async fn mpesa_success_credits_wallet_once_even_with_duplicate_callbacks(pool: PgPool) {
    let cfg = Config::for_tests();
    let state = AppState::new(pool.clone(), cfg.clone());
    let wallet = mpesa_setup(&pool).await;
    repo::create_deposit_intent(&pool, "ws_CO_1", wallet.id, 150_000, "254708374149")
        .await
        .unwrap();

    let payload = stk_payload("ws_CO_1", 0, 1500.0);
    assert!(repo::store_callback(&pool, "ws_CO_1", 0, &payload).await.unwrap());
    assert!(
        !repo::store_callback(&pool, "ws_CO_1", 0, &payload).await.unwrap(),
        "duplicate delivery collapses"
    );

    assert!(worker::process_one_callback(&pool, &cfg, &state.metrics).await.unwrap());
    assert!(
        !worker::process_one_callback(&pool, &cfg, &state.metrics).await.unwrap(),
        "nothing left to do"
    );

    assert_eq!(balance(&pool, wallet.id).await, 150_000);
    let clearing = {
        let mut c = pool.acquire().await.unwrap();
        repo::get_account_by_code(&mut c, worker::MPESA_CLEARING).await.unwrap()
    };
    assert_eq!(clearing.balance_minor, 150_000);
    assert!(repo::reconcile(&pool).await.unwrap().ok);
}

#[sqlx::test(migrations = "./migrations")]
async fn mpesa_callback_before_intent_is_retried_then_succeeds(pool: PgPool) {
    let cfg = Config::for_tests();
    let state = AppState::new(pool.clone(), cfg.clone());
    let wallet = mpesa_setup(&pool).await;

    // Callback arrives first (out of order)...
    repo::store_callback(&pool, "ws_CO_2", 0, &stk_payload("ws_CO_2", 0, 100.0))
        .await
        .unwrap();
    assert!(worker::process_one_callback(&pool, &cfg, &state.metrics).await.unwrap());
    let (status, attempts): (String, i32) =
        sqlx::query_as("SELECT status, attempts FROM mpesa_callbacks WHERE checkout_request_id='ws_CO_2'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), attempts),
        ("received", 1),
        "scheduled for retry with backoff"
    );
    assert_eq!(balance(&pool, wallet.id).await, 0);

    // ...then the intent appears and the retry becomes due.
    repo::create_deposit_intent(&pool, "ws_CO_2", wallet.id, 10_000, "254708374149")
        .await
        .unwrap();
    sqlx::query("UPDATE mpesa_callbacks SET next_attempt_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    assert!(worker::process_one_callback(&pool, &cfg, &state.metrics).await.unwrap());
    assert_eq!(balance(&pool, wallet.id).await, 10_000);
}

#[sqlx::test(migrations = "./migrations")]
async fn mpesa_unmatched_callback_goes_dead_after_max_attempts(pool: PgPool) {
    let cfg = Config::for_tests(); // max_callback_attempts = 3
    let state = AppState::new(pool.clone(), cfg.clone());
    mpesa_setup(&pool).await;
    repo::store_callback(&pool, "ws_CO_ghost", 0, &stk_payload("ws_CO_ghost", 0, 5.0))
        .await
        .unwrap();

    for _ in 0..3 {
        sqlx::query("UPDATE mpesa_callbacks SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        worker::process_one_callback(&pool, &cfg, &state.metrics).await.unwrap();
    }
    let status: String = sqlx::query_scalar("SELECT status FROM mpesa_callbacks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "dead");
}

#[sqlx::test(migrations = "./migrations")]
async fn mpesa_failed_payment_and_amount_mismatch_never_credit(pool: PgPool) {
    let cfg = Config::for_tests();
    let state = AppState::new(pool.clone(), cfg.clone());
    let wallet = mpesa_setup(&pool).await;

    repo::create_deposit_intent(&pool, "c_fail", wallet.id, 10_000, "254708374149")
        .await
        .unwrap();
    repo::create_deposit_intent(&pool, "c_mismatch", wallet.id, 10_000, "254708374149")
        .await
        .unwrap();
    repo::store_callback(&pool, "c_fail", 1032, &stk_payload("c_fail", 1032, 0.0))
        .await
        .unwrap();
    repo::store_callback(&pool, "c_mismatch", 0, &stk_payload("c_mismatch", 0, 5.0))
        .await
        .unwrap();

    while worker::process_one_callback(&pool, &cfg, &state.metrics).await.unwrap() {}

    assert_eq!(balance(&pool, wallet.id).await, 0);
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT checkout_request_id, status FROM payment_intents ORDER BY 1")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(rows.iter().all(|(_, s)| s == "failed"));
}

// -------------------------------------------------------------------- HTTP

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value, axum::http::HeaderMap) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), headers)
}

fn json_req(method: &str, uri: &str, auth: bool, key: Option<&str>, body: Value) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if auth {
        b = b.header("authorization", "Bearer test-key");
    }
    if let Some(k) = key {
        b = b.header("idempotency-key", k);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn http_end_to_end(pool: PgPool) {
    let app = router(AppState::new(pool.clone(), Config::for_tests()));

    // auth required
    let (s, _, _) = call(&app, json_req("POST", "/v1/accounts", false, None, json!({}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let mk = |code: &str, kind: &str, neg: bool| json!({"code":code,"name":code,"kind":kind,"currency":"KES","allow_negative":neg});
    let (s, seed, _) = call(
        &app,
        json_req("POST", "/v1/accounts", true, None, mk("SEED", "equity", true)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, wallet, _) = call(
        &app,
        json_req("POST", "/v1/accounts", true, None, mk("W1", "liability", false)),
    )
    .await;
    let (s, _, _) = call(
        &app,
        json_req("POST", "/v1/accounts", true, None, mk("W1", "liability", false)),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "duplicate account code");

    let body = json!({"description":"seed","currency":"KES","entries":[
        {"account_id": seed["id"], "direction":"debit","amount_minor":5000},
        {"account_id": wallet["id"], "direction":"credit","amount_minor":5000}]});

    // missing idempotency key
    let (s, _, _) = call(&app, json_req("POST", "/v1/transactions", true, None, body.clone())).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let (s, t1, _) = call(
        &app,
        json_req("POST", "/v1/transactions", true, Some("abc"), body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, t2, h) = call(
        &app,
        json_req("POST", "/v1/transactions", true, Some("abc"), body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.get("idempotent-replayed").unwrap(), "true");
    assert_eq!(t1["id"], t2["id"]);

    let uri = format!("/v1/accounts/{}", wallet["id"].as_str().unwrap());
    let req = Request::builder()
        .uri(uri)
        .header("authorization", "Bearer test-key")
        .body(Body::empty())
        .unwrap();
    let (s, acc, _) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(acc["balance_minor"], 5000);

    // webhook: wrong token rejected, right token accepted, no bearer needed
    let payload = stk_payload("ws_CO_http", 0, 50.0);
    let (s, _, _) = call(
        &app,
        json_req("POST", "/v1/webhooks/mpesa/nope", false, None, payload.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, ack, _) = call(
        &app,
        json_req("POST", "/v1/webhooks/mpesa/test-webhook-token", false, None, payload),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ack["ResultCode"], 0);

    let req = Request::builder()
        .uri("/v1/reconcile")
        .header("authorization", "Bearer test-key")
        .body(Body::empty())
        .unwrap();
    let (s, report, _) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(report["ok"], true);
}
