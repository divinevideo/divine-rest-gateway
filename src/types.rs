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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_response_serialization() {
        let response = QueryResponse {
            events: vec![serde_json::json!({"id": "test"})],
            eose: true,
            complete: true,
            cached: false,
            cache_age_seconds: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"events\""));
        assert!(json.contains("\"eose\":true"));
        assert!(json.contains("\"complete\":true"));
        assert!(json.contains("\"cached\":false"));
        // cache_age_seconds should be skipped when None
        assert!(!json.contains("cache_age_seconds"));
    }

    #[test]
    fn test_query_response_with_cache_age() {
        let response = QueryResponse {
            events: vec![],
            eose: true,
            complete: true,
            cached: true,
            cache_age_seconds: Some(42),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"cached\":true"));
        assert!(json.contains("\"cache_age_seconds\":42"));
    }

    #[test]
    fn test_publish_request_deserialization() {
        let json = r#"{"event": {"id": "abc", "kind": 1}}"#;
        let request: PublishRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.event["id"], "abc");
        assert_eq!(request.event["kind"], 1);
    }

    #[test]
    fn test_publish_response_serialization() {
        let response = PublishResponse {
            status: "queued".to_string(),
            event_id: "abc123".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"queued\""));
        assert!(json.contains("\"event_id\":\"abc123\""));
    }

    #[test]
    fn test_publish_status_minimal() {
        let status = PublishStatus {
            status: "pending".to_string(),
            attempts: None,
            verified_at: None,
            error: None,
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"status\":\"pending\""));
        // Optional fields should be skipped
        assert!(!json.contains("attempts"));
        assert!(!json.contains("verified_at"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_publish_status_full() {
        let status = PublishStatus {
            status: "verified".to_string(),
            attempts: Some(3),
            verified_at: Some("2024-01-01T00:00:00Z".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"status\":\"verified\""));
        assert!(json.contains("\"attempts\":3"));
        assert!(json.contains("\"verified_at\""));
    }

    #[test]
    fn test_publish_status_with_error() {
        let status = PublishStatus {
            status: "failed".to_string(),
            attempts: Some(5),
            verified_at: None,
            error: Some("relay rejected".to_string()),
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"error\":\"relay rejected\""));
    }

    #[test]
    fn test_publish_status_roundtrip() {
        let status = PublishStatus {
            status: "verified".to_string(),
            attempts: Some(2),
            verified_at: Some("2024-01-01T12:00:00Z".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&status).unwrap();
        let deserialized: PublishStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.status, status.status);
        assert_eq!(deserialized.attempts, status.attempts);
        assert_eq!(deserialized.verified_at, status.verified_at);
    }

    #[test]
    fn test_error_response_new() {
        let err = ErrorResponse::new("test_error");
        assert_eq!(err.error, "test_error");
        assert!(err.detail.is_none());
        assert!(err.retry_after.is_none());
    }

    #[test]
    fn test_error_response_with_detail() {
        let err = ErrorResponse::new("invalid_request").with_detail("missing field");
        assert_eq!(err.error, "invalid_request");
        assert_eq!(err.detail, Some("missing field".to_string()));
    }

    #[test]
    fn test_error_response_serialization() {
        let err = ErrorResponse::new("rate_limited");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"error\":\"rate_limited\""));
        // Optional fields should be skipped
        assert!(!json.contains("detail"));
        assert!(!json.contains("retry_after"));
    }

    #[test]
    fn test_error_response_full_serialization() {
        let mut err = ErrorResponse::new("rate_limited").with_detail("too many requests");
        err.retry_after = Some(60);

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"error\":\"rate_limited\""));
        assert!(json.contains("\"detail\":\"too many requests\""));
        assert!(json.contains("\"retry_after\":60"));
    }

    #[test]
    fn test_cached_query_serialization() {
        let cached = CachedQuery {
            events: vec![serde_json::json!({"id": "event1"})],
            eose: true,
            timestamp: 1700000000,
        };

        let json = serde_json::to_string(&cached).unwrap();
        let deserialized: CachedQuery = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.events.len(), 1);
        assert!(deserialized.eose);
        assert_eq!(deserialized.timestamp, 1700000000);
    }
}
