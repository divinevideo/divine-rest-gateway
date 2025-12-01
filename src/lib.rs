// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod cache;
mod filter;
mod relay_pool;
mod router;
mod types;

pub use relay_pool::RelayPool;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    router::handle_request(req, env).await
}
