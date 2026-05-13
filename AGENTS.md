# AGENTS.md

## Build & test commands

```bash
cargo check                                      # Fast compile-check (what CI runs)
cargo build                                      # Full build (CI doesn't run this)
cargo build --release                            # Docker-style build
cargo test --lib                                 # Unit tests only (no DynamoDB needed)
cargo test --test integration_test               # Integration tests (requires DynamoDB local)
cargo clippy -- -D warnings                      # Lint (CI gate)
cargo fmt --check                                # Format check (CI gate)
```

Formatting uses default `rustfmt`. Clippy is strict (`-D warnings`), matching CI at `.github/workflows/ci.yml:52`.

## Integration tests

Require DynamoDB local on `http://localhost:8000`. CI spawns it as a service container (`ci.yml:31-35`). Integration tests must run sequentially (`--test-threads=1`). Locally:

```bash
docker compose up -d dynamodb-local
AWS_REGION=us-east-1 AWS_ENDPOINT_URL_DYNAMODB=http://localhost:8000 AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test cargo test --test integration_test -- --test-threads=1
```

All integration tests are in `tests/integration_test.rs` — the crate has NO `#[cfg(test)]` integration modules inside `src/`.

**Dependencies:** Uses `aws-sdk-dynamodb` v1 (NOT rusoto). The migration was completed 2026-05-06. Requires Rust ≥1.91 (Dockerfile uses `rust:1.95-bookworm`).

## Architecture

Single-crate, hex/ports-and-adapters layout:

```
src/main.rs             — entrypoint, env loading, router assembly
src/lib.rs              — re-exports all modules (for test visibility)
src/domain/
  entities.rs           — ApiKey, Tier, Transaction, PricingConfig, etc.
  ports.rs              — Traits: Repository (super-trait), ApiKeyPort, TransactionPort, MetricsPort, etc.
src/application/
  billing.rs            — Pure billing logic + tiktoken-rs token counting
src/infrastructure/
  dynamodb.rs           — DynamoDbStore: all port impls + DynamoDB client (aws-sdk-dynamodb)
src/interfaces/
  proxy.rs              — POST /v1/chat/completions (stream + non-stream)
  embeddings.rs         — POST /v1/embeddings (passthrough, no billing)
  provider.rs           — GET /v1/models, GET /v1/openai/models
  admin.rs              — Admin JSON API handlers (dashboard, keys CRUD, pricing CRUD, transactions, health)
  admin_auth.rs         — Bearer-token middleware with rate limiting + CIDR allowlist
  metrics.rs            — GET /metrics (Prometheus text)
  dto.rs                — Request/response types and serde DTOs
  completions.rs        — POST /v1/completions (legacy endpoint, auth + billing + idempotency)
  error.rs              — GateError type, ScopedInflightGuard, error helpers
  images.rs             — POST /v1/images/generations (passthrough, no billing)
  audio.rs              — POST /v1/audio/transcriptions, POST /v1/audio/translations
src/security.rs         — SHA-256 hashing for API keys and payloads
src/runtime_metrics.rs  — In-memory Prometheus metrics with flush to DynamoDB
src/state.rs            — AppState struct shared across axum routes
src/per_key_limiter.rs  — Per-API-key concurrency limiter with RAII guard
src/request_cache.rs    — In-memory content-addressable request cache with TTL/LRU
src/provider_registry.rs — Cached provider registry with lazy loading from DynamoDB
frontend/dist/          — Vite/React SPA admin dashboard (served as static files)
```

`src/domain/ports.rs:90-92` defines `Repository` as a super-trait of all individual ports. Any struct implementing all ports automatically satisfies `Repository`. The `AppState` holds `Arc<dyn Repository>`.

## Runtime modes

Controlled by `RUN_MODE` env var (default: `lambda`):

- `RUN_MODE=lambda` — runs via `lambda_http::run(app)` for AWS Lambda (container image)
- `RUN_MODE=server` — starts a plain `axum::serve` TCP listener on `LISTEN_ADDR`

For local dev, always use `RUN_MODE=server`.

## Multi-provider

The system supports multiple upstream providers via a provider registry pattern:

- The default provider is configured via `NIM_BASE_URL`/`NIM_API_KEY` env vars (in-memory only, never stored in DynamoDB)
- Additional providers are stored in the `providers` DynamoDB table and cached in `ProviderRegistry`
- Each pricing entry maps a model to a provider via `provider_id`
- Admin API: `GET/POST /admin/providers`, `POST /admin/providers/delete`, `POST /admin/providers/refresh`
- `ProviderRegistry` caches providers in memory with `RwLock`, lazy loaded from DynamoDB

### Provider resolution flow

```
Request model name (e.g. "gpt-4o")
  → repo.pricing_for_model(model) → Option<PricingConfig>
    ├─ Some(pricing) → pricing.provider_id (e.g. "nim-gpu")
    └─ None → "default" (falls back to default provider)
  → registry.get(provider_id)
    ├─ Found in cache → Provider
    ├─ provider_id == "default" → default_provider.clone()
    └─ Not found → ERROR (400/502, no silent fallback)
```

All gateway endpoints (`/v1/chat/completions`, `/v1/embeddings`, `/v1/images/generations`, `/v1/audio/*`, `/v1/completions`) resolve providers via this flow. Only chat completions and completions do billing; others are passthrough.

### /v1/models aggregates all providers

The model list endpoint iterates all cached providers (default + DynamoDB), fetches upstream models from each, and merges results. If a provider is unreachable, a warning is logged and models from other providers still appear.

### Provider cache coherency

- Refreshed at startup (`main.rs:143-144`)
- Refreshed after every `upsert_provider` / `delete_provider` admin call
- Manual refresh endpoint: `POST /admin/providers/refresh`
- No TTL-based refresh — strictly event-driven

### Provider CRUD

- `upsert_provider`: Creates or updates a provider. `created_at` is preserved on updates. If `api_key` is empty and the provider already exists, the existing key is reused.
- `delete_provider`: Deletes by `id` PK, then refreshes cache.
- `list_providers`: Returns all providers from DynamoDB, sorted by `id`.

### Health check per provider

The `GET /admin/health` endpoint checks connectivity for each cached provider individually (`upstream_{provider_id}`) plus an aggregate `upstream_connectivity` flag.

### Pricing validation

When creating/updating pricing via `POST /admin/pricing`, the `provider_id` is validated against the registry. Unknown provider IDs return 400.

## DynamoDB (NOT PostgreSQL)

The `migrations/` directory contains PostgreSQL `.sql` files — these are **not used**. The project uses DynamoDB exclusively via `aws-sdk-dynamodb` v1. The `DynamoDbStore::init_schema()` in `src/infrastructure/dynamodb.rs:67` is a no-op. All schema creation happens via `scripts/bootstrap_dynamodb_local.sh` or Terraform (`terraform/dynamodb.tf`).

Eight DynamoDB tables (table names configurable via env vars with defaults):
1. `api_keys` — PK: `key_hash`
2. `pricing_config` — PK: `model_name`
3. `transactions` — PK: `id`
4. `transactions_timeline` — PK: `timeline_pk`, SK: `timeline_sk` (sharded, see below)
5. `idempotency_keys` — PK: `idempotency_key` (with TTL on `expires_at`)
6. `metrics_global` — PK: `metric_id` (stores both `global` aggregates and `runtime_metrics` snapshot)
7. `providers` — PK: `id` (stores upstream provider configurations)
8. `dead_letter` — PK: `id` (optional, stores failed TransactWriteItems)

Local DynamoDB: `docker compose up -d dynamodb-local` (port 8000), then `./scripts/bootstrap_dynamodb_local.sh`.

## Frontend (admin dashboard)

The admin dashboard is a Vite/React SPA served as static files from `frontend/dist/`. The Rust binary serves these files via the `spa_fallback` handler in `src/main.rs:199`. Run `npm run build` in `frontend/` to rebuild the static assets. The old `templates/admin.html` is a legacy artifact and is not used.

## Key env vars & quirks

- `ADMIN_BEARER_TOKENS` (CSV) takes priority over `ADMIN_BEARER_TOKEN` (single token) — see `src/interfaces/admin_auth.rs:101-115`. Use the CSV form for rotation.
- `ADMIN_IP_ALLOWLIST` (comma-separated CIDRs) — when non-empty, blocks all admin requests from IPs not in the list.
- `DDB_DEAD_LETTER_TABLE` — optional; when set, failed `TransactWriteItems` writes are persisted to a dead-letter table.
- `DDB_PROVIDERS_TABLE` (default `"providers"`) — table for provider configurations
- `TIMELINE_SHARD_COUNT` (default 1) — partitions `transactions_timeline` into N shards. Shard 1 = partition key `"global"`, shards 2+ = `"shard-2"`, etc. Recent-transaction queries fan out across all shards. Increase this when a single timeline partition becomes a hot spot.
- `METRICS_FLUSH_INTERVAL_SECONDS` (default 300) — how often runtime metrics snapshots are persisted to DynamoDB (merged back on restart).
- `MAX_OUTPUT_TOKENS` (default 16384) — caps `max_tokens` and `max_completion_tokens` in forwarded requests.
- `MAX_INFLIGHT_PER_KEY` (default 10) — per-API-key concurrency limit. Requests exceeding this per-key cap get 429.
- Upstream retries: 3 attempts with 100ms/300ms delay, only for 5xx responses (`src/interfaces/proxy.rs:390-403`).
- Request body limit: 8 MB (`src/main.rs:140`).
- `NIM_BASE_URL` and `NIM_API_KEY` now define the "default" provider; additional providers are managed via admin API. If `NIM_BASE_URL` ends in `/v1`, the `/v1` suffix is stripped before appending `/v1/chat/completions` or `/v1/embeddings` (see `upstream_completions_url` in `proxy.rs:405-411`).
- `PROVIDER_NAME_PREFIX` was replaced by per-provider `name` in the `providers` DynamoDB table, but still used for the default provider.

## Local dev setup

```bash
cp .env.example .env    # edit NIM_API_KEY and ADMIN_BEARER_TOKEN
docker compose up --build -d
./scripts/bootstrap_dynamodb_local.sh
./scripts/backfill_idempotency_ttl.sh  # only if pre-existing records lack TTL
```

Or use the all-in-one script: `./scripts/run_local_stack.sh`.

App: `http://localhost:8080`, Admin: `http://localhost:8080/admin`, DynamoDB: `http://localhost:8000`

## Deployment

Uses the Lambda container image flow (`Dockerfile`), not the zip/glibc target. CI deploys on push to `main` (when `src/`, `Cargo.*`, `Dockerfile`, or `terraform/` change). ECR repo + Lambda function name are hardcoded in `deploy.yml`. Terraform files are in `terraform/` but deployment is a separate manual/CI step.

## Lint/fmt ordering

CI runs these independently in parallel (`ci.yml`). There's no prescribed local order, but `cargo clippy -- -D warnings` and `cargo fmt --check` should both pass before pushing.
