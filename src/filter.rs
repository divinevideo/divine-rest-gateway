// ABOUTME: Nostr filter parsing, validation, and base64url encoding
// ABOUTME: Handles conversion between HTTP query params and Nostr filter objects

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Filter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(rename = "#e", skip_serializing_if = "Option::is_none")]
    pub e_tags: Option<Vec<String>>,
    #[serde(rename = "#p", skip_serializing_if = "Option::is_none")]
    pub p_tags: Option<Vec<String>>,
}

impl Filter {
    /// Decode a base64url-encoded filter from query string
    pub fn from_base64(encoded: &str) -> Result<Self, FilterError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| FilterError::InvalidBase64)?;
        let json = String::from_utf8(bytes).map_err(|_| FilterError::InvalidUtf8)?;
        serde_json::from_str(&json).map_err(|_| FilterError::InvalidJson)
    }

    /// Encode filter to base64url for use in URLs
    pub fn to_base64(&self) -> String {
        let json = serde_json::to_string(self).expect("filter serialization cannot fail");
        URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    /// Generate cache key hash for this filter
    pub fn cache_key(&self) -> String {
        let json = serde_json::to_string(self).expect("filter serialization cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        let hash = hasher.finalize();
        format!("query:{}", hex::encode(&hash[..16])) // 128-bit truncated
    }

    /// Determine TTL in seconds based on filter content
    pub fn ttl_seconds(&self) -> u64 {
        match self.kinds.as_ref().and_then(|k| k.first()) {
            Some(0) => 900,   // profiles: 15 min
            Some(3) => 600,   // contacts: 10 min
            Some(1) => 300,   // notes: 5 min
            Some(7) => 120,   // reactions: 2 min
            _ => 300,         // default: 5 min
        }
    }

    /// Check if this is a single-event lookup by ID
    pub fn is_single_event_lookup(&self) -> bool {
        matches!(&self.ids, Some(ids) if ids.len() == 1)
            && self.authors.is_none()
            && self.kinds.is_none()
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
        let filter = Filter {
            authors: Some(vec!["abc123".to_string()]),
            kinds: Some(vec![1]),
            limit: Some(20),
            ids: None,
            since: None,
            until: None,
            e_tags: None,
            p_tags: None,
        };

        let encoded = filter.to_base64();
        let decoded = Filter::from_base64(&encoded).unwrap();
        assert_eq!(filter, decoded);
    }

    #[test]
    fn test_filter_roundtrip_all_fields() {
        let filter = Filter {
            ids: Some(vec!["id1".to_string(), "id2".to_string()]),
            authors: Some(vec!["author1".to_string()]),
            kinds: Some(vec![1, 6, 7]),
            since: Some(1700000000),
            until: Some(1700100000),
            limit: Some(100),
            e_tags: Some(vec!["event1".to_string()]),
            p_tags: Some(vec!["pubkey1".to_string()]),
        };

        let encoded = filter.to_base64();
        let decoded = Filter::from_base64(&encoded).unwrap();
        assert_eq!(filter, decoded);
    }

    #[test]
    fn test_filter_empty() {
        let filter = Filter::default();
        let encoded = filter.to_base64();
        let decoded = Filter::from_base64(&encoded).unwrap();
        assert_eq!(filter, decoded);
    }

    #[test]
    fn test_cache_key_deterministic() {
        let filter = Filter {
            authors: Some(vec!["abc".to_string()]),
            kinds: Some(vec![1]),
            limit: None,
            ids: None,
            since: None,
            until: None,
            e_tags: None,
            p_tags: None,
        };

        let key1 = filter.cache_key();
        let key2 = filter.cache_key();
        assert_eq!(key1, key2);
        assert!(key1.starts_with("query:"));
    }

    #[test]
    fn test_cache_key_different_for_different_filters() {
        let filter1 = Filter {
            authors: Some(vec!["abc".to_string()]),
            ..Default::default()
        };
        let filter2 = Filter {
            authors: Some(vec!["def".to_string()]),
            ..Default::default()
        };

        assert_ne!(filter1.cache_key(), filter2.cache_key());
    }

    #[test]
    fn test_cache_key_length() {
        let filter = Filter::default();
        let key = filter.cache_key();
        // "query:" prefix (6 chars) + 32 hex chars (16 bytes) = 38 chars
        assert_eq!(key.len(), 38);
    }

    #[test]
    fn test_ttl_by_kind() {
        let profile_filter = Filter {
            kinds: Some(vec![0]),
            ..Default::default()
        };
        assert_eq!(profile_filter.ttl_seconds(), 900); // 15 min

        let note_filter = Filter {
            kinds: Some(vec![1]),
            ..Default::default()
        };
        assert_eq!(note_filter.ttl_seconds(), 300); // 5 min
    }

    #[test]
    fn test_ttl_contacts() {
        let filter = Filter {
            kinds: Some(vec![3]),
            ..Default::default()
        };
        assert_eq!(filter.ttl_seconds(), 600); // 10 min
    }

    #[test]
    fn test_ttl_reactions() {
        let filter = Filter {
            kinds: Some(vec![7]),
            ..Default::default()
        };
        assert_eq!(filter.ttl_seconds(), 120); // 2 min
    }

    #[test]
    fn test_ttl_default() {
        let filter = Filter {
            kinds: Some(vec![30023]), // Long-form content
            ..Default::default()
        };
        assert_eq!(filter.ttl_seconds(), 300); // 5 min default

        let filter_no_kind = Filter::default();
        assert_eq!(filter_no_kind.ttl_seconds(), 300); // 5 min default
    }

    #[test]
    fn test_is_single_event_lookup_true() {
        let filter = Filter {
            ids: Some(vec!["abc123".to_string()]),
            ..Default::default()
        };
        assert!(filter.is_single_event_lookup());
    }

    #[test]
    fn test_is_single_event_lookup_false_multiple_ids() {
        let filter = Filter {
            ids: Some(vec!["abc".to_string(), "def".to_string()]),
            ..Default::default()
        };
        assert!(!filter.is_single_event_lookup());
    }

    #[test]
    fn test_is_single_event_lookup_false_with_authors() {
        let filter = Filter {
            ids: Some(vec!["abc".to_string()]),
            authors: Some(vec!["author".to_string()]),
            ..Default::default()
        };
        assert!(!filter.is_single_event_lookup());
    }

    #[test]
    fn test_is_single_event_lookup_false_with_kinds() {
        let filter = Filter {
            ids: Some(vec!["abc".to_string()]),
            kinds: Some(vec![1]),
            ..Default::default()
        };
        assert!(!filter.is_single_event_lookup());
    }

    #[test]
    fn test_is_single_event_lookup_false_no_ids() {
        let filter = Filter::default();
        assert!(!filter.is_single_event_lookup());
    }

    #[test]
    fn test_from_base64_invalid_base64() {
        let result = Filter::from_base64("not valid base64!!!");
        assert!(matches!(result, Err(FilterError::InvalidBase64)));
    }

    #[test]
    fn test_from_base64_invalid_utf8() {
        // Valid base64 but invalid UTF-8
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
    fn test_filter_serialization_skips_none() {
        let filter = Filter {
            kinds: Some(vec![1]),
            ..Default::default()
        };
        let json = serde_json::to_string(&filter).unwrap();
        // Should only contain "kinds", not null fields
        assert!(json.contains("kinds"));
        assert!(!json.contains("null"));
        assert!(!json.contains("authors"));
    }

    #[test]
    fn test_filter_tag_serialization() {
        let filter = Filter {
            e_tags: Some(vec!["event1".to_string()]),
            p_tags: Some(vec!["pubkey1".to_string()]),
            ..Default::default()
        };
        let json = serde_json::to_string(&filter).unwrap();
        // Tags should be serialized with # prefix
        assert!(json.contains("\"#e\""));
        assert!(json.contains("\"#p\""));
    }
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            ids: None,
            authors: None,
            kinds: None,
            since: None,
            until: None,
            limit: None,
            e_tags: None,
            p_tags: None,
        }
    }
}
