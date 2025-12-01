# Divine REST Gateway - React Client Usage

REST API for Nostr that provides cached access to relay data via simple HTTP requests.

**Base URL**: `https://gateway.divine.video`

## Quick Start

```typescript
const GATEWAY_URL = 'https://gateway.divine.video';

// Fetch a user's profile
const profile = await fetch(`${GATEWAY_URL}/profile/${pubkey}`).then(r => r.json());

// Query events with a filter
const filter = { authors: [pubkey], kinds: [1], limit: 20 };
const encoded = btoa(JSON.stringify(filter))
  .replace(/\+/g, '-')
  .replace(/\//g, '_')
  .replace(/=/g, '');
const notes = await fetch(`${GATEWAY_URL}/query?filter=${encoded}`).then(r => r.json());
```

## API Endpoints

### GET `/profile/{pubkey}`

Fetch a user's profile (kind 0 metadata event).

```typescript
interface ProfileResponse {
  events: NostrEvent[];
  cached: boolean;
  cache_age_seconds?: number;
}

async function getProfile(pubkey: string): Promise<ProfileResponse> {
  const response = await fetch(`${GATEWAY_URL}/profile/${pubkey}`);
  if (!response.ok) throw new Error('Failed to fetch profile');
  return response.json();
}

// Usage
const { events } = await getProfile('82341f882b6eabcd2ba7f1ef90aad961cf074af15b9ef44a09f9d2a8fbfbe6a2');
const profile = events[0] ? JSON.parse(events[0].content) : null;
console.log(profile?.name, profile?.picture);
```

### GET `/event/{id}`

Fetch a single event by its ID.

```typescript
async function getEvent(eventId: string): Promise<NostrEvent | null> {
  const response = await fetch(`${GATEWAY_URL}/event/${eventId}`);
  if (!response.ok) return null;
  const { events } = await response.json();
  return events[0] || null;
}
```

### GET `/query?filter={base64url}`

Query events using a Nostr filter. The filter must be base64url-encoded JSON.

```typescript
// Helper to encode filters
function encodeFilter(filter: NostrFilter): string {
  return btoa(JSON.stringify(filter))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=/g, '');
}

// Helper to query events
async function queryEvents(filter: NostrFilter): Promise<NostrEvent[]> {
  const encoded = encodeFilter(filter);
  const response = await fetch(`${GATEWAY_URL}/query?filter=${encoded}`);
  if (!response.ok) throw new Error('Query failed');
  const { events } = await response.json();
  return events;
}

// Examples
const userNotes = await queryEvents({
  authors: ['pubkey...'],
  kinds: [1],
  limit: 50
});

const mentions = await queryEvents({
  '#p': ['pubkey...'],
  kinds: [1],
  limit: 20
});

const recentGlobal = await queryEvents({
  kinds: [1],
  limit: 100,
  since: Math.floor(Date.now() / 1000) - 3600 // last hour
});
```

## React Hooks

### useProfile Hook

```typescript
import { useState, useEffect } from 'react';

interface Profile {
  name?: string;
  about?: string;
  picture?: string;
  nip05?: string;
  lud16?: string;
  banner?: string;
}

export function useProfile(pubkey: string | undefined) {
  const [profile, setProfile] = useState<Profile | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    if (!pubkey) return;

    setLoading(true);
    setError(null);

    fetch(`https://gateway.divine.video/profile/${pubkey}`)
      .then(r => r.json())
      .then(({ events }) => {
        if (events[0]) {
          setProfile(JSON.parse(events[0].content));
        }
      })
      .catch(setError)
      .finally(() => setLoading(false));
  }, [pubkey]);

  return { profile, loading, error };
}

// Usage in component
function ProfileCard({ pubkey }: { pubkey: string }) {
  const { profile, loading } = useProfile(pubkey);

  if (loading) return <div>Loading...</div>;

  return (
    <div>
      <img src={profile?.picture} alt={profile?.name} />
      <h2>{profile?.name}</h2>
      <p>{profile?.about}</p>
    </div>
  );
}
```

### useNostrQuery Hook

```typescript
import { useState, useEffect, useMemo } from 'react';

interface NostrFilter {
  ids?: string[];
  authors?: string[];
  kinds?: number[];
  '#e'?: string[];
  '#p'?: string[];
  since?: number;
  until?: number;
  limit?: number;
}

interface NostrEvent {
  id: string;
  pubkey: string;
  created_at: number;
  kind: number;
  tags: string[][];
  content: string;
  sig: string;
}

function encodeFilter(filter: NostrFilter): string {
  return btoa(JSON.stringify(filter))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=/g, '');
}

export function useNostrQuery(filter: NostrFilter | null) {
  const [events, setEvents] = useState<NostrEvent[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const [cached, setCached] = useState(false);

  const filterKey = useMemo(
    () => (filter ? JSON.stringify(filter) : null),
    [filter]
  );

  useEffect(() => {
    if (!filter || !filterKey) return;

    setLoading(true);
    setError(null);

    const encoded = encodeFilter(filter);
    fetch(`https://gateway.divine.video/query?filter=${encoded}`)
      .then(r => r.json())
      .then(data => {
        setEvents(data.events);
        setCached(data.cached);
      })
      .catch(setError)
      .finally(() => setLoading(false));
  }, [filterKey]);

  return { events, loading, error, cached };
}

// Usage
function UserFeed({ pubkey }: { pubkey: string }) {
  const { events, loading } = useNostrQuery({
    authors: [pubkey],
    kinds: [1],
    limit: 50
  });

  if (loading) return <div>Loading feed...</div>;

  return (
    <div>
      {events.map(event => (
        <div key={event.id}>
          <p>{event.content}</p>
          <small>{new Date(event.created_at * 1000).toLocaleString()}</small>
        </div>
      ))}
    </div>
  );
}
```

### useFeed Hook (with pagination)

```typescript
import { useState, useCallback } from 'react';

export function useFeed(authors: string[], kinds: number[] = [1]) {
  const [events, setEvents] = useState<NostrEvent[]>([]);
  const [loading, setLoading] = useState(false);
  const [hasMore, setHasMore] = useState(true);

  const loadMore = useCallback(async () => {
    if (loading) return;

    setLoading(true);

    const oldest = events[events.length - 1];
    const filter: NostrFilter = {
      authors,
      kinds,
      limit: 20,
      ...(oldest && { until: oldest.created_at - 1 })
    };

    try {
      const encoded = encodeFilter(filter);
      const response = await fetch(
        `https://gateway.divine.video/query?filter=${encoded}`
      );
      const { events: newEvents } = await response.json();

      if (newEvents.length < 20) setHasMore(false);
      setEvents(prev => [...prev, ...newEvents]);
    } finally {
      setLoading(false);
    }
  }, [authors, kinds, events, loading]);

  const refresh = useCallback(async () => {
    setEvents([]);
    setHasMore(true);
    // loadMore will be called by effect
  }, []);

  return { events, loading, hasMore, loadMore, refresh };
}
```

## Response Format

All endpoints return JSON with this structure:

```typescript
interface QueryResponse {
  events: NostrEvent[];  // Array of Nostr events
  eose: boolean;         // End of stored events reached
  complete: boolean;     // Query fully satisfied
  cached: boolean;       // Response served from cache
  cache_age_seconds?: number; // How old the cached data is
}
```

## Filter Reference

Standard NIP-01 filter fields:

| Field | Type | Description |
|-------|------|-------------|
| `ids` | `string[]` | Event IDs to fetch |
| `authors` | `string[]` | Pubkeys of event authors |
| `kinds` | `number[]` | Event kinds (0=profile, 1=note, etc) |
| `#e` | `string[]` | Events being referenced |
| `#p` | `string[]` | Pubkeys being tagged |
| `since` | `number` | Unix timestamp, events after |
| `until` | `number` | Unix timestamp, events before |
| `limit` | `number` | Max events to return |

## Cache TTLs

Responses are cached based on content type:

| Content | TTL |
|---------|-----|
| Profiles (kind 0) | 15 minutes |
| Contacts (kind 3) | 10 minutes |
| Notes (kind 1) | 5 minutes |
| Reactions (kind 7) | 2 minutes |
| Other queries | 5 minutes |

The `cached` and `cache_age_seconds` fields tell you if data came from cache.

## Error Handling

```typescript
interface ErrorResponse {
  error: string;
  detail?: string;
}

async function safeQuery(filter: NostrFilter): Promise<NostrEvent[]> {
  try {
    const encoded = encodeFilter(filter);
    const response = await fetch(
      `https://gateway.divine.video/query?filter=${encoded}`
    );

    if (!response.ok) {
      const error = await response.json();
      throw new Error(error.detail || error.error);
    }

    const { events } = await response.json();
    return events;
  } catch (err) {
    console.error('Query failed:', err);
    return [];
  }
}
```

## TypeScript Types

```typescript
// Core Nostr types
interface NostrEvent {
  id: string;
  pubkey: string;
  created_at: number;
  kind: number;
  tags: string[][];
  content: string;
  sig: string;
}

interface NostrFilter {
  ids?: string[];
  authors?: string[];
  kinds?: number[];
  '#e'?: string[];
  '#p'?: string[];
  '#t'?: string[];
  since?: number;
  until?: number;
  limit?: number;
}

// Gateway response types
interface QueryResponse {
  events: NostrEvent[];
  eose: boolean;
  complete: boolean;
  cached: boolean;
  cache_age_seconds?: number;
}

interface ErrorResponse {
  error: string;
  detail?: string;
}

// Common profile structure
interface NostrProfile {
  name?: string;
  display_name?: string;
  about?: string;
  picture?: string;
  banner?: string;
  nip05?: string;
  lud16?: string;
  website?: string;
}
```

## Complete Example: Profile Page

```tsx
import { useState, useEffect } from 'react';

const GATEWAY = 'https://gateway.divine.video';

function encodeFilter(filter: any): string {
  return btoa(JSON.stringify(filter))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=/g, '');
}

export function ProfilePage({ pubkey }: { pubkey: string }) {
  const [profile, setProfile] = useState<any>(null);
  const [notes, setNotes] = useState<any[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    async function load() {
      setLoading(true);

      // Fetch profile and notes in parallel
      const [profileRes, notesRes] = await Promise.all([
        fetch(`${GATEWAY}/profile/${pubkey}`).then(r => r.json()),
        fetch(`${GATEWAY}/query?filter=${encodeFilter({
          authors: [pubkey],
          kinds: [1],
          limit: 20
        })}`).then(r => r.json())
      ]);

      if (profileRes.events[0]) {
        setProfile(JSON.parse(profileRes.events[0].content));
      }
      setNotes(notesRes.events);
      setLoading(false);
    }

    load();
  }, [pubkey]);

  if (loading) return <div>Loading...</div>;

  return (
    <div>
      {profile && (
        <header>
          {profile.banner && <img src={profile.banner} alt="Banner" />}
          <img src={profile.picture} alt={profile.name} />
          <h1>{profile.display_name || profile.name}</h1>
          <p>{profile.about}</p>
          {profile.nip05 && <span>{profile.nip05}</span>}
        </header>
      )}

      <section>
        <h2>Notes</h2>
        {notes.map(note => (
          <article key={note.id}>
            <p>{note.content}</p>
            <time>{new Date(note.created_at * 1000).toLocaleString()}</time>
          </article>
        ))}
      </section>
    </div>
  );
}
```
