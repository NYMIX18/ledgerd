use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub api_key: String,
    pub mpesa_webhook_token: String,
    pub worker_interval_ms: u64,
    pub max_callback_attempts: i32,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        fn req(k: &str) -> anyhow::Result<String> {
            env::var(k).map_err(|_| anyhow::anyhow!("missing required env var {k}"))
        }
        fn opt<T: std::str::FromStr>(k: &str, default: T) -> T {
            env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        Ok(Self {
            database_url: req("DATABASE_URL")?,
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            api_key: req("API_KEY")?,
            mpesa_webhook_token: req("MPESA_WEBHOOK_TOKEN")?,
            worker_interval_ms: opt("WORKER_INTERVAL_MS", 1000),
            max_callback_attempts: opt("MAX_CALLBACK_ATTEMPTS", 8),
        })
    }

    /// Fixed config for tests.
    pub fn for_tests() -> Self {
        Self {
            database_url: String::new(),
            bind_addr: "127.0.0.1:0".into(),
            api_key: "test-key".into(),
            mpesa_webhook_token: "test-webhook-token".into(),
            worker_interval_ms: 50,
            max_callback_attempts: 3,
        }
    }
}
