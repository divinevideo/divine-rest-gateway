// ABOUTME: Main entry point for the Cloudflare Worker
// ABOUTME: Handles HTTP routing and Worker lifecycle

use worker::*;

mod cache;
mod filter;
mod types;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    Response::ok("Divine REST Gateway")
}
