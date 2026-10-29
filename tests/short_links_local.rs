// ABOUTME: Exercises anonymous creation and SQLite IP quotas through the real local Worker.
// ABOUTME: Requires local Wrangler on 9785 with canonical upstream and a server-only bearer.
use reqwest::blocking::Client;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LOCAL: &str = "http://127.0.0.1:9785/api/links";
const BODY: &str = r#"{"url":"https://divine.video/video/shortener-gateway-verification"}"#;

fn avoid_quota_window_boundary() {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        % 60;
    if seconds > 30 {
        std::thread::sleep(Duration::from_secs(61 - seconds));
    }
}

#[test]
#[ignore = "requires isolated local Wrangler on 9785; see short-link runtime verification"]
fn anonymous_creation_http() {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
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
    for body in ["{}", r#"{"url":"https://evil.example/video/synthetic"}"#] {
        let invalid = client
            .post(LOCAL)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .unwrap();
        assert_eq!(invalid.status(), 400);
    }
    let created = client
        .post(LOCAL)
        .header("Origin", "https://divine.video")
        .header("Content-Type", "application/json")
        .body(BODY)
        .send()
        .unwrap();
    assert!(
        created.status().is_success(),
        "anonymous creator status {}",
        created.status()
    );
    assert_eq!(created.headers()["cache-control"], "no-store");
    assert_eq!(
        created.headers()["access-control-allow-origin"],
        "https://divine.video"
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
    // Older clients may still send NIP-98; it is ignored rather than required.
    let duplicate = client
        .post(LOCAL)
        .header("Content-Type", "application/json")
        .header("Authorization", "Nostr obsolete")
        .body(BODY)
        .send()
        .unwrap();
    assert_eq!(duplicate.status(), 200);
    assert_eq!(duplicate.json::<serde_json::Value>().unwrap(), mapping);
}

#[test]
#[ignore = "requires isolated local Wrangler on 9785; see short-link runtime verification"]
fn anonymous_ip_quota_is_atomic() {
    avoid_quota_window_boundary();
    let client = Arc::new(
        Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap(),
    );
    let jobs: Vec<_> = (0..125)
        .map(|_| {
            let client = client.clone();
            std::thread::spawn(move || {
                let response = client
                    .post(LOCAL)
                    .header("Content-Type", "application/json")
                    .header("CF-Connecting-IP", "192.0.2.43")
                    .body(BODY)
                    .send()
                    .unwrap();
                let status = response.status().as_u16();
                if status == 429 {
                    assert_eq!(response.headers()["retry-after"], "60");
                }
                status
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
