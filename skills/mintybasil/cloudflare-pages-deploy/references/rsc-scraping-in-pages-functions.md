# Scraping Next.js RSC Endpoints from Cloudflare Pages Functions

## Context

Some web apps (e.g. Boundless Explorer) embed data in Next.js RSC (React Server Components) flight-data responses instead of exposing public REST APIs. You can extract this data from a CF Pages Function that proxies the RSC endpoint, parses the payload, and returns clean JSON.

## Pattern

### 1. Pages Function (`functions/api/market-stats.ts`)

```ts
const UPSTREAM = 'https://explorer.boundless.network/base/stats';
const CACHE_KEY = 'market-stats-v2';  // BUMP when response shape changes!
const CACHE_TTL = 7200; // 2 hours

interface Env { EPOCHS_CACHE: KVNamespace; }

export const onRequestGet: PagesFunction<Env> = async ({ env }) => {
  // 1. Try KV cache
  const cached = await env.EPOCHS_CACHE.get(CACHE_KEY);
  if (cached) {
    return new Response(cached, {
      headers: { 'Content-Type': 'application/json', 'X-Cache': 'HIT' },
    });
  }

  // 2. Fetch fresh from RSC endpoint
  try {
    const upstream = await fetch(UPSTREAM, {
      headers: { 'RSC': '1', 'Accept': 'text/x-component' },
    });
    if (!upstream.ok) throw new Error(`Upstream ${upstream.status}`);
    const rscText = await upstream.text();
    const raw = extractStatsArray(rscText);
    const normalised = normaliseBuckets(raw);
    const body = JSON.stringify(normalised);

    // 3. Cache in KV
    await env.EPOCHS_CACHE.put(CACHE_KEY, body, { expirationTtl: CACHE_TTL });

    return new Response(body, {
      headers: { 'Content-Type': 'application/json', 'X-Cache': 'MISS' },
    });
  } catch (err) {
    // 4. Stale-while-error fallback from KV
    const stale = await env.EPOCHS_CACHE.getByCacheKey?.(CACHE_KEY) ?? await env.EPOCHS_CACHE.get(CACHE_KEY);
    if (stale) {
      return new Response(stale, {
        headers: { 'Content-Type': 'application/json', 'X-Cache': 'STALE' },
      });
    }
    return new Response(JSON.stringify({ error: '...' }), { status: 502 });
  }
};
```

### 2. RSC Payload Extraction

The RSC flight-data payload is a mix of React instructions and embedded JSON. Key steps:

```ts
function extractStatsArray(rscText: string): StatsBucket[] {
  // Find the first JSON array containing chain_id entries
  const startMarker = '[{"chain_id":';
  const startIdx = rscText.indexOf(startMarker);
  if (startIdx === -1) throw new Error('No stats JSON array found');

  // Bracket-match to find the array end
  let depth = 0, endIdx = -1;
  for (let i = startIdx; i < rscText.length; i++) {
    if (rscText[i] === '[') depth++;
    else if (rscText[i] === ']') depth--;
    if (depth === 0) { endIdx = i + 1; break; }
  }

  const jsonStr = rscText.slice(startIdx, endIdx);
  // RSC date references like "$D2025-12-01T00:00:00.000Z" need $D stripped
  const cleaned = jsonStr.replace(/"\$D([^"]+)"/g, '"$1"');
  return JSON.parse(cleaned);
}
```

### 3. Normalisation

Convert raw RSC buckets (bigint strings, RSC-prefixed dates) into clean frontend-friendly JSON:

```ts
function normaliseBuckets(buckets: StatsBucket[]): MarketStatsBucket[] {
  return buckets
    .filter(b => Number(b.total_cycles) > 0)  // skip empty buckets
    .map(b => {
      const totalCycles = Number(BigInt(b.total_cycles));
      // total_program_cycles = market/program cycles (ZK proof orders)
      const marketCycles = Number(BigInt(b.total_program_cycles));
      // PoVW = total - market (baseline mining work)
      const povwCycles = Math.max(0, totalCycles - marketCycles);
      // ... format date, compute percentages, etc.
    })
    .sort((a, b) => a.date.localeCompare(b.date));
}
```

### 4. Excluding Ongoing Epochs

When proxying time-series data where the most recent record is still accumulating (e.g. the current epoch), exclude it from results:

```ts
const sorted = [...entries].sort((a, b) => b.epoch - a.epoch);
const completed = sorted.slice(1); // skip the ongoing epoch
return completed.map(normalise);
```

Including an in-progress record skews averages, charts, and makes the "latest" stat misleading.

## Data Semantics: Boundless total_cycles vs total_program_cycles

In the Boundless network's `/base/stats` RSC endpoint:
- `total_cycles` = ALL computation cycles (PoVW mining + market program orders)
- `total_program_cycles` = cycles from ZK proof execution orders on the open market (NOT PoVW)
- PoVW cycles = `total_cycles - total_program_cycles` (the baseline mining work)

Market cycles typically dominate (~98%) while PoVW mining is the small remainder (~1-2%). Visualizing these on a stacked area chart makes the small PoVW sliver invisible. Use separate non-stacked areas or a side-by-side bar chart instead.

## Pitfalls

- **RSC format is fragile** — it's an implementation detail of Next.js, not a stable API. The payload structure can change with any deploy. Always have a fallback (bundled data, cached KV).
- **BigInt strings** — RSC serializes large numbers as strings (e.g. `"532000000000000"`). Use `Number(BigInt(str))` to parse safely.
- **`$D` date prefix** — RSC embeds dates with a `$D` prefix like `"$D2025-12-01T00:00:00.000Z"`. Strip this before JSON.parse.
- **Bracket matching** — don't use regex alone for extracting nested JSON from RSC text. Use a depth-counter bracket matcher as shown above.
- **KV stale-while-error** — always serve stale KV data if upstream fails, before returning 502. This gives resilience against transient upstream outages.
- **Cache headers** — set `X-Cache: HIT|MISS|STALE` so the frontend can show data freshness.
- **KV cache key versioning** — when you change the response shape (rename fields, add/remove properties), bump the cache key (e.g. `market-stats` → `market-stats-v2`). Old-format cached data will cause runtime crashes in the frontend if it expects new field names. Defensive `??` fallbacks in the frontend mitigate this but cache key versioning is the root fix.
- **Exclude ongoing/latest records** — when proxying epoch-like data where the most recent record is still being written, sort descending and skip the first entry. Including a partial/ongoing record skews averages and charts.
- **Frontend null-safety** — always use `?? 0` or `?? fallback` when consuming API data in chart components. Cached or stale responses may have different field names than the current code expects. A single `undefined.toFixed()` call crashes the entire page.