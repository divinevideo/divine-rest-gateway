import assert from "node:assert/strict";
import { test } from "node:test";
import { createHash } from "node:crypto";
import { schnorr } from "@noble/curves/secp256k1";
import { Miniflare, Response } from "miniflare";

const endpoint = "https://gateway.divine.video/api/links";
const identity = "01".repeat(32);
const ip = "02".repeat(32);
const target = "https://divine.video/video/synthetic-runtime-check";
const body = JSON.stringify({ url: target });

function headers() {
  const secret = new Uint8Array(32).fill(42);
  const pubkey = Buffer.from(schnorr.getPublicKey(secret)).toString("hex");
  const created_at = Math.floor(Date.now() / 1000);
  const digest = (value) => createHash("sha256").update(value).digest();
  const tags = [["u", endpoint], ["method", "POST"], ["payload", digest(body).toString("hex")]];
  const id = digest(JSON.stringify([0, pubkey, created_at, 27235, tags, ""]));
  const sig = Buffer.from(schnorr.sign(id, secret, new Uint8Array(32))).toString("hex");
  const event = { id: id.toString("hex"), pubkey, created_at, kind: 27235, tags, content: "", sig };
  return { "Content-Type": "application/json", "CF-Connecting-IP": "192.0.2.43",
    Origin: "https://divine.video", Authorization: `Nostr ${Buffer.from(JSON.stringify(event)).toString("base64")}` };
}

const enabled = { SHORT_LINK_CREATION_ENABLED: "true", SHORT_LINK_CREATION_TOKEN: "synthetic-upstream-token" };

async function runtime(t, bindings = {}, extra = {}) {
  const mf = new Miniflare({
    modules: true,
    scriptPath: "tests/short_links_fixture.mjs",
    modulesRules: [
      { type: "ESModule", include: ["**/*.js"], fallthrough: true },
      { type: "CompiledWasm", include: ["**/*.wasm"] },
    ],
    compatibilityDate: "2024-01-01",
    compatibilityFlags: ["nodejs_compat"],
    cf: false,
    bindings,
    durableObjects: { SHORT_LINK_LIMITER: { className: "TestLimiter", useSQLite: true } },
    outboundService() { throw new Error("unexpected outbound request"); },
    ...extra,
  });
  t.after(() => mf.dispose());
  await mf.ready;
  if (extra.durableObjects) return { mf };
  const namespace = await mf.getDurableObjectNamespace("SHORT_LINK_LIMITER");
  const limiter = namespace.get(namespace.idFromName("creator-v1"));
  const sql = async (statement, args) => {
    const response = await limiter.fetch("https://fixture/sql", {
      method: "POST", body: JSON.stringify({ sql: statement, args }),
    });
    assert.equal(response.status, 200);
    return response.json();
  };
  const consume = (keys = { identity, ip }) => limiter.fetch("https://limiter/consume", {
    method: "POST", body: JSON.stringify(keys),
  });
  return { mf, sql, consume };
}

test("pause returns before body, authentication, limiter binding or upstream access", async (t) => {
  // No token, no limiter binding: moving the gate below either access fails.
  const { mf } = await runtime(t, { SHORT_LINK_CREATION_ENABLED: "false" }, { durableObjects: {} });
  const response = await mf.dispatchFetch(endpoint, { method: "POST" });
  assert.equal(response.status, 503);
  assert.equal(await response.text(), "temporarily_unavailable");
  assert.equal(response.headers.get("cache-control"), "no-store");
  const preflight = await mf.dispatchFetch(endpoint, {
    method: "OPTIONS", headers: { Origin: "https://divine.video" },
  });
  assert.equal(preflight.status, 204);
  assert.match(preflight.headers.get("access-control-allow-headers"), /Authorization/);
});

test("CORS caches preflight and exposes quota retry timing", async (t) => {
  const { mf } = await runtime(t);
  const response = await mf.dispatchFetch(endpoint, {
    method: "OPTIONS", headers: { Origin: "https://divine.video" },
  });
  assert.equal(response.status, 204);
  assert.equal(response.headers.get("access-control-max-age"), "86400");
  assert.equal(response.headers.get("access-control-expose-headers"), "Retry-After");
  const denied = await mf.dispatchFetch(endpoint, {
    method: "OPTIONS", headers: { Origin: "https://evil.example" },
  });
  assert.equal(denied.status, 403);
  assert.equal(denied.headers.get("access-control-allow-origin"), null);
});

test("concurrent minute requests admit exactly 30 and reject without charging", async (t) => {
  const { consume, sql } = await runtime(t);
  const seconds = Math.floor(Date.now() / 1000) % 60;
  if (seconds > 45) await new Promise((resolve) => setTimeout(resolve, (61 - seconds) * 1000));
  const minute = Math.floor(Date.now() / 60000);
  const responses = await Promise.all(Array.from({ length: 35 }, () => consume()));
  assert.equal(responses.filter((response) => response.status === 200).length, 30);
  assert.equal(responses.filter((response) => response.status === 429).length, 5);
  for (const response of responses.filter((response) => response.status === 429)) {
    assert.equal(response.headers.get("retry-after"), "60");
  }
  assert.equal(Math.floor(Date.now() / 60000), minute, "retry test away from minute boundary");
  const rows = await sql("SELECT count FROM quotas");
  assert.equal(rows.length, 5);
  assert.ok(rows.every((row) => row.count === 30));
});

for (const [bucket, limit] of [[`daily:identity:${identity}`, 100], [`daily:ip:${ip}`, 300], ["daily:global", 1000]]) {
  test(`daily budget ${bucket.split(":")[1]} survives minute rollover and charges atomically`, async (t) => {
    const { consume, sql } = await runtime(t);
    // Initialize the real table, then isolate the last admit of each daily budget.
    assert.equal((await consume()).status, 200);
    await sql("DELETE FROM quotas");
    const minute = Math.floor(Date.now() / 60000);
    const dayEnd = (Math.floor(minute / 1440) + 1) * 1440 - 1;
    await sql("INSERT INTO quotas(bucket,window,count) VALUES (?,?,?)", [bucket, dayEnd, limit - 1]);
    const responses = await Promise.all(Array.from({ length: 4 }, (_, index) => consume({
      identity: bucket.startsWith("daily:identity:") ? identity : (index + 10).toString(16).padStart(64, "0"),
      ip: bucket.startsWith("daily:ip:") ? ip : (index + 20).toString(16).padStart(64, "0"),
    })));
    assert.deepEqual(responses.map((r) => r.status).sort(), [200, 429, 429, 429]);
    const retry = Number(responses.find((r) => r.status === 429).headers.get("retry-after"));
    assert.ok(retry > 0 && retry <= 86400);
    // Simulate expired minute buckets without advancing or mocking the runtime clock.
    await sql("UPDATE quotas SET window=? WHERE bucket NOT LIKE 'daily:%'", [minute - 1]);
    assert.equal((await consume()).status, 429);
    const rows = await sql("SELECT bucket,count FROM quotas ORDER BY bucket");
    assert.equal(rows.find((row) => row.bucket === bucket).count, limit);
    assert.ok(rows.filter((row) => row.bucket !== bucket).every((row) => row.count === 1));
  });
}

test("legacy minute usage is preserved and expired counters use indexed cleanup", async (t) => {
  const { consume, sql } = await runtime(t);
  await sql("CREATE TABLE quotas(bucket TEXT NOT NULL, window INTEGER NOT NULL, count INTEGER NOT NULL, PRIMARY KEY(bucket,window))");
  const minute = Math.floor(Date.now() / 60000);
  await sql("INSERT INTO quotas VALUES (?,?,30),(?,?,8)", [identity, minute, "expired", minute - 1]);
  assert.equal((await consume()).status, 429);
  assert.deepEqual(await sql("SELECT count FROM quotas WHERE bucket=?", [identity]), [{ count: 30 }]);
  assert.deepEqual(await sql("SELECT count FROM quotas WHERE bucket='expired'"), []);
  const plan = await sql("EXPLAIN QUERY PLAN DELETE FROM quotas WHERE window < ?", [minute]);
  assert.ok(plan.some((row) => row.detail.includes("INDEX quotas_window")), JSON.stringify(plan));
});

test("network minute budget applies across identities", async (t) => {
  const { consume, sql } = await runtime(t);
  await consume();
  await sql("UPDATE quotas SET count=119 WHERE bucket=?", [ip]);
  const responses = await Promise.all([consume({ identity: "03".repeat(32), ip }), consume({ identity: "04".repeat(32), ip })]);
  assert.deepEqual(responses.map((r) => r.status).sort(), [200, 429]);
  assert.deepEqual(await sql("SELECT count FROM quotas WHERE bucket=?", [ip]), [{ count: 120 }]);
});

test("expired daily budgets reset without retaining old identities", async (t) => {
  const { consume, sql } = await runtime(t);
  await consume();
  await sql("UPDATE quotas SET window=?,count=1000", [Math.floor(Date.now() / 60000) - 1]);
  assert.equal((await consume()).status, 200);
  const rows = await sql("SELECT count FROM quotas");
  assert.equal(rows.length, 5);
  assert.ok(rows.every((row) => row.count === 1));
});

test("signed creation uses the upstream credential, tolerates metadata and forwards daily retry timing", async (t) => {
  let upstreamCalls = 0;
  const { mf, sql } = await runtime(t, enabled, {
    async outboundService(request) {
      upstreamCalls++;
      assert.equal(request.url, "https://divine.video/api/links");
      assert.equal(request.method, "POST");
      assert.equal(request.headers.get("Authorization"), "Bearer synthetic-upstream-token");
      assert.deepEqual(await request.json(), { url: target });
      return Response.json({ code: "Abc12345", short_url: "https://divine.video/l/Abc12345", url: target, extra: true }, { status: 201 });
    },
  });
  const response = await mf.dispatchFetch(endpoint, { method: "POST", headers: headers(), body });
  assert.equal(response.status, 201);
  assert.deepEqual(await response.json(), { code: "Abc12345", short_url: "https://divine.video/l/Abc12345", url: target });
  await sql("UPDATE quotas SET count=1000 WHERE bucket='daily:global'");
  const blocked = await mf.dispatchFetch(endpoint, { method: "POST", headers: headers(), body });
  assert.equal(blocked.status, 429);
  assert.ok(Number(blocked.headers.get("retry-after")) > 60);
  assert.equal(blocked.headers.get("access-control-expose-headers"), "Retry-After");
  assert.equal(upstreamCalls, 1);
});

test("missing upstream secret leaves all quota counters untouched", async (t) => {
  const { mf, sql } = await runtime(t, { SHORT_LINK_CREATION_ENABLED: "true" });
  const response = await mf.dispatchFetch(endpoint, { method: "POST", headers: headers(), body });
  assert.equal(response.status, 503);
  assert.equal(await response.text(), "temporarily_unavailable");
  assert.deepEqual(await sql("SELECT name FROM sqlite_master WHERE type='table' AND name='quotas'"), []);
});

test("one deadline bounds a stalled request body", async (t) => {
  const { mf } = await runtime(t, enabled);
  const stream = new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("{"));
      const timer = setTimeout(() => controller.close(), 2500);
      t.after(() => clearTimeout(timer));
    },
  });
  const response = await mf.dispatchFetch(endpoint, { method: "POST", headers: headers(), body: stream, duplex: "half" });
  assert.equal(response.status, 503);
  assert.equal(await response.text(), "temporarily_unavailable");
});

test("one deadline bounds a slow limiter before upstream creation", async (t) => {
  let upstreamCalls = 0;
  const { mf } = await runtime(t, { ...enabled, LIMITER_DELAY_MS: "2500" }, {
    outboundService() {
      upstreamCalls++;
      return Response.json({ code: "Abc12345", short_url: "https://divine.video/l/Abc12345", url: target }, { status: 201 });
    },
  });
  const response = await mf.dispatchFetch(endpoint, { method: "POST", headers: headers(), body });
  assert.equal(response.status, 503);
  assert.equal(upstreamCalls, 0);
});
