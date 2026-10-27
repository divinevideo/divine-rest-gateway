// ABOUTME: Exercises the real Worker handler and SQLite quota concurrency under local Wrangler.
// ABOUTME: Requires an enabled local Worker with canonical local-upstream and an upstream bearer.
use base64::{engine::general_purpose::STANDARD, Engine};
use k256::schnorr::SigningKey;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const ENDPOINT: &str = "https://gateway.divine.video/api/links";
const LOCAL: &str = "http://127.0.0.1:9785/api/links";

fn authorization(body: &[u8], identity: u8) -> String {
    let key = SigningKey::from_bytes(&[identity; 32]).unwrap();
    let pubkey = hex::encode(key.verifying_key().to_bytes());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let tags = serde_json::json!([
        ["u", ENDPOINT],
        ["method", "POST"],
        ["payload", hex::encode(Sha256::digest(body))]
    ]);
    let serialized = serde_json::json!([0, pubkey, now, 27235, tags, ""]).to_string();
    let id: [u8; 32] = Sha256::digest(serialized.as_bytes()).into();
    let signature = key.sign_prehash_with_aux_rand(&id, &[0u8; 32]).unwrap();
    let event = serde_json::json!({"id":hex::encode(id),"pubkey":pubkey,"created_at":now,"kind":27235,"tags":tags,"content":"","sig":hex::encode(signature.to_bytes())});
    format!("Nostr {}", STANDARD.encode(event.to_string().as_bytes()))
}

#[test]
#[ignore = "requires isolated local Wrangler on9785; see protected short-link design"]
fn protected_creation_http_and_concurrent_quota() {
    let client = Arc::new(
        Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap(),
    );
    let preflight = client
        .request(reqwest::Method::OPTIONS, LOCAL)
        .header("Origin", "https://divine.video")
        .send()
        .unwrap();
    assert_eq!(preflight.status(), 204);
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "https://divine.video"
    );
    assert_eq!(preflight.headers()["cache-control"], "no-store");
    let foreign = client
        .post(LOCAL)
        .header("Origin", "https://evil.example")
        .send()
        .unwrap();
    assert_eq!(foreign.status(), 403);
    assert!(!foreign
        .headers()
        .contains_key("access-control-allow-origin"));
    assert_eq!(
        client
            .post(LOCAL)
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .unwrap()
            .status(),
        401
    );
    let invalid = br#"{"url":"https://evil.example/video/synthetic"}"#;
    assert_eq!(
        client
            .post(LOCAL)
            .header("Content-Type", "application/json")
            .header("Authorization", authorization(invalid, 1))
            .body(invalid.to_vec())
            .send()
            .unwrap()
            .status(),
        400
    );
    let body = br#"{"url":"https://divine.video/video/shortener-gateway-verification"}"#;
    let signed = authorization(body, 2);
    let changed = client
        .post(LOCAL)
        .header("Content-Type", "application/json")
        .header("Authorization", &signed)
        .body("{}")
        .send()
        .unwrap();
    assert_eq!(changed.status(), 401);
    let created = client
        .post(LOCAL)
        .header("Content-Type", "application/json")
        .header("Authorization", &signed)
        .body(body.to_vec())
        .send()
        .unwrap();
    assert!(
        created.status().is_success(),
        "creator status{}",
        created.status()
    );
    let mapping: serde_json::Value = created.json().unwrap();
    assert_eq!(
        mapping["url"],
        "https://divine.video/video/shortener-gateway-verification"
    );
    assert_eq!(mapping["code"].as_str().unwrap().len(), 8);
    assert_eq!(
        mapping["short_url"],
        format!(
            "https://divine.video/l/{}",
            mapping["code"].as_str().unwrap()
        )
    );
    // A separate synthetic identity gets exactly30 admits despite concurrent calls.
    let signed = Arc::new(authorization(body, 3));
    let jobs: Vec<_> = (0..35)
        .map(|_| {
            let client = client.clone();
            let signed = signed.clone();
            std::thread::spawn(move || {
                client
                    .post(LOCAL)
                    .header("Content-Type", "application/json")
                    .header("Authorization", signed.as_str())
                    .body(body.to_vec())
                    .send()
                    .unwrap()
                    .status()
                    .as_u16()
            })
        })
        .collect();
    let statuses: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    assert_eq!(
        statuses.iter().filter(|&&status| status == 429).count(),
        5,
        "{statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .all(|status| [200, 201, 429].contains(status)),
        "{statuses:?}"
    );
}

#[test]
#[ignore = "requires isolated local Wrangler on9785; see protected short-link design"]
fn ip_quota_applies_across_authenticated_identities() {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        % 60;
    if seconds > 45 {
        std::thread::sleep(Duration::from_secs(61 - seconds));
    }
    let client = Arc::new(
        Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap(),
    );
    let body = br#"{"url":"https://divine.video/video/shortener-gateway-verification"}"#;
    let jobs: Vec<_> = (4..9)
        .flat_map(|identity| {
            let signed = Arc::new(authorization(body, identity));
            let client = client.clone();
            (0..25).map(move |_| {
                let signed = signed.clone();
                let client = client.clone();
                std::thread::spawn(move || {
                    client
                        .post(LOCAL)
                        .header("Content-Type", "application/json")
                        .header("CF-Connecting-IP", "192.0.2.43")
                        .header("Authorization", signed.as_str())
                        .body(body.to_vec())
                        .send()
                        .unwrap()
                        .status()
                        .as_u16()
                })
            })
        })
        .collect();
    let statuses: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    assert_eq!(
        statuses.iter().filter(|&&status| status == 429).count(),
        5,
        "{statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .all(|status| [200, 201, 429, 503].contains(status)),
        "{statuses:?}"
    );
}
