// ABOUTME: Body-bound authenticated creation proxy with explicit origins, quotas and deadlines.
use crate::{
    auth,
    short_link_contract::{ip_quota_key, origin_allowed, parse_creation_body, validate_mapping},
    short_link_limiter::QuotaKeys,
};
use futures_util::{
    future::{select, Either},
    pin_mut, StreamExt,
};
use sha2::{Digest, Sha256};
use std::time::Duration;
use wasm_bindgen::JsValue;
use worker::*;

const ENDPOINT: &str = "https://gateway.divine.video/api/links";
const UPSTREAM: &str = "https://divine.video/api/links";

pub async fn handle(mut req: Request, env: Env) -> Result<Response> {
    let origin = req.headers().get("Origin")?;
    let result = handle_inner(&mut req, &env, origin.as_deref()).await;
    // Runtime/binding failures must not expose internal details or credentials.
    let mut response = result.unwrap_or(Response::error("temporarily_unavailable", 503)?);
    let headers = response.headers_mut();
    headers.set("Cache-Control", "no-store")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("Vary", "Origin")?;
    if let Some(origin) = origin
        .as_deref()
        .filter(|origin| origin_allowed(Some(origin)))
    {
        headers.set("Access-Control-Allow-Origin", origin)?;
        headers.set("Access-Control-Allow-Methods", "POST, OPTIONS")?;
        headers.set(
            "Access-Control-Allow-Headers",
            "Content-Type, Authorization",
        )?;
    }
    Ok(response)
}

async fn handle_inner(req: &mut Request, env: &Env, origin: Option<&str>) -> Result<Response> {
    if req.url()?.as_str() != ENDPOINT {
        return Response::error("not_found", 404);
    }
    if !origin_allowed(origin) {
        return Response::error("origin_not_allowed", 403);
    }
    if req.method() == Method::Options {
        return Ok(Response::empty()?.with_status(204));
    }
    if req.method() != Method::Post {
        return Response::error("method_not_allowed", 405);
    }
    if env
        .var("SHORT_LINK_CREATION_ENABLED")
        .map(|value| value.to_string())
        .unwrap_or_default()
        != "true"
    {
        return Response::error("temporarily_unavailable", 503);
    }
    let content_type = req.headers().get("Content-Type")?.unwrap_or_default();
    if content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        != "application/json"
    {
        return Response::error("unsupported_media_type", 415);
    }
    let mut body = Vec::new();
    let mut stream = req.stream()?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > 4096 {
            return Response::error("body_too_large", 413);
        }
        body.extend(chunk);
    }
    let authorization = req.headers().get("Authorization")?;
    let identity = match auth::validate_creation(
        authorization.as_deref(),
        ENDPOINT,
        &body,
        (Date::now().as_millis() / 1000) as u64,
    ) {
        Ok(identity) => identity,
        Err(_) => return Response::error("unauthorized", 401),
    };
    let target = match parse_creation_body(&body) {
        Ok(target) => target,
        Err(_) => return Response::error("invalid_target", 400),
    };
    let Some(ip) = req
        .headers()
        .get("CF-Connecting-IP")?
        .filter(|ip| !ip.is_empty())
    else {
        return Response::error("temporarily_unavailable", 503);
    };
    let keys = QuotaKeys {
        identity: hex::encode(Sha256::digest(format!(
            "short-link:identity:{}",
            identity.pubkey
        ))),
        ip: match ip_quota_key(&ip) {
            Some(key) => key,
            None => return Response::error("temporarily_unavailable", 503),
        },
    };
    let namespace = env.durable_object("SHORT_LINK_LIMITER")?;
    let stub = namespace.id_from_name("creator-v1")?.get_stub()?;
    let quota_request = Request::new_with_init(
        "https://limiter/consume",
        RequestInit::new()
            .with_method(Method::Post)
            .with_body(Some(JsValue::from_str(&serde_json::to_string(&keys)?))),
    )?;
    let quota = stub.fetch_with_request(quota_request).await?;
    if quota.status_code() == 429 {
        let mut response = Response::error("rate_limited", 429)?;
        response.headers_mut().set("Retry-After", "60")?;
        return Ok(response);
    }
    if quota.status_code() != 200 {
        return Response::error("temporarily_unavailable", 503);
    }
    let token = env.secret("SHORT_LINK_CREATION_TOKEN")?.to_string();
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    headers.set("Authorization", &format!("Bearer {token}"))?;
    let upstream = Request::new_with_init(
        UPSTREAM,
        RequestInit::new()
            .with_method(Method::Post)
            .with_headers(headers)
            .with_redirect(RequestRedirect::Manual)
            .with_body(Some(JsValue::from_str(&serde_json::to_string(
                &serde_json::json!({"url":target}),
            )?))),
    )?;
    let controller = AbortController::default();
    let signal = controller.signal();
    let operation = async {
        let mut response = Fetch::Request(upstream).send_with_signal(&signal).await?;
        if ![200, 201].contains(&response.status_code()) {
            return Response::error("temporarily_unavailable", 503);
        }
        let mut bytes = Vec::new();
        let mut stream = response.stream()?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if bytes.len() + chunk.len() > 4096 {
                return Response::error("temporarily_unavailable", 503);
            }
            bytes.extend(chunk);
        }
        match validate_mapping(&bytes, &target) {
            Ok(mapping) => Ok(Response::from_json(&mapping)?.with_status(response.status_code())),
            Err(_) => Response::error("temporarily_unavailable", 503),
        }
    };
    let deadline = Delay::from(Duration::from_millis(1500));
    pin_mut!(operation, deadline);
    match select(operation, deadline).await {
        Either::Left((response, _)) => response,
        Either::Right(_) => {
            controller.abort();
            Response::error("temporarily_unavailable", 503)
        }
    }
}
