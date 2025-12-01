// ABOUTME: Integration tests against the live Divine REST Gateway
// ABOUTME: Tests endpoints, response formats, and error handling

use reqwest::blocking::Client;
use serde_json::Value;

const GATEWAY_URL: &str = "https://gateway.divine.video";

fn client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
}

fn encode_filter(filter: &Value) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(filter.to_string().as_bytes())
}

// ============================================================================
// Root endpoint tests
// ============================================================================

#[test]
fn test_root_returns_html() {
    let resp = client()
        .get(GATEWAY_URL)
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);
    let content_type = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("text/html"));

    let body = resp.text().unwrap();
    assert!(body.contains("Divine REST Gateway"));
    assert!(body.contains("/query"));
}

#[test]
fn test_health_endpoint() {
    let resp = client()
        .get(format!("{}/health", GATEWAY_URL))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().unwrap(), "ok");
}

// ============================================================================
// Query endpoint tests
// ============================================================================

#[test]
fn test_query_returns_events() {
    let filter = serde_json::json!({
        "kinds": [0],
        "limit": 1
    });
    let encoded = encode_filter(&filter);

    let resp = client()
        .get(format!("{}/query?filter={}", GATEWAY_URL, encoded))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().expect("Invalid JSON response");
    assert!(body["events"].is_array());
    assert!(body["eose"].is_boolean());
    assert!(body["complete"].is_boolean());
    assert!(body["cached"].is_boolean());
}

#[test]
fn test_query_has_cache_headers() {
    let filter = serde_json::json!({
        "kinds": [0],
        "limit": 1
    });
    let encoded = encode_filter(&filter);

    let resp = client()
        .get(format!("{}/query?filter={}", GATEWAY_URL, encoded))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);

    // Should have Cache-Control header
    let cache_control = resp.headers().get("cache-control");
    assert!(cache_control.is_some(), "Missing Cache-Control header");
}

#[test]
fn test_query_missing_filter() {
    let resp = client()
        .get(format!("{}/query", GATEWAY_URL))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 400);

    let body: Value = resp.json().expect("Invalid JSON response");
    assert_eq!(body["error"], "invalid_filter");
}

#[test]
fn test_query_invalid_base64() {
    let resp = client()
        .get(format!("{}/query?filter=not-valid-base64!!!", GATEWAY_URL))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 400);

    let body: Value = resp.json().expect("Invalid JSON response");
    assert_eq!(body["error"], "invalid_filter");
}

#[test]
fn test_query_with_authors() {
    // Jack's pubkey
    let filter = serde_json::json!({
        "authors": ["82341f882b6eabcd2ba7f1ef90aad961cf074af15b9ef44a09f9d2a8fbfbe6a2"],
        "kinds": [0],
        "limit": 1
    });
    let encoded = encode_filter(&filter);

    let resp = client()
        .get(format!("{}/query?filter={}", GATEWAY_URL, encoded))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().expect("Invalid JSON response");
    let events = body["events"].as_array().unwrap();

    // Should find jack's profile
    if !events.is_empty() {
        assert_eq!(events[0]["kind"], 0);
        assert_eq!(
            events[0]["pubkey"],
            "82341f882b6eabcd2ba7f1ef90aad961cf074af15b9ef44a09f9d2a8fbfbe6a2"
        );
    }
}

#[test]
fn test_query_with_limit() {
    let filter = serde_json::json!({
        "kinds": [1],
        "limit": 5
    });
    let encoded = encode_filter(&filter);

    let resp = client()
        .get(format!("{}/query?filter={}", GATEWAY_URL, encoded))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().expect("Invalid JSON response");
    let events = body["events"].as_array().unwrap();

    // Should respect limit (may return fewer)
    assert!(events.len() <= 5);
}

// ============================================================================
// Profile endpoint tests
// ============================================================================

#[test]
fn test_profile_endpoint() {
    // Jack's pubkey
    let pubkey = "82341f882b6eabcd2ba7f1ef90aad961cf074af15b9ef44a09f9d2a8fbfbe6a2";

    let resp = client()
        .get(format!("{}/profile/{}", GATEWAY_URL, pubkey))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().expect("Invalid JSON response");
    let events = body["events"].as_array().unwrap();

    if !events.is_empty() {
        // Should be a kind 0 event
        assert_eq!(events[0]["kind"], 0);
        assert_eq!(events[0]["pubkey"], pubkey);

        // Content should be parseable JSON (profile metadata)
        let content = events[0]["content"].as_str().unwrap();
        let profile: Value = serde_json::from_str(content).expect("Profile content should be JSON");
        assert!(profile.is_object());
    }
}

#[test]
fn test_profile_nonexistent() {
    // Random pubkey that likely doesn't exist
    let pubkey = "0000000000000000000000000000000000000000000000000000000000000000";

    let resp = client()
        .get(format!("{}/profile/{}", GATEWAY_URL, pubkey))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().expect("Invalid JSON response");
    let events = body["events"].as_array().unwrap();

    // Should return empty events array
    assert!(events.is_empty());
}

// ============================================================================
// Event endpoint tests
// ============================================================================

#[test]
fn test_event_endpoint_not_found() {
    // Random event ID that likely doesn't exist
    let event_id = "0000000000000000000000000000000000000000000000000000000000000000";

    let resp = client()
        .get(format!("{}/event/{}", GATEWAY_URL, event_id))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().expect("Invalid JSON response");
    let events = body["events"].as_array().unwrap();

    // Should return empty events array
    assert!(events.is_empty());
}

// ============================================================================
// Publish endpoint tests (auth required)
// ============================================================================

#[test]
fn test_publish_requires_auth() {
    let event = serde_json::json!({
        "event": {
            "id": "test",
            "pubkey": "test",
            "created_at": 0,
            "kind": 1,
            "tags": [],
            "content": "test",
            "sig": "test"
        }
    });

    let resp = client()
        .post(format!("{}/publish", GATEWAY_URL))
        .json(&event)
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 401);

    let body: Value = resp.json().expect("Invalid JSON response");
    assert_eq!(body["error"], "auth_failed");
}

#[test]
fn test_publish_invalid_auth_format() {
    let event = serde_json::json!({
        "event": {
            "id": "test",
            "pubkey": "test",
            "created_at": 0,
            "kind": 1,
            "tags": [],
            "content": "test",
            "sig": "test"
        }
    });

    let resp = client()
        .post(format!("{}/publish", GATEWAY_URL))
        .header("Authorization", "Bearer invalid")
        .json(&event)
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 401);

    let body: Value = resp.json().expect("Invalid JSON response");
    assert_eq!(body["error"], "auth_failed");
}

// ============================================================================
// Publish status endpoint tests
// ============================================================================

#[test]
fn test_publish_status_not_found() {
    let event_id = "nonexistent_event_id";

    let resp = client()
        .get(format!("{}/publish/status/{}", GATEWAY_URL, event_id))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 404);

    let body: Value = resp.json().expect("Invalid JSON response");
    assert_eq!(body["error"], "not_found");
}

// ============================================================================
// 404 tests
// ============================================================================

#[test]
fn test_unknown_endpoint() {
    let resp = client()
        .get(format!("{}/unknown/endpoint", GATEWAY_URL))
        .send()
        .expect("Failed to reach gateway");

    assert_eq!(resp.status(), 404);

    let body: Value = resp.json().expect("Invalid JSON response");
    assert_eq!(body["error"], "not_found");
}

// ============================================================================
// Response format validation
// ============================================================================

#[test]
fn test_content_type_json() {
    let filter = serde_json::json!({"kinds": [0], "limit": 1});
    let encoded = encode_filter(&filter);

    let resp = client()
        .get(format!("{}/query?filter={}", GATEWAY_URL, encoded))
        .send()
        .expect("Failed to reach gateway");

    let content_type = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("application/json"));
}

#[test]
fn test_events_have_required_fields() {
    let filter = serde_json::json!({"kinds": [1], "limit": 1});
    let encoded = encode_filter(&filter);

    let resp = client()
        .get(format!("{}/query?filter={}", GATEWAY_URL, encoded))
        .send()
        .expect("Failed to reach gateway");

    let body: Value = resp.json().expect("Invalid JSON response");
    let events = body["events"].as_array().unwrap();

    for event in events {
        // All Nostr events must have these fields
        assert!(event["id"].is_string(), "Event missing 'id'");
        assert!(event["pubkey"].is_string(), "Event missing 'pubkey'");
        assert!(event["created_at"].is_number(), "Event missing 'created_at'");
        assert!(event["kind"].is_number(), "Event missing 'kind'");
        assert!(event["tags"].is_array(), "Event missing 'tags'");
        assert!(event["content"].is_string(), "Event missing 'content'");
        assert!(event["sig"].is_string(), "Event missing 'sig'");
    }
}

// ============================================================================
// Cache behavior tests
// ============================================================================

#[test]
fn test_second_request_is_cached() {
    let filter = serde_json::json!({
        "kinds": [0],
        "limit": 1
    });
    let encoded = encode_filter(&filter);
    let url = format!("{}/query?filter={}", GATEWAY_URL, encoded);

    // First request
    let resp1 = client().get(&url).send().expect("Failed to reach gateway");
    assert_eq!(resp1.status(), 200);
    let body1: Value = resp1.json().unwrap();

    // Second request should be cached
    let resp2 = client().get(&url).send().expect("Failed to reach gateway");
    assert_eq!(resp2.status(), 200);
    let body2: Value = resp2.json().unwrap();

    // Second response should indicate it was cached
    // (first might or might not be cached depending on prior requests)
    if body1["cached"] == false {
        assert_eq!(body2["cached"], true, "Second request should be cached");
    }
}
