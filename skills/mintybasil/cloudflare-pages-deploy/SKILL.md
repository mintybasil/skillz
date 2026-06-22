---
name: cloudflare-pages-deploy
author: zeroklaw
description: >
  Deploy a static site or SPA to Cloudflare Pages via wrangler CLI + GitHub
  Actions. Covers project creation, wrangler.toml config, KV bindings, Pages
  Functions, PR preview deployments, and auto-commenting preview URLs on PRs.
tags: [cloudflare, pages, github-actions, deployment, wrangler, ci-cd]
triggers:
  - user wants to deploy a site to Cloudflare Pages
  - user asks about Cloudflare Pages setup
  - user wants preview deployments on PRs
  - user wants to add a Pages Function or KV binding
---

# Cloudflare Pages Deployment

## Prerequisites

- Cloudflare account (Account ID: in dashboard URL or Overview page)
- API token with: **Cloudflare Pages — Edit** + **Workers KV Storage — Edit** (if using KV)
  - Create at: dash.cloudflare.com → My Profile → API Tokens → Create Custom Token
  - The "Edit Cloudflare Workers" template is a good base — add Pages Edit on top
- `wrangler` CLI available (`npx wrangler` works without global install)
- GitHub repo with Actions enabled

## GitHub Secrets Required

| Secret | Notes |
|---|---|
| `CLOUDFLARE_API_TOKEN` | API token (is a secret — never commit) |
| `CLOUDFLARE_ACCOUNT_ID` | Not truly secret but conventional to store here |

KV namespace IDs are **not secrets** — commit them directly to `wrangler.toml`.

## 1. Create the Pages Project (one-time)

The project must exist before the first deploy. Either:

**Via Cloudflare dashboard:**
1. Workers & Pages → Create → Pages → Direct Upload
2. Set the project name (must match `--project-name` in wrangler command)
3. Click Create — no upload needed, CI handles deploys

**Via wrangler CLI:**
```bash
wrangler pages project create <project-name> --production-branch main
```

## 2. `wrangler.toml`

```toml
name = "my-project"
compatibility_date = "2024-01-01"
pages_build_output_dir = "./dist"

# Optional: KV namespace binding for Pages Functions
[[kv_namespaces]]
binding = "MY_CACHE"
id = "abc123..."          # production namespace ID (not a secret)
preview_id = "def456..."  # preview namespace ID (not a secret)
```

### Creating KV namespaces (if needed)
```bash
wrangler kv namespace create MY_CACHE
wrangler kv namespace create MY_CACHE --preview
```
Copy the IDs from the output into `wrangler.toml`.

## 3. Pages Functions

Drop TypeScript files in `functions/` — they are automatically deployed as Workers alongside the frontend.

```
functions/
  _middleware.ts        # runs on all routes
  api/
    data.ts             # serves GET /api/data
```

### CORS middleware (`functions/_middleware.ts`)
```ts
export const onRequest: PagesFunction = async (context) => {
  const response = await context.next();
  const newResponse = new Response(response.body, response);
  newResponse.headers.set('Access-Control-Allow-Origin', '*');
  newResponse.headers.set('Access-Control-Allow-Methods', 'GET, OPTIONS');
  return newResponse;
};
```

### R2 bucket binding (`functions/bundles/[[catchall]].ts`)

Use `[[catchall]]` (double-bracket greedy) for dynamic file serving — it matches
paths containing dots (e.g. `.tar.zst`) and nested segments. Single-bracket
`[filename]` does NOT match paths with slashes, and may behave unexpectedly with
dots in some routing configurations.

```ts
interface Env {
  BUNDLES_BUCKET: R2Bucket;
}

export const onRequest: PagesFunction<Env> = async (context) => {
  const { params, env } = context;
  const suffix = Array.isArray(params.catchall)
    ? params.catchall.join("/")
    : params.catchall;

  // IMPORTANT: if objects are stored with a key prefix in R2, prepend it here.
  // e.g. if the upload workflow stores at bundles/<file>, and the URL is
  // /bundles/<file>, the R2 key is bundles/<file> — not just <file>.
  const r2Key = `bundles/${suffix}`;

  const object = await env.BUNDLES_BUCKET.get(r2Key);
  if (!object) return new Response("Not Found", { status: 404 });

  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("etag", object.httpEtag);
  headers.set("cache-control", "public, max-age=3600, s-maxage=3600");
  return new Response(object.body, { headers });
};
```

**wrangler.toml for R2:**
```toml
[[r2_buckets]]
binding = "BUNDLES_BUCKET"
bucket_name = "my-bucket-name"  # local dev only
```

The `bucket_name` in `wrangler.toml` is used for **local dev only** (`wrangler pages dev`).
For production, set the binding in the CF dashboard:
Pages → `<project>` → Settings → Functions → R2 bucket bindings.
The dashboard binding takes precedence and can reference a bucket whose name is
stored as a secret (unlike wrangler.toml which requires a literal name).

### Function with KV caching (`functions/api/data.ts`)
```ts
interface Env {
  MY_CACHE: KVNamespace;
}

const CACHE_KEY = 'data';
const CACHE_TTL = 7200; // 2 hours

export const onRequestGet: PagesFunction<Env> = async ({ env }) => {
  const cached = await env.MY_CACHE.get(CACHE_KEY);
  if (cached) {
    return new Response(cached, {
      headers: { 'Content-Type': 'application/json', 'X-Cache': 'HIT' },
    });
  }

  const upstream = await env.MY_CACHE.get('upstream_url') ?? '';
  const res = await fetch(upstream);
  const body = await res.text();
  await env.MY_CACHE.put(CACHE_KEY, body, { expirationTtl: CACHE_TTL });

  return new Response(body, {
    headers: { 'Content-Type': 'application/json', 'X-Cache': 'MISS' },
  });
};
```

> ⚠️ Cloudflare Workers have a **50 subrequest limit** per invocation. Avoid loops that fire one `fetch()` per item in a large array — batch or skip where possible.

## 4. GitHub Actions Workflow

```yaml
name: Deploy to Cloudflare Pages

on:
  push:
    branches:
      - main
  pull_request:

jobs:
  deploy:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      deployments: write
      pull-requests: write  # required for PR comment step

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'

      - name: Install dependencies
        run: npm ci

      - name: Build
        run: npm run build

      - name: Deploy to Cloudflare Pages
        id: deploy
        uses: cloudflare/wrangler-action@v3
        with:
          apiToken: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          accountId: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
          command: pages deploy dist --project-name=my-project --commit-dirty=true

      - name: Comment preview URL on PR
        if: github.event_name == 'pull_request'
        uses: actions/github-script@v7
        with:
          script: |
            const url = '${{ steps.deploy.outputs.deployment-url }}';
            const alias = '${{ steps.deploy.outputs.pages-deployment-alias-url }}';

            const marker = '<!-- cf-pages-preview -->';
            const body = `${marker}\n## Preview deployment\n\n| | URL |\n|---|---|\n| **Latest commit** | ${url} |\n| **Branch alias** | ${alias} |\n\nThe branch alias URL always points to the latest commit on this PR.`;

            const { data: comments } = await github.rest.issues.listComments({
              owner: context.repo.owner,
              repo: context.repo.repo,
              issue_number: context.issue.number,
            });

            const existing = comments.find(c => c.body.includes(marker));

            if (existing) {
              await github.rest.issues.updateComment({
                owner: context.repo.owner,
                repo: context.repo.repo,
                comment_id: existing.id,
                body,
              });
            } else {
              await github.rest.issues.createComment({
                owner: context.repo.owner,
                repo: context.repo.repo,
                issue_number: context.issue.number,
                body,
              });
            }
```

**Key points:**
- `id: deploy` on the wrangler step exposes `deployment-url` and `pages-deployment-alias-url` outputs
- The comment step uses an HTML marker `<!-- cf-pages-preview -->` to upsert — updates the existing comment on re-push rather than creating duplicates
- `pull-requests: write` permission is required for the comment step
- GitHub strips `target="_blank"` from Markdown — links cannot be forced to open in a new tab

## 5. Local Dev with Pages Functions

`npm run dev` / `vite dev` does **not** run Pages Functions. To test functions locally:

```bash
# Build first, then serve with wrangler (includes Functions + KV)
npm run build
npx wrangler pages dev dist --kv MY_CACHE
```

Consider adding a fallback in the frontend for when the API is unavailable (e.g. local dev without wrangler).

## References

- `references/rsc-scraping-in-pages-functions.md` — Pattern for scraping Next.js RSC (React Server Components) flight-data endpoints from inside a CF Pages Function, with KV caching and stale-while-error fallback. Useful when no public REST API exists.

## Pitfalls

- **Project must exist before first deploy** — `wrangler pages deploy` errors if the project doesn't exist yet; create it in the dashboard or via CLI first
- **KV IDs are not secrets** — commit them to `wrangler.toml` directly; only the API token needs to be a secret
- **50 subrequest limit** — each Workers invocation (Pages Function) can make at most 50 outbound `fetch()` calls
- **`--commit-dirty=true`** — required when the repo has uncommitted changes at deploy time (e.g. generated `dist/` is gitignored)
- **`pages_build_output_dir`** in `wrangler.toml` replaces the old `[site] bucket` syntax for Pages projects
- **`[[catchall]]` not `[filename]` for file serving** — use double-bracket greedy segments when matching filenames with dots (`.tar.zst`, `.tar.gz`) or paths with slashes; single-bracket `[param]` won't match those correctly
- **R2 key prefix mismatch** — if objects were uploaded with a path prefix (e.g. `bundles/<file>`) the function must reconstruct the full key, not just use the URL path segment; always check the upload workflow to confirm the exact key layout
- **R2 bucket_name in wrangler.toml is local-dev only** — for production, set the binding in the CF dashboard; this lets you use a bucket whose name is stored as a secret
- **KV cache key versioning** — when you change the response shape (rename fields, add/remove properties), bump the cache key (e.g. `market-stats` → `market-stats-v2`). Old-format cached data will cause runtime crashes in the frontend if it expects new field names. Defensive `??` fallbacks in the frontend mitigate this but cache key versioning is the root fix.
- **Exclude ongoing/latest records** — when proxying epoch-like data where the most recent record is still being written, sort descending and skip the first entry. Including a partial/ongoing record skews averages and charts.
- **SPA routing requires `_redirects`** — Single-page apps using client-side routing (React Router, Vue Router, etc.) will 404 on deep-links (`/blog/my-post`, `/about`) when loaded directly. Cloudflare Pages serves the file at that path if it exists, otherwise returns 404 — it does NOT fall back to `index.html` by default. Fix: add `public/_redirects` with `/* /index.html 200` to route all paths to the SPA entry point.
