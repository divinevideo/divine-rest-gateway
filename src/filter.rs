// ABOUTME: Nostr filter parsing, validation, and base64url encoding
// ABOUTME: Handles conversion between HTTP query params and Nostr filter objects

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Raw filter that preserves the exact JSON for cache keys and relay queries.
/// We keep the original JSON to ensure no fields are lost during parsing.
#[derive(Debug, Clone)]
pub struct Filter {
    /// The raw JSON string - passed directly to relays, used for cache key
    pub raw_json: String,
    /// Parsed filter for reading specific fields (TTL, limit, etc.)
    parsed: ParsedFilter,
}

/// Internal parsed representation for reading filter fields
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ParsedFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    authors: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kinds: Option<Vec<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
}

impl Filter {
    /// Create filter from raw JSON string
    pub fn from_json(raw_json: &str) -> Result<Self, FilterError> {
        // Validate it's valid JSON
        let _: serde_json::Value = serde_json::from_str(raw_json)
            .map_err(|_| FilterError::InvalidJson)?;

        // Parse known fields for TTL/limit lookups (ignoring unknown fields)
        let parsed: ParsedFilter = serde_json::from_str(raw_json).unwrap_or_default();

        Ok(Self { raw_json: raw_json.to_string(), parsed })
    }

    /// Decode a base64url-encoded filter from query string.
    /// Preserves the raw JSON for passing to relays unchanged.
    pub fn from_base64(encoded: &str) -> Result<Self, FilterError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| FilterError::InvalidBase64)?;
        let raw_json = String::from_utf8(bytes).map_err(|_| FilterError::InvalidUtf8)?;
        Self::from_json(&raw_json)
    }

    /// Encode filter to base64url for use in URLs
    pub fn to_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.raw_json.as_bytes())
    }

    /// Generate cache key hash from the RAW JSON - includes ALL fields
    pub fn cache_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.raw_json.as_bytes());
        let hash = hasher.finalize();
        format!("query:{}", hex::encode(&hash[..16])) // 128-bit truncated
    }

    /// Get the raw JSON for passing to relays
    pub fn as_json(&self) -> &str {
        &self.raw_json
    }

    /// Get limit if specified
    pub fn limit(&self) -> Option<usize> {
        self.parsed.limit
    }

    /// Determine TTL in seconds based on filter content
    pub fn ttl_seconds(&self) -> u64 {
        match self.parsed.kinds.as_ref().and_then(|k| k.first()) {
            Some(0) => 900,   // profiles: 15 min
            Some(3) => 600,   // contacts: 10 min
            Some(1) => 300,   // notes: 5 min
            Some(7) => 120,   // reactions: 2 min
            _ => 300,         // default: 5 min
        }
    }

    /// Check if this is a single-event lookup by ID
    pub fn is_single_event_lookup(&self) -> bool {
        matches!(&self.parsed.ids, Some(ids) if ids.len() == 1)
            && self.parsed.authors.is_none()
            && self.parsed.kinds.is_none()
    }
}

#[derive(Debug)]
pub enum FilterError {
    InvalidBase64,
    InvalidUtf8,
    InvalidJson,
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBase64 => write!(f, "invalid base64 encoding"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8"),
            Self::InvalidJson => write!(f, "invalid JSON filter"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_roundtrip() {
        let json = r#"{"authors":["abc123"],"kinds":[1],"limit":20}"#;
        let filter = Filter::from_json(json).unwrap();
        let encoded = filter.to_base64();
        let decoded = Filter::from_base64(&encoded).unwrap();
        assert_eq!(filter.raw_json, decoded.raw_json);
    }

    #[test]
    fn test_filter_preserves_custom_tags() {
        // This is the critical test - custom tags like #platform must be preserved
        let json = r##"{"kinds":[34236],"limit":20,"#platform":["vine"]}"##;
        let filter = Filter::from_json(json).unwrap();

        // Raw JSON must be preserved exactly
        assert_eq!(filter.raw_json, json);
        assert!(filter.as_json().contains("#platform"));
        assert!(filter.as_json().contains("vine"));

        // Round-trip through base64 must preserve
        let encoded = filter.to_base64();
        let decoded = Filter::from_base64(&encoded).unwrap();
        assert_eq!(decoded.raw_json, json);
    }

    #[test]
    fn test_cache_key_includes_all_fields() {
        // Two filters with different tags must have different cache keys
        let filter1 = Filter::from_json(r#"{"kinds":[34236],"limit":20}"#).unwrap();
        let filter2 = Filter::from_json(r##"{"kinds":[34236],"limit":20,"#platform":["vine"]}"##).unwrap();

        assert_ne!(filter1.cache_key(), filter2.cache_key());
    }

    #[test]
    fn test_cache_key_deterministic() {
        let json = r#"{"authors":["abc"],"kinds":[1]}"#;
        let filter = Filter::from_json(json).unwrap();

        let key1 = filter.cache_key();
        let key2 = filter.cache_key();
        assert_eq!(key1, key2);
        assert!(key1.starts_with("query:"));
    }

    #[test]
    fn test_cache_key_length() {
        let filter = Filter::from_json("{}").unwrap();
        let key = filter.cache_key();
        // "query:" prefix (6 chars) + 32 hex chars (16 bytes) = 38 chars
        assert_eq!(key.len(), 38);
    }

    #[test]
    fn test_ttl_by_kind() {
        let profile = Filter::from_json(r#"{"kinds":[0]}"#).unwrap();
        assert_eq!(profile.ttl_seconds(), 900); // 15 min

        let note = Filter::from_json(r#"{"kinds":[1]}"#).unwrap();
        assert_eq!(note.ttl_seconds(), 300); // 5 min

        let contacts = Filter::from_json(r#"{"kinds":[3]}"#).unwrap();
        assert_eq!(contacts.ttl_seconds(), 600); // 10 min

        let reactions = Filter::from_json(r#"{"kinds":[7]}"#).unwrap();
        assert_eq!(reactions.ttl_seconds(), 120); // 2 min
    }

    #[test]
    fn test_ttl_default() {
        let filter = Filter::from_json(r#"{"kinds":[30023]}"#).unwrap();
        assert_eq!(filter.ttl_seconds(), 300); // 5 min default

        let empty = Filter::from_json("{}").unwrap();
        assert_eq!(empty.ttl_seconds(), 300); // 5 min default
    }

    #[test]
    fn test_is_single_event_lookup() {
        let single = Filter::from_json(r#"{"ids":["abc123"]}"#).unwrap();
        assert!(single.is_single_event_lookup());

        let multiple = Filter::from_json(r#"{"ids":["a","b"]}"#).unwrap();
        assert!(!multiple.is_single_event_lookup());

        let with_authors = Filter::from_json(r#"{"ids":["a"],"authors":["x"]}"#).unwrap();
        assert!(!with_authors.is_single_event_lookup());

        let with_kinds = Filter::from_json(r#"{"ids":["a"],"kinds":[1]}"#).unwrap();
        assert!(!with_kinds.is_single_event_lookup());

        let no_ids = Filter::from_json("{}").unwrap();
        assert!(!no_ids.is_single_event_lookup());
    }

    #[test]
    fn test_from_base64_invalid_base64() {
        let result = Filter::from_base64("not valid base64!!!");
        assert!(matches!(result, Err(FilterError::InvalidBase64)));
    }

    #[test]
    fn test_from_base64_invalid_utf8() {
        let invalid_utf8 = URL_SAFE_NO_PAD.encode(&[0xFF, 0xFE]);
        let result = Filter::from_base64(&invalid_utf8);
        assert!(matches!(result, Err(FilterError::InvalidUtf8)));
    }

    #[test]
    fn test_from_base64_invalid_json() {
        let not_json = URL_SAFE_NO_PAD.encode(b"not json");
        let result = Filter::from_base64(&not_json);
        assert!(matches!(result, Err(FilterError::InvalidJson)));
    }

    #[test]
    fn test_filter_error_display() {
        assert_eq!(FilterError::InvalidBase64.to_string(), "invalid base64 encoding");
        assert_eq!(FilterError::InvalidUtf8.to_string(), "invalid UTF-8");
        assert_eq!(FilterError::InvalidJson.to_string(), "invalid JSON filter");
    }

    #[test]
    fn test_limit_extraction() {
        let filter = Filter::from_json(r#"{"limit":50}"#).unwrap();
        assert_eq!(filter.limit(), Some(50));

        let no_limit = Filter::from_json(r#"{"kinds":[1]}"#).unwrap();
        assert_eq!(no_limit.limit(), None);
    }
}
