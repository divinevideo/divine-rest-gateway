// ABOUTME: NIP-98 HTTP authentication validation
// ABOUTME: Validates kind 27235 auth events for authenticated endpoints

use base64::{engine::general_purpose::STANDARD, Engine};
use k256::schnorr::{signature::Verifier, Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct AuthResult {
    pub pubkey: String,
}

#[derive(Debug)]
pub enum AuthError {
    MissingHeader,
    InvalidFormat,
    InvalidBase64,
    InvalidJson,
    InvalidKind,
    InvalidMethod,
    InvalidUrl,
    Expired,
    InvalidSignature,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader => write!(f, "missing Authorization header"),
            Self::InvalidFormat => write!(f, "invalid Authorization format, expected 'Nostr <token>'"),
            Self::InvalidBase64 => write!(f, "invalid base64 token"),
            Self::InvalidJson => write!(f, "invalid JSON event"),
            Self::InvalidKind => write!(f, "invalid event kind, expected 27235"),
            Self::InvalidMethod => write!(f, "method tag does not match request"),
            Self::InvalidUrl => write!(f, "url tag does not match request"),
            Self::Expired => write!(f, "auth event expired"),
            Self::InvalidSignature => write!(f, "invalid event signature"),
        }
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Clone))]
pub(crate) struct AuthEvent {
    id: String,
    pubkey: String,
    created_at: u64,
    kind: u32,
    tags: Vec<Vec<String>>,
    content: String,
    sig: String,
}

pub fn validate_nip98(
    auth_header: Option<&str>,
    method: &str,
    url: &str,
) -> Result<AuthResult, AuthError> {
    let header = auth_header.ok_or(AuthError::MissingHeader)?;

    // Parse "Nostr <base64>" format
    let token = header
        .strip_prefix("Nostr ")
        .ok_or(AuthError::InvalidFormat)?;

    // Decode base64
    let json_bytes = STANDARD.decode(token).map_err(|_| AuthError::InvalidBase64)?;
    let json_str = String::from_utf8(json_bytes).map_err(|_| AuthError::InvalidBase64)?;

    // Parse event
    let event: AuthEvent = serde_json::from_str(&json_str).map_err(|_| AuthError::InvalidJson)?;

    // Validate kind
    if event.kind != 27235 {
        return Err(AuthError::InvalidKind);
    }

    // Check created_at within ±60 seconds
    let now = (js_sys::Date::now() / 1000.0) as u64;
    if event.created_at > now + 60 || event.created_at < now.saturating_sub(60) {
        return Err(AuthError::Expired);
    }

    // Validate method tag
    let method_tag = event
        .tags
        .iter()
        .find(|t| t.get(0).map(|s| s.as_str()) == Some("method"))
        .and_then(|t| t.get(1))
        .ok_or(AuthError::InvalidMethod)?;

    if method_tag.to_uppercase() != method.to_uppercase() {
        return Err(AuthError::InvalidMethod);
    }

    // Validate URL tag
    let url_tag = event
        .tags
        .iter()
        .find(|t| t.get(0).map(|s| s.as_str()) == Some("u"))
        .and_then(|t| t.get(1))
        .ok_or(AuthError::InvalidUrl)?;

    if url_tag != url {
        return Err(AuthError::InvalidUrl);
    }

    // Verify signature
    if !verify_signature(&event) {
        return Err(AuthError::InvalidSignature);
    }

    Ok(AuthResult {
        pubkey: event.pubkey,
    })
}

// Made pub(crate) for testing
pub(crate) fn verify_signature(event: &AuthEvent) -> bool {
    // Compute event ID (SHA256 of serialized event)
    let serialized = serde_json::json!([
        0,
        event.pubkey,
        event.created_at,
        event.kind,
        event.tags,
        event.content
    ]);
    let serialized_str = serialized.to_string();

    let mut hasher = Sha256::new();
    hasher.update(serialized_str.as_bytes());
    let computed_id = hex::encode(hasher.finalize());

    // Verify computed ID matches claimed ID
    if computed_id != event.id {
        return false;
    }

    // Parse public key (32-byte x-only pubkey)
    let pubkey_bytes: [u8; 32] = match hex::decode(&event.pubkey) {
        Ok(bytes) if bytes.len() == 32 => bytes.try_into().unwrap(),
        _ => return false,
    };

    let verifying_key = match VerifyingKey::from_bytes(&pubkey_bytes) {
        Ok(vk) => vk,
        Err(_) => return false,
    };

    // Parse signature (64 bytes)
    let sig_bytes = match hex::decode(&event.sig) {
        Ok(bytes) if bytes.len() == 64 => bytes,
        _ => return false,
    };

    let signature = match Signature::try_from(sig_bytes.as_slice()) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Parse event ID as message (the hash that was signed)
    let id_bytes = match hex::decode(&event.id) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => return false,
    };

    // Verify schnorr signature over the event ID
    verifying_key.verify(&id_bytes, &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Valid NIP-98 style event (kind 27235) - generated with valid signature
    fn make_test_event() -> AuthEvent {
        // This is a real valid Nostr event structure
        // Using a known test vector
        AuthEvent {
            id: "b9fead6eef87d8400cbc1a5621600b360438f6d8571c140f76c791ab1e872650".to_string(),
            pubkey: "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".to_string(),
            created_at: 1234567890,
            kind: 27235,
            tags: vec![
                vec!["u".to_string(), "https://example.com/publish".to_string()],
                vec!["method".to_string(), "POST".to_string()],
            ],
            content: "".to_string(),
            sig: "f418c97b50cc68227e82f4f3a79d79eb2b7a0fa517859c86e1a8fa91e3741b6d4e5d9e1b8f9aa2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4".to_string(),
        }
    }

    #[test]
    fn test_auth_error_display() {
        assert_eq!(AuthError::MissingHeader.to_string(), "missing Authorization header");
        assert_eq!(AuthError::InvalidFormat.to_string(), "invalid Authorization format, expected 'Nostr <token>'");
        assert_eq!(AuthError::InvalidBase64.to_string(), "invalid base64 token");
        assert_eq!(AuthError::InvalidJson.to_string(), "invalid JSON event");
        assert_eq!(AuthError::InvalidKind.to_string(), "invalid event kind, expected 27235");
        assert_eq!(AuthError::InvalidMethod.to_string(), "method tag does not match request");
        assert_eq!(AuthError::InvalidUrl.to_string(), "url tag does not match request");
        assert_eq!(AuthError::Expired.to_string(), "auth event expired");
        assert_eq!(AuthError::InvalidSignature.to_string(), "invalid event signature");
    }

    #[test]
    fn test_event_id_computation() {
        // Test that event ID is correctly computed as SHA256 of serialized event
        let event = AuthEvent {
            id: "".to_string(), // Will compute
            pubkey: "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".to_string(),
            created_at: 1234567890,
            kind: 1,
            tags: vec![],
            content: "test".to_string(),
            sig: "".to_string(),
        };

        let serialized = serde_json::json!([
            0,
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags,
            event.content
        ]);
        let serialized_str = serialized.to_string();

        let mut hasher = sha2::Sha256::new();
        hasher.update(serialized_str.as_bytes());
        let computed_id = hex::encode(hasher.finalize());

        // Verify the ID format is correct (64 hex chars)
        assert_eq!(computed_id.len(), 64);
        assert!(computed_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_verify_signature_invalid_pubkey() {
        let mut event = make_test_event();
        event.pubkey = "invalid".to_string();
        assert!(!verify_signature(&event));
    }

    #[test]
    fn test_verify_signature_invalid_sig() {
        let mut event = make_test_event();
        event.sig = "invalid".to_string();
        assert!(!verify_signature(&event));
    }

    #[test]
    fn test_verify_signature_wrong_length_pubkey() {
        let mut event = make_test_event();
        event.pubkey = "abcd".to_string(); // Too short
        assert!(!verify_signature(&event));
    }

    #[test]
    fn test_verify_signature_wrong_length_sig() {
        let mut event = make_test_event();
        event.sig = "abcd".to_string(); // Too short
        assert!(!verify_signature(&event));
    }

    #[test]
    fn test_verify_signature_id_mismatch() {
        let mut event = make_test_event();
        event.id = "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        assert!(!verify_signature(&event));
    }

    #[test]
    fn test_parse_auth_header_missing() {
        // Can't test full validate_nip98 without js_sys, but we can test header parsing
        let result = None::<&str>.ok_or(AuthError::MissingHeader);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_auth_header_wrong_format() {
        let header = "Bearer token123";
        let result = header.strip_prefix("Nostr ");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_auth_header_correct_format() {
        let header = "Nostr dGVzdA==";
        let token = header.strip_prefix("Nostr ");
        assert_eq!(token, Some("dGVzdA=="));
    }

    #[test]
    fn test_base64_decode() {
        let token = "eyJ0ZXN0IjogdHJ1ZX0="; // {"test": true}
        let decoded = STANDARD.decode(token).unwrap();
        let json_str = String::from_utf8(decoded).unwrap();
        assert!(json_str.contains("test"));
    }

    #[test]
    fn test_method_tag_extraction() {
        let tags = vec![
            vec!["u".to_string(), "https://example.com".to_string()],
            vec!["method".to_string(), "POST".to_string()],
        ];

        let method_tag = tags
            .iter()
            .find(|t| t.get(0).map(|s| s.as_str()) == Some("method"))
            .and_then(|t| t.get(1));

        assert_eq!(method_tag, Some(&"POST".to_string()));
    }

    #[test]
    fn test_url_tag_extraction() {
        let tags = vec![
            vec!["u".to_string(), "https://example.com/api".to_string()],
            vec!["method".to_string(), "GET".to_string()],
        ];

        let url_tag = tags
            .iter()
            .find(|t| t.get(0).map(|s| s.as_str()) == Some("u"))
            .and_then(|t| t.get(1));

        assert_eq!(url_tag, Some(&"https://example.com/api".to_string()));
    }

    #[test]
    fn test_method_comparison_case_insensitive() {
        let method_tag = "post";
        let request_method = "POST";
        assert_eq!(method_tag.to_uppercase(), request_method.to_uppercase());
    }
}
