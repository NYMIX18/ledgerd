//! Posting-path benchmark (database + posting algorithm, no HTTP).
//!
//!   DATABASE_URL=postgres://ledger:ledger@localhost:5432/ledger \
//!   N=20000 C=32 ACCOUNTS=1000 cargo run --release --example bench
//!
//! Two scenarios, every request using a unique idempotency key:
//!   contended : all tasks move money between the SAME two accounts (worst case, row-lock bound)
//!   spread    : tasks move money between random pairs out of ACCOUNTS accounts
use std::time::{Duration, Instant};

use ledgerd::{domain::*, money::Currency, repo};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

fn env<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

fn xorshift(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

async fn run(name: &str, pool: &PgPool, wallets: &[Uuid], n: usize, c: usize, contended: bool) {
    let run_id = Uuid::new_v4().simple().to_string();
    let per_task = n / c;
    let started = Instant::now();
    let mut handles = Vec::new();
    for t in 0..c {
        let (pool, wallets, run_id) = (pool.clone(), wallets.to_vec(), run_id.clone());
        handles.push(tokio::spawn(async move {
            let mut rng = 0x9E3779B97F4A7C15u64 ^ (t as u64 + 1);
            let mut lat = Vec::with_capacity(per_task);
            let mut errors = 0usize;
            for i in 0..per_task {
                let (a, b) = if contended {
                    if i % 2 == 0 {
                        (wallets[0], wallets[1])
                    } else {
                        (wallets[1], wallets[0])
                    }
                } else {
                    let a = (xorshift(&mut rng) as usize) % wallets.len();
                    let mut b = (xorshift(&mut rng) as usize) % wallets.len();
                    if a == b {
                        b = (b + 1) % wallets.len();
                    }
                    (wallets[a], wallets[b])
                };
                let req = NewTransaction {
                    description: "bench".into(),
                    currency: Currency::Kes,
                    external_ref: None,
                    entries: vec![
                        NewEntry {
                            account_id: a,
                            direction: Direction::Debit,
                            amount_minor: 1,
                        },
                        NewEntry {
                            account_id: b,
                            direction: Direction::Credit,
                            amount_minor: 1,
                        },
                    ],
                };
                let t0 = Instant::now();
                if repo::post_transaction(&pool, &format!("bench-{run_id}-{t}-{i}"), &req)
                    .await
                    .is_err()
                {
                    errors += 1;
                }
                lat.push(t0.elapsed());
            }
            (lat, errors)
        }));
    }
    let mut all: Vec<Duration> = Vec::new();
    let mut errors = 0;
    for h in handles {
        let (l, e) = h.await.unwrap();
        all.extend(l);
        errors += e;
    }
    let elapsed = started.elapsed();
    all.sort();
    let pct = |p: f64| all[((all.len() as f64 * p) as usize).min(all.len() - 1)].as_secs_f64() * 1000.0;
    println!(
        "| {name} | {} | {c} | {:.0} | {:.2} | {:.2} | {:.2} | {errors} |",
        all.len(),
        all.len() as f64 / elapsed.as_secs_f64(),
        pct(0.50),
        pct(0.95),
        pct(0.99)
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::var("DATABASE_URL")?;
    let (n, c, n_acc): (usize, usize, usize) = (env("N", 20_000), env("C", 32), env("ACCOUNTS", 1000));
    let pool = PgPoolOptions::new().max_connections(c as u32 + 2).connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let tag = Uuid::new_v4().simple().to_string();
    let seed = repo::create_account(
        &pool,
        &format!("BENCH-SEED-{tag}"),
        "seed",
        AccountKind::Equity,
        Currency::Kes,
        true,
    )
    .await?;
    let mut wallets = Vec::new();
    for i in 0..n_acc.max(2) {
        let w = repo::create_account(
            &pool,
            &format!("BENCH-{tag}-{i}"),
            "w",
            AccountKind::Liability,
            Currency::Kes,
            false,
        )
        .await?;
        let req = NewTransaction {
            description: "fund".into(),
            currency: Currency::Kes,
            external_ref: None,
            entries: vec![
                NewEntry {
                    account_id: seed.id,
                    direction: Direction::Debit,
                    amount_minor: 1_000_000,
                },
                NewEntry {
                    account_id: w.id,
                    direction: Direction::Credit,
                    amount_minor: 1_000_000,
                },
            ],
        };
        repo::post_transaction(&pool, &format!("fund-{tag}-{i}"), &req).await?;
        wallets.push(w.id);
    }

    println!("| scenario | requests | concurrency | tx/s | p50 ms | p95 ms | p99 ms | errors |");
    println!("|---|---|---|---|---|---|---|---|");
    run("contended (2 accounts)", &pool, &wallets[..2], n, c, true).await;
    run(
        &format!("spread ({} accounts)", wallets.len()),
        &pool,
        &wallets,
        n,
        c,
        false,
    )
    .await;

    let report = repo::reconcile(&pool).await?;
    println!("\nreconcile after benchmark: ok = {}", report.ok);
    Ok(())
}
