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
    fn test_ttl_by_kind() {
        let profile_filter = Filter {
            kinds: Some(vec![0]),
            ..Default::default()
        };
        assert_eq!(profile_filter.ttl_seconds(), 900);

        let note_filter = Filter {
            kinds: Some(vec![1]),
            ..Default::default()
        };
        assert_eq!(note_filter.ttl_seconds(), 300);
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
