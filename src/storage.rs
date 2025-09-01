use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPayload {
    pub key: String,
    pub payload: String,
    pub expires_at: i64,
}

#[async_trait]
pub trait Storage: Send + Sync {
    async fn get_cache(&self, key: &str, now: i64) -> Result<Option<String>>;
    async fn put_cache(&self, key: &str, payload: &str, expires_at: i64) -> Result<()>;
}
