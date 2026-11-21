use std::sync::{atomic::Ordering::Relaxed, Arc};

use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::{
    config::Config,
    domain::{AccountKind, NewTransaction},
    error::AppError,
    metrics::Metrics,
    money::Currency,
    mpesa, repo, worker,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cfg: Arc<Config>,
    pub metrics: Arc<Metrics>,
}

impl AppState {
    pub fn new(pool: PgPool, cfg: Config) -> Self {
        Self {
            pool,
            cfg: Arc::new(cfg),
            metrics: Arc::new(Metrics::default()),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/accounts", post(create_account))
        .route("/accounts/:id", get(get_account))
        .route("/transactions", post(post_transaction))
        .route("/transactions/:id", get(get_transaction))
        .route("/deposits/mpesa", post(create_deposit))
        .route("/reconcile", get(reconcile))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_api_key));

    // Daraja cannot send a Bearer token; this route is protected by a secret path segment.
    let public = Router::new().route("/webhooks/mpesa/:token", post(mpesa_webhook));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .nest("/v1", authed.merge(public))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn require_api_key(State(s): State<AppState>, req: Request, next: Next) -> Result<Response, AppError> {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| mpesa::token_matches(t, &s.cfg.api_key))
        .unwrap_or(false);
    if !ok {
        return Err(AppError::Unauthorized);
    }
    Ok(next.run(req).await)
}

async fn healthz(State(s): State<AppState>) -> Result<Json<Value>, AppError> {
    sqlx::query("SELECT 1").execute(&s.pool).await?;
    Ok(Json(json!({ "status": "ok" })))
}

async fn metrics(State(s): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        s.metrics.render(),
    )
}

// ---------------------------------------------------------------- accounts

#[derive(Deserialize)]
struct CreateAccount {
    code: String,
    name: String,
    kind: AccountKind,
    currency: Currency,
    #[serde(default)]
    allow_negative: bool,
}

async fn create_account(
    State(s): State<AppState>,
    Json(b): Json<CreateAccount>,
) -> Result<impl IntoResponse, AppError> {
    if b.code.trim().is_empty() || b.code.len() > 64 || b.name.trim().is_empty() {
        return Err(AppError::Validation("code (1-64 chars) and name are required".into()));
    }
    let acc = repo::create_account(
        &s.pool,
        b.code.trim(),
        b.name.trim(),
        b.kind,
        b.currency,
        b.allow_negative,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(acc)))
}

async fn get_account(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<impl IntoResponse, AppError> {
    Ok(Json(repo::get_account(&s.pool, id).await?))
}

// ------------------------------------------------------------ transactions

async fn post_transaction(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewTransaction>,
) -> Result<Response, AppError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty() && k.len() <= 255)
        .ok_or_else(|| AppError::Validation("Idempotency-Key header (1-255 chars) is required".into()))?;

    let result = repo::post_transaction(&s.pool, key, &body).await;
    match result {
        Ok(outcome) => {
            let (status, counter) = if outcome.replayed {
                (StatusCode::OK, &s.metrics.transactions_replayed)
            } else {
                (StatusCode::CREATED, &s.metrics.transactions_posted)
            };
            counter.fetch_add(1, Relaxed);
            let mut resp = (status, Json(outcome.transaction)).into_response();
            if outcome.replayed {
                resp.headers_mut()
                    .insert("idempotent-replayed", HeaderValue::from_static("true"));
            }
            Ok(resp)
        }
        Err(e) => {
            s.metrics.transactions_rejected.fetch_add(1, Relaxed);
            Err(e)
        }
    }
}

async fn get_transaction(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<impl IntoResponse, AppError> {
    Ok(Json(repo::get_transaction(&s.pool, id).await?))
}

// ------------------------------------------------------------------ M-Pesa

#[derive(Deserialize)]
struct CreateDeposit {
    account_id: Uuid,
    amount_minor: i64,
    msisdn: String,
}

/// Starts a deposit. In production this is where you call Daraja's STK push endpoint and use
/// the `CheckoutRequestID` it returns. Here we mint one so the flow is runnable offline.
async fn create_deposit(
    State(s): State<AppState>,
    Json(b): Json<CreateDeposit>,
) -> Result<impl IntoResponse, AppError> {
    if !(b.msisdn.len() == 12 && b.msisdn.starts_with("254") && b.msisdn.chars().all(|c| c.is_ascii_digit())) {
        return Err(AppError::Validation("msisdn must look like 2547XXXXXXXX".into()));
    }
    let checkout = format!("ws_CO_{}", Uuid::new_v4().simple());
    let intent = repo::create_deposit_intent(&s.pool, &checkout, b.account_id, b.amount_minor, &b.msisdn).await?;
    Ok((StatusCode::ACCEPTED, Json(intent)))
}

/// Fast-ack webhook: verify, persist, return. All real work happens in the worker.
async fn mpesa_webhook(
    State(s): State<AppState>,
    Path(token): Path<String>,
    Json(payload): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    if !mpesa::token_matches(&token, &s.cfg.mpesa_webhook_token) {
        return Err(AppError::Unauthorized);
    }
    let cb = mpesa::parse_stk_callback(&payload).map_err(|e| AppError::Validation(e.to_string()))?;
    let is_new = repo::store_callback(&s.pool, &cb.checkout_request_id, cb.result_code, &payload).await?;
    if is_new {
        s.metrics.callbacks_received.fetch_add(1, Relaxed);
    } else {
        s.metrics.callbacks_duplicate.fetch_add(1, Relaxed);
    }
    // Daraja expects this exact acknowledgement shape.
    Ok(Json(json!({ "ResultCode": 0, "ResultDesc": "Accepted" })))
}

async fn reconcile(State(s): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let ok = worker::run_reconcile(&s.pool, &s.metrics).await?;
    let report = repo::reconcile(&s.pool).await?;
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    Ok((status, Json(report)))
}
