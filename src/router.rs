// ABOUTME: HTTP request routing for the REST gateway
// ABOUTME: Routes requests to appropriate handlers based on path and method

use crate::cache::Cache;
use crate::filter::Filter;
use crate::types::{ErrorResponse, QueryResponse};
use worker::*;

pub async fn handle_request(req: Request, env: Env) -> Result<Response> {
    let url = req.url()?;
    let path = url.path();
    let method = req.method();

    match (method, path) {
        (Method::Get, "/") => Response::ok("Divine REST Gateway v0.1.0"),

        (Method::Get, "/health") => Response::ok("ok"),

        (Method::Get, "/query") => handle_query(req, env).await,

        (Method::Get, path) if path.starts_with("/profile/") => {
            handle_profile(req, env, &path[9..]).await
        }

        (Method::Get, path) if path.starts_with("/event/") => {
            handle_event(req, env, &path[7..]).await
        }

        (Method::Get, path) if path.starts_with("/publish/status/") => {
            handle_publish_status(env, &path[16..]).await
        }

        (Method::Post, "/publish") => handle_publish(req, env).await,

        _ => {
            let err = ErrorResponse::new("not_found").with_detail("endpoint not found");
            json_response(&err, 404)
        }
    }
}

async fn handle_query(req: Request, env: Env) -> Result<Response> {
    let url = req.url()?;
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

    let filter_param = match params.get("filter") {
        Some(f) => f,
        None => {
            let err = ErrorResponse::new("invalid_filter").with_detail("missing filter parameter");
            return json_response(&err, 400);
        }
    };

    let filter = match Filter::from_base64(filter_param) {
        Ok(f) => f,
        Err(e) => {
            let err = ErrorResponse::new("invalid_filter").with_detail(&e.to_string());
            return json_response(&err, 400);
        }
    };

    let kv = env.kv("CACHE")?;
    let cache = Cache::new(kv);
    let cache_key = filter.cache_key();

    // Check cache first
    if let Some((cached, age)) = cache.get_query(&cache_key).await? {
        let response = QueryResponse {
            events: cached.events,
            eose: cached.eose,
            complete: cached.eose,
            cached: true,
            cache_age_seconds: Some(age),
        };
        return json_response_with_cache(&response, 200, filter.ttl_seconds());
    }

    // Cache miss - query relay via Durable Object
    let relay_pool = env.durable_object("RELAY_POOL")?;
    let stub = relay_pool.id_from_name("default")?.get_stub()?;

    let do_req = Request::new_with_init(
        "http://do/query",
        RequestInit::new()
            .with_method(Method::Post)
            .with_body(Some(serde_json::to_string(&filter)?.into())),
    )?;

    let mut do_resp = stub.fetch_with_request(do_req).await?;
    let events: Vec<serde_json::Value> = do_resp.json().await?;

    // Cache the result
    cache
        .put_query(&cache_key, events.clone(), true, filter.ttl_seconds())
        .await?;

    let response = QueryResponse {
        events,
        eose: true,
        complete: true,
        cached: false,
        cache_age_seconds: None,
    };
    json_response_with_cache(&response, 200, filter.ttl_seconds())
}

async fn handle_profile(_req: Request, env: Env, pubkey: &str) -> Result<Response> {
    let filter = Filter {
        authors: Some(vec![pubkey.to_string()]),
        kinds: Some(vec![0]),
        limit: Some(1),
        ..Default::default()
    };

    // Reuse query logic via internal request
    let encoded = filter.to_base64();
    let url = format!("http://internal/query?filter={}", encoded);
    let req = Request::new(&url, Method::Get)?;
    handle_query(req, env).await
}

async fn handle_event(_req: Request, env: Env, event_id: &str) -> Result<Response> {
    let filter = Filter {
        ids: Some(vec![event_id.to_string()]),
        limit: Some(1),
        ..Default::default()
    };

    let encoded = filter.to_base64();
    let url = format!("http://internal/query?filter={}", encoded);
    let req = Request::new(&url, Method::Get)?;
    handle_query(req, env).await
}

async fn handle_publish_status(env: Env, event_id: &str) -> Result<Response> {
    let kv = env.kv("CACHE")?;
    let cache = Cache::new(kv);

    match cache.get_publish_status(event_id).await? {
        Some(status) => json_response(&status, 200),
        None => {
            let err = ErrorResponse::new("not_found").with_detail("event not found");
            json_response(&err, 404)
        }
    }
}

async fn handle_publish(mut req: Request, env: Env) -> Result<Response> {
    // TODO: NIP-98 auth validation

    let body: crate::types::PublishRequest = req.json().await?;

    // Extract event ID
    let event_id = body
        .event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // TODO: Queue for publishing - Cloudflare Queues API not yet available in worker-rs
    // For now, return a placeholder response
    // let queue = env.queue("PUBLISH_QUEUE")?;
    // queue.send(body.event).await?;

    // Set initial status
    let kv = env.kv("CACHE")?;
    let cache = Cache::new(kv);
    let status = crate::types::PublishStatus {
        status: "queued".to_string(),
        attempts: Some(0),
        verified_at: None,
        error: None,
    };
    cache.set_publish_status(&event_id, &status).await?;

    let response = crate::types::PublishResponse {
        status: "queued".to_string(),
        event_id,
    };
    json_response(&response, 202)
}

fn json_response<T: serde::Serialize>(data: &T, status: u16) -> Result<Response> {
    let body = serde_json::to_string(data)?;
    let mut headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    Ok(Response::from_body(ResponseBody::Body(body.into_bytes()))?.with_status(status).with_headers(headers))
}

fn json_response_with_cache<T: serde::Serialize>(data: &T, status: u16, max_age: u64) -> Result<Response> {
    let body = serde_json::to_string(data)?;
    let mut headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    headers.set("Cache-Control", &format!("public, max-age={}, s-maxage={}", max_age, max_age))?;
    Ok(Response::from_body(ResponseBody::Body(body.into_bytes()))?.with_status(status).with_headers(headers))
}
