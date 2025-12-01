// ABOUTME: NIP-98 HTTP authentication validation
// ABOUTME: Validates kind 27235 auth events for authenticated endpoints

use base64::{engine::general_purpose::STANDARD, Engine};
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
struct AuthEvent {
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

fn verify_signature(event: &AuthEvent) -> bool {
    // Compute event ID
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

    if computed_id != event.id {
        return false;
    }

    // Verify schnorr signature using nostr crate's secp256k1 re-export
    use nostr::secp256k1::{schnorr::Signature, Message, Secp256k1, XOnlyPublicKey};

    let secp = Secp256k1::verification_only();

    // Parse public key
    let pubkey_bytes = match hex::decode(&event.pubkey) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let pubkey = match XOnlyPublicKey::from_slice(&pubkey_bytes) {
        Ok(pk) => pk,
        Err(_) => return false,
    };

    // Parse signature
    let sig_bytes = match hex::decode(&event.sig) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let sig = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Parse event ID as message
    let id_bytes = match hex::decode(&event.id) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let msg = match Message::from_digest_slice(&id_bytes) {
        Ok(m) => m,
        Err(_) => return false,
    };

    // Verify the signature
    secp.verify_schnorr(&sig, &msg, &pubkey).is_ok()
}
