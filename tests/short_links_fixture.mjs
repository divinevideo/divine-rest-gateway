// Test-only SQL access; the request path still executes the exported Rust limiter.
import gateway, { ShortLinkLimiter } from "../build/worker/shim.mjs";
export { RelayPool } from "../build/worker/shim.mjs";

export class TestLimiter extends ShortLinkLimiter {
  constructor(state, env) {
    super(state, env);
    this.testState = state;
    this.testDelay = Number(env.LIMITER_DELAY_MS ?? 0);
  }

  async fetch(request) {
    if (new URL(request.url).hostname === "fixture") {
      const { sql, args = [] } = await request.json();
      return Response.json([...this.testState.storage.sql.exec(sql, ...args)]);
    }
    if (this.testDelay) await new Promise((resolve) => setTimeout(resolve, this.testDelay));
    return super.fetch(request);
  }
}

export default gateway;
