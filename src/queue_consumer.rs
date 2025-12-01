// ABOUTME: Cloudflare Queue consumer for processing event publishes
// ABOUTME: Handles publishing to relay with verification and retry logic

use crate::cache::Cache;
use crate::types::PublishStatus;
use worker::*;

pub async fn handle_queue(message_batch: MessageBatch<serde_json::Value>, env: Env) -> Result<()> {
    let relay_pool = env.durable_object("RELAY_POOL")?;
    let stub = relay_pool.id_from_name("default")?.get_stub()?;
    let kv = env.kv("REST_GATEWAY_CACHE")?;
    let cache = Cache::new(kv);

    for message in message_batch.messages()? {
        let event = message.body();
        let event_id = event
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Get current attempt count
        let current_status = cache.get_publish_status(&event_id).await?.unwrap_or(PublishStatus {
            status: "processing".to_string(),
            attempts: Some(0),
            verified_at: None,
            error: None,
        });
        let attempts = current_status.attempts.unwrap_or(0) + 1;

        // Update status to processing
        cache
            .set_publish_status(
                &event_id,
                &PublishStatus {
                    status: format!("attempt_{}", attempts),
                    attempts: Some(attempts),
                    verified_at: None,
                    error: None,
                },
            )
            .await?;

        // Publish to relay
        let publish_req = Request::new_with_init(
            "http://do/publish",
            RequestInit::new()
                .with_method(Method::Post)
                .with_body(Some(serde_json::to_string(&event)?.into())),
        )?;
        let mut publish_resp = stub.fetch_with_request(publish_req).await?;
        let publish_result: serde_json::Value = publish_resp.json().await?;
        let relay_ok = publish_result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);

        if !relay_ok {
            // Relay rejected - retry
            cache
                .set_publish_status(
                    &event_id,
                    &PublishStatus {
                        status: format!("retry_{}", attempts),
                        attempts: Some(attempts),
                        verified_at: None,
                        error: Some("relay rejected".to_string()),
                    },
                )
                .await?;
            message.retry();
            continue;
        }

        // Verify event exists on relay
        let verify_req = Request::new_with_init(
            "http://do/verify",
            RequestInit::new()
                .with_method(Method::Post)
                .with_body(Some(serde_json::json!({ "event_id": event_id }).to_string().into())),
        )?;
        let mut verify_resp = stub.fetch_with_request(verify_req).await?;
        let verify_result: serde_json::Value = verify_resp.json().await?;
        let found = verify_result.get("found").and_then(|v| v.as_bool()).unwrap_or(false);

        if found {
            // Success - mark as published
            let now = js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default();
            cache
                .set_publish_status(
                    &event_id,
                    &PublishStatus {
                        status: "published".to_string(),
                        attempts: Some(attempts),
                        verified_at: Some(now),
                        error: None,
                    },
                )
                .await?;
            message.ack();
        } else {
            // Not found - retry
            cache
                .set_publish_status(
                    &event_id,
                    &PublishStatus {
                        status: format!("retry_{}", attempts),
                        attempts: Some(attempts),
                        verified_at: None,
                        error: Some("event not found on relay".to_string()),
                    },
                )
                .await?;
            message.retry();
        }
    }

    Ok(())
}
