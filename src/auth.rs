// ABOUTME: NIP-98 HTTP authentication validation
// ABOUTME: Validates kind 27235 auth events for authenticated endpoints

use base64::{engine::general_purpose::STANDARD, Engine};
use k256::schnorr::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

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

pub fn validate_nip98(auth_header: Option<&str>, method: &str, url: &str) -> Result<(), AuthError> {
    validate_at(
        auth_header,
        method,
        url,
        (js_sys::Date::now() / 1000.0) as u64,
    )
}

fn validate_at(
    auth_header: Option<&str>,
    method: &str,
    url: &str,
    now: u64,
) -> Result<(), AuthError> {
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
    if !method_tag.eq_ignore_ascii_case(method) {
        return Err(AuthError::InvalidMethod);
    }
    if unique_tag(&event, "u") != Some(url) {
        return Err(AuthError::InvalidUrl);
    }
    if !verify_signature(&event) {
        return Err(AuthError::InvalidSignature);
    }
    Ok(())
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

    fn signed_header(now: u64, kind: u32, tags: Vec<Vec<String>>, uppercase: bool) -> String {
        let key = k256::schnorr::SigningKey::from_bytes(&[1u8; 32]).unwrap();
        let mut event = AuthEvent {
            id: String::new(),
            pubkey: hex::encode(key.verifying_key().to_bytes()),
            created_at: now,
            kind,
            content: String::new(),
            sig: String::new(),
            tags,
        };
        if uppercase {
            event.pubkey = event.pubkey.to_ascii_uppercase();
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

    const PUBLISH_URL: &str = "https://gateway.divine.video/publish";

    fn publish_tags() -> Vec<Vec<String>> {
        vec![
            vec!["u".into(), PUBLISH_URL.into()],
            vec!["method".into(), "POST".into()],
        ]
    }

    #[test]
    fn publish_auth_accepts_valid_signatures_and_equivalent_key_encodings() {
        for uppercase in [false, true] {
            let header = signed_header(1000, 27235, publish_tags(), uppercase);
            assert!(validate_at(Some(&header), "POST", PUBLISH_URL, 1000).is_ok());
        }
    }

    #[test]
    fn publish_auth_limits_clock_skew_in_both_directions() {
        let header = signed_header(1000, 27235, publish_tags(), false);
        for now in [940, 1060] {
            assert!(validate_at(Some(&header), "POST", PUBLISH_URL, now).is_ok());
        }
        for now in [939, 1061, u64::MAX] {
            assert!(matches!(
                validate_at(Some(&header), "POST", PUBLISH_URL, now),
                Err(AuthError::Expired)
            ));
        }
    }

    #[test]
    fn publish_auth_binds_exact_url_and_case_insensitive_method() {
        let header = signed_header(1000, 27235, publish_tags(), false);
        assert!(matches!(
            validate_at(Some(&header), "POST", "https://other.example/publish", 1000),
            Err(AuthError::InvalidUrl)
        ));
        assert!(matches!(
            validate_at(Some(&header), "GET", PUBLISH_URL, 1000),
            Err(AuthError::InvalidMethod)
        ));
        assert!(validate_at(Some(&header), "post", PUBLISH_URL, 1000).is_ok());
    }

    #[test]
    fn publish_auth_rejects_duplicate_missing_or_malformed_binding_tags() {
        for name in ["u", "method"] {
            for malformed in [
                None,
                Some(vec![name.into()]),
                Some(vec![name.into(), "value".into(), "extra".into()]),
            ] {
                let mut tags = publish_tags();
                tags.retain(|tag| tag[0] != name);
                if let Some(tag) = malformed {
                    tags.push(tag);
                }
                let header = signed_header(1000, 27235, tags, false);
                let result = validate_at(Some(&header), "POST", PUBLISH_URL, 1000);
                if name == "u" {
                    assert!(matches!(result, Err(AuthError::InvalidUrl)));
                } else {
                    assert!(matches!(result, Err(AuthError::InvalidMethod)));
                }
            }
            let mut tags = publish_tags();
            tags.push(tags.iter().find(|tag| tag[0] == name).unwrap().clone());
            let header = signed_header(1000, 27235, tags, false);
            let result = validate_at(Some(&header), "POST", PUBLISH_URL, 1000);
            if name == "u" {
                assert!(matches!(result, Err(AuthError::InvalidUrl)));
            } else {
                assert!(matches!(result, Err(AuthError::InvalidMethod)));
            }
        }
    }

    #[test]
    fn publish_auth_rejects_wrong_kind_and_invalid_signature() {
        let header = signed_header(1000, 1, publish_tags(), false);
        assert!(matches!(
            validate_at(Some(&header), "POST", PUBLISH_URL, 1000),
            Err(AuthError::InvalidKind)
        ));
        let header = signed_header(1000, 27235, publish_tags(), false);
        let bytes = STANDARD
            .decode(header.strip_prefix("Nostr ").unwrap())
            .unwrap();
        let mut event: AuthEvent = serde_json::from_slice(&bytes).unwrap();
        event.sig = "00".repeat(64);
        let tampered = format!(
            "Nostr {}",
            STANDARD.encode(serde_json::to_vec(&event).unwrap())
        );
        assert!(matches!(
            validate_at(Some(&tampered), "POST", PUBLISH_URL, 1000),
            Err(AuthError::InvalidSignature)
        ));
    }

    #[test]
    fn publish_auth_rejects_missing_malformed_and_oversized_headers() {
        assert!(matches!(
            validate_at(None, "POST", PUBLISH_URL, 1000),
            Err(AuthError::MissingHeader)
        ));
        assert!(matches!(
            validate_at(Some("Bearer test"), "POST", PUBLISH_URL, 1000),
            Err(AuthError::InvalidFormat)
        ));
        assert!(matches!(
            validate_at(Some("Nostr !!!"), "POST", PUBLISH_URL, 1000),
            Err(AuthError::InvalidBase64)
        ));
        assert!(matches!(
            validate_at(Some("Nostr e30="), "POST", PUBLISH_URL, 1000),
            Err(AuthError::InvalidJson)
        ));
        assert!(matches!(
            validate_at(Some(&"Nostr ".repeat(3000)), "POST", PUBLISH_URL, 1000),
            Err(AuthError::InvalidFormat)
        ));
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
}
