# Divine REST Gateway

REST API caching proxy for Nostr, running on Cloudflare Workers.

## Features

- **Read acceleration**: Cache Nostr queries with CDN + KV caching
- **Write proxy**: Reliable event publishing with verification and retries
- **NIP-98 auth**: Authenticated writes via HTTP Authorization
- **Edge deployment**: Global distribution via Cloudflare Workers

## API

### Query Events

```
GET /query?filter=<base64url-encoded-filter>
```

Filter is a base64url-encoded JSON Nostr filter:
```json
{"authors": ["pubkey"], "kinds": [1], "limit": 20}
```

### Convenience Endpoints

```
GET /profile/{pubkey}  - Get kind 0 profile
GET /event/{id}        - Get single event by ID
```

### Publish Event

```
POST /publish
Authorization: Nostr <base64-nip98-event>
Content-Type: application/json

{"event": {...signed nostr event...}}
```

### Check Publish Status

```
GET /publish/status/{event_id}
```

## Development

```bash
# Install wrangler
npm install -g wrangler

# Create KV namespace
wrangler kv:namespace create REST_GATEWAY_CACHE
wrangler kv:namespace create REST_GATEWAY_CACHE --preview

# Update wrangler.toml with namespace IDs

# Create queues
wrangler queues create divine-publish-events
wrangler queues create divine-publish-failed

# Run locally
wrangler dev

# Deploy
wrangler deploy
```

## Configuration

Set `RELAY_URL` in wrangler.toml vars or as a secret:
```bash
wrangler secret put RELAY_URL
```

## License

MIT
