// ABOUTME: NIP-98 HTTP authentication validation
// ABOUTME: Validates kind 27235 auth events for authenticated endpoints

use base64::{engine::general_purpose::STANDARD, Engine};
use k256::schnorr::{Signature, VerifyingKey};
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
    InvalidPayload,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader => write!(f, "missing Authorization header"),
            Self::InvalidFormat => {
                write!(f, "invalid Authorization format, expected 'Nostr <token>'")
            }
            Self::InvalidBase64 => write!(f, "invalid base64 token"),
            Self::InvalidJson => write!(f, "invalid JSON event"),
            Self::InvalidKind => write!(f, "invalid event kind, expected 27235"),
            Self::InvalidMethod => write!(f, "method tag does not match request"),
            Self::InvalidUrl => write!(f, "url tag does not match request"),
            Self::Expired => write!(f, "auth event expired"),
            Self::InvalidSignature => write!(f, "invalid event signature"),
            Self::InvalidPayload => write!(f, "payload tag does not match request"),
        }
    }
}

#[derive(Deserialize, serde::Serialize)]
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
    validate_at(
        auth_header,
        method,
        url,
        None,
        (js_sys::Date::now() / 1000.0) as u64,
    )
}

/// Strict body-bound authentication for the protected creator. The injected time
/// keeps the entire protocol validator testable without a JavaScript runtime.
pub fn validate_creation(
    auth_header: Option<&str>,
    url: &str,
    body: &[u8],
    now: u64,
) -> Result<AuthResult, AuthError> {
    validate_at(auth_header, "POST", url, Some(body), now)
}

fn validate_at(
    auth_header: Option<&str>,
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    now: u64,
) -> Result<AuthResult, AuthError> {
    let header = auth_header.ok_or(AuthError::MissingHeader)?;
    if header.len() > 16384 {
        return Err(AuthError::InvalidFormat);
    }
    let token = header
        .strip_prefix("Nostr ")
        .ok_or(AuthError::InvalidFormat)?;
    let bytes = STANDARD
        .decode(token)
        .map_err(|_| AuthError::InvalidBase64)?;
    let event: AuthEvent = serde_json::from_slice(&bytes).map_err(|_| AuthError::InvalidJson)?;
    if event.kind != 27235 {
        return Err(AuthError::InvalidKind);
    }
    if event.created_at.abs_diff(now) > 60 {
        return Err(AuthError::Expired);
    }
    let method_tag = unique_tag(&event, "method").ok_or(AuthError::InvalidMethod)?;
    if if body.is_some() {
        method_tag != method
    } else {
        !method_tag.eq_ignore_ascii_case(method)
    } {
        return Err(AuthError::InvalidMethod);
    }
    if unique_tag(&event, "u") != Some(url) {
        return Err(AuthError::InvalidUrl);
    }
    if let Some(body) = body {
        let digest = hex::encode(Sha256::digest(body));
        if unique_tag(&event, "payload") != Some(digest.as_str()) {
            return Err(AuthError::InvalidPayload);
        }
    }
    if !verify_signature(&event) {
        return Err(AuthError::InvalidSignature);
    }
    Ok(AuthResult {
        pubkey: event.pubkey.to_ascii_lowercase(),
    })
}

fn unique_tag<'a>(event: &'a AuthEvent, name: &str) -> Option<&'a str> {
    let mut tags = event
        .tags
        .iter()
        .filter(|tag| tag.first().map(String::as_str) == Some(name));
    let tag = tags.next()?;
    if tag.len() != 2 || tags.next().is_some() {
        return None;
    }
    Some(tag[1].as_str())
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
    verifying_key.verify_raw(&id_bytes, &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_nostr_signature_over_event_id_without_hashing_again() {
        let key = k256::schnorr::SigningKey::from_bytes(&[1u8; 32]).unwrap();
        let mut event = AuthEvent {
            id: String::new(),
            pubkey: hex::encode(key.verifying_key().to_bytes()),
            created_at: 1234567890,
            kind: 27235,
            tags: vec![vec!["method".into(), "POST".into()]],
            content: String::new(),
            sig: String::new(),
        };
        let serialized = serde_json::json!([
            0,
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags,
            event.content
        ])
        .to_string();
        let id: [u8; 32] = Sha256::digest(serialized.as_bytes()).into();
        event.id = hex::encode(id);
        event.sig = hex::encode(
            key.sign_prehash_with_aux_rand(&id, &[0u8; 32])
                .unwrap()
                .to_bytes(),
        );
        assert!(verify_signature(&event));
        event.content = "tampered".into();
        assert!(!verify_signature(&event));
    }

    fn signed_creation_header(body: &[u8], now: u64, extra: Option<Vec<String>>) -> String {
        signed_creation_header_with_case(body, now, extra, false)
    }

    fn signed_creation_header_with_case(
        body: &[u8],
        now: u64,
        extra: Option<Vec<String>>,
        uppercase: bool,
    ) -> String {
        let key = k256::schnorr::SigningKey::from_bytes(&[1u8; 32]).unwrap();
        let mut event = AuthEvent {
            id: String::new(),
            pubkey: hex::encode(key.verifying_key().to_bytes()),
            created_at: now,
            kind: 27235,
            content: String::new(),
            sig: String::new(),
            tags: vec![
                vec!["u".into(), "https://gateway.divine.video/api/links".into()],
                vec!["method".into(), "POST".into()],
                vec!["payload".into(), hex::encode(Sha256::digest(body))],
            ],
        };
        if uppercase {
            event.pubkey = event.pubkey.to_ascii_uppercase();
        }
        if let Some(tag) = extra {
            event.tags.push(tag);
        }
        let serialized = serde_json::json!([
            0,
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags,
            event.content
        ])
        .to_string();
        let id: [u8; 32] = Sha256::digest(serialized.as_bytes()).into();
        event.id = hex::encode(id);
        event.sig = hex::encode(
            key.sign_prehash_with_aux_rand(&id, &[0u8; 32])
                .unwrap()
                .to_bytes(),
        );
        format!(
            "Nostr {}",
            STANDARD.encode(serde_json::to_vec(&event).unwrap())
        )
    }

    #[test]
    fn equivalent_key_encodings_share_one_authenticated_identity() {
        let body = br#"{"url":"https://divine.video/video/synthetic"}"#;
        let url = "https://gateway.divine.video/api/links";
        let lower = signed_creation_header_with_case(body, 1000, None, false);
        let upper = signed_creation_header_with_case(body, 1000, None, true);
        let lower = validate_creation(Some(&lower), url, body, 1000).unwrap();
        let upper = validate_creation(Some(&upper), url, body, 1000).unwrap();
        assert_eq!(lower.pubkey, upper.pubkey);
    }

    #[test]
    fn creation_auth_binds_body_url_timestamp_and_unique_tags() {
        let body = br#"{"url":"https://divine.video/video/synthetic"}"#;
        let url = "https://gateway.divine.video/api/links";
        let header = signed_creation_header(body, 1000, None);
        assert!(validate_creation(Some(&header), url, body, 1000).is_ok());
        assert!(matches!(
            validate_creation(Some(&header), url, b"changed", 1000),
            Err(AuthError::InvalidPayload)
        ));
        assert!(matches!(
            validate_creation(Some(&header), "https://other.example/api/links", body, 1000),
            Err(AuthError::InvalidUrl)
        ));
        for now in [939, 1061, u64::MAX] {
            assert!(matches!(
                validate_creation(Some(&header), url, body, now),
                Err(AuthError::Expired)
            ));
        }
        for tag in [
            vec!["method".into(), "POST".into()],
            vec!["u".into(), url.into()],
            vec!["payload".into(), "bad".into()],
        ] {
            let header = signed_creation_header(body, 1000, Some(tag));
            assert!(validate_creation(Some(&header), url, body, 1000).is_err());
        }
        let tampered = header.replace("Nostr ", "Bearer ");
        assert!(validate_creation(Some(&tampered), url, body, 1000).is_err());
    }

    #[test]
    fn test_auth_error_display() {
        assert_eq!(
            AuthError::MissingHeader.to_string(),
            "missing Authorization header"
        );
        assert_eq!(
            AuthError::InvalidFormat.to_string(),
            "invalid Authorization format, expected 'Nostr <token>'"
        );
        assert_eq!(AuthError::InvalidBase64.to_string(), "invalid base64 token");
        assert_eq!(AuthError::InvalidJson.to_string(), "invalid JSON event");
        assert_eq!(
            AuthError::InvalidKind.to_string(),
            "invalid event kind, expected 27235"
        );
        assert_eq!(
            AuthError::InvalidMethod.to_string(),
            "method tag does not match request"
        );
        assert_eq!(
            AuthError::InvalidUrl.to_string(),
            "url tag does not match request"
        );
        assert_eq!(AuthError::Expired.to_string(), "auth event expired");
        assert_eq!(
            AuthError::InvalidSignature.to_string(),
            "invalid event signature"
        );
    }

    #[test]
    fn creation_auth_rejects_invalid_headers_and_binding_changes() {
        let body = br#"{"url":"https://divine.video/video/synthetic"}"#;
        let url = "https://gateway.divine.video/api/links";
        for header in [
            None,
            Some("Bearer test"),
            Some("Nostr !!!"),
            Some("Nostr e30="),
        ] {
            assert!(validate_creation(header, url, body, 1000).is_err());
        }
        let header = signed_creation_header(body, 1000, None);
        for mutation in ["method", "kind", "payload", "signature"] {
            let bytes = STANDARD
                .decode(header.strip_prefix("Nostr ").unwrap())
                .unwrap();
            let mut event: AuthEvent = serde_json::from_slice(&bytes).unwrap();
            match mutation {
                "method" => event.tags[1][1] = "GET".into(),
                "kind" => event.kind = 1,
                "payload" => {
                    event.tags.pop();
                }
                _ => event.sig = "00".repeat(64),
            }
            let changed = format!(
                "Nostr {}",
                STANDARD.encode(serde_json::to_vec(&event).unwrap())
            );
            assert!(
                validate_creation(Some(&changed), url, body, 1000).is_err(),
                "{mutation}"
            );
        }
    }
}
