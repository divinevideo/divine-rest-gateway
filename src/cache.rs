// ABOUTME: Workers KV cache operations for storing and retrieving query results
// ABOUTME: Handles TTL management and cache key generation

use crate::types::{CachedQuery, PublishStatus};
use worker::kv::KvStore;
use worker::*;

pub struct Cache {
    kv: KvStore,
}

impl Cache {
    pub fn new(kv: KvStore) -> Self {
        Self { kv }
    }

    /// Get cached query result
    pub async fn get_query(&self, cache_key: &str) -> Result<Option<(CachedQuery, u64)>> {
        match self.kv.get(cache_key).json::<CachedQuery>().await? {
            Some(cached) => {
                let now = now_seconds();
                let age = now.saturating_sub(cached.timestamp);
                Ok(Some((cached, age)))
            }
            None => Ok(None),
        }
    }

    /// Store query result with TTL
    pub async fn put_query(&self, cache_key: &str, events: Vec<serde_json::Value>, eose: bool, ttl_seconds: u64) -> Result<()> {
        let cached = CachedQuery {
            events,
            eose,
            timestamp: now_seconds(),
        };
        self.kv
            .put(cache_key, serde_json::to_string(&cached)?)?
            .expiration_ttl(ttl_seconds)
            .execute()
            .await?;
        Ok(())
    }

    /// Get publish status
    pub async fn get_publish_status(&self, event_id: &str) -> Result<Option<PublishStatus>> {
        let key = format!("publish:{}", event_id);
        Ok(self.kv.get(&key).json::<PublishStatus>().await?)
    }

    /// Set publish status
    pub async fn set_publish_status(&self, event_id: &str, status: &PublishStatus) -> Result<()> {
        let key = format!("publish:{}", event_id);
        self.kv
            .put(&key, serde_json::to_string(status)?)?
            .expiration_ttl(86400) // 24 hours
            .execute()
            .await?;
        Ok(())
    }
}

/// Get current Unix timestamp in seconds
fn now_seconds() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}
