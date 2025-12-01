// ABOUTME: API request/response types for the REST gateway
// ABOUTME: Defines JSON structures for query responses and publish requests

use serde::{Deserialize, Serialize};

/// Response for query endpoints
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub events: Vec<serde_json::Value>,
    pub eose: bool,
    pub complete: bool,
    pub cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_age_seconds: Option<u64>,
}

/// Request body for publish endpoint
#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    pub event: serde_json::Value,
}

/// Response for publish endpoint
#[derive(Debug, Serialize)]
pub struct PublishResponse {
    pub status: String,
    pub event_id: String,
}

/// Response for publish status endpoint
#[derive(Debug, Serialize, Deserialize)]
pub struct PublishStatus {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Standard error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u32>,
}

impl ErrorResponse {
    pub fn new(error: &str) -> Self {
        Self {
            error: error.to_string(),
            detail: None,
            retry_after: None,
        }
    }

    pub fn with_detail(mut self, detail: &str) -> Self {
        self.detail = Some(detail.to_string());
        self
    }
}

/// Cached query data stored in KV
#[derive(Debug, Serialize, Deserialize)]
pub struct CachedQuery {
    pub events: Vec<serde_json::Value>,
    pub eose: bool,
    pub timestamp: u64,
}
