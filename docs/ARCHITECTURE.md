# TokenRouter Architecture

## Overview

TokenRouter is a Rust-based inference gateway + billing engine for NVIDIA NIM. It proxies OpenAI-compatible chat completion requests, counts tokens with `tiktoken-rs`, applies tier-based pricing, and persists all transactions atomically to DynamoDB.

```
┌─────────────┐     ┌──────────────┐     ┌───────────────┐
│  Client SDK  │────▶│  TokenRouter  │────▶│  NVIDIA NIM   │
│ (OpenAI-compat)│   │  (gateway)    │     │  (upstream)    │
└─────────────┘     └──────┬────────┘     └───────────────┘
                           │
                    ┌──────▼────────┐
                    │   DynamoDB    │
                    │  (DDB tables) │
                    └───────────────┘
```

## Clean Architecture

The project follows Clean Architecture with four explicit layers. Inner layers have zero knowledge of outer layers.

```
src/
├── domain/          ← Entities + port traits (zero external deps)
├── application/     ← BillingService (pure business logic)
├── infrastructure/  ← DynamoDbStore (implements all ports)
└── interfaces/      ← HTTP handlers, DTOs, auth middleware, error types
```

### Dependency flow

```
interfaces → domain
interfaces → application
interfaces → infrastructure (via Repository trait)
infrastructure → domain
application → domain
domain → nothing (except chrono + serde)
```

`AppState` holds `Arc<dyn Repository>` — the composite port trait — enabling mock injection in tests without touching infrastructure.

## Data flow — chat completions request

```
1. POST /v1/chat/completions
2. Rate limit check (global semaphore + per-key counter)
3. Auth: extract Bearer token → SHA-256 hash → DynamoDB get_item
4. Idempotency check: if key exists, return cached response
5. Look up pricing for the model
6. Count input tokens (messages + tools)
7. Forward to upstream (NVIDIA NIM)
8. Count output tokens
9. Compute costs (provider cost input + output, tier sell price)
10. Atomic write via DynamoDB TransactWriteItems:
    - idempotency put (conditional)
    - transaction put
    - timeline put (sharded)
    - API key balance update
    - global metrics update
11. Return response with usage injected
```

## Module map

| File | Responsibility |
|------|---------------|
| `main.rs` | Entrypoint, env loading, router assembly, graceful shutdown |
| `lib.rs` | Re-exports all modules (test visibility) |
| **domain/** | |
| `entities.rs` | ApiKey, Tier, Transaction, PricingConfig, DashboardMetrics, IdempotencyRecord, RecentTransactionsPage, RuntimeMetricsSnapshot |
| `ports.rs` | ApiKeyPort, PricingPort, TransactionPort, MetricsPort, HealthPort, RuntimeMetricsPort + Repository super-trait |
| **application/** | |
| `billing.rs` | BillingService: token counting (tiktoken-rs), price selection, cost calculation, credit limit check |
| **infrastructure/** | |
| `dynamodb.rs` | DynamoDbStore: implements all ports via aws-sdk-dynamodb v1. Table validation on startup. Dead-letter table for failed transactions. |
| **interfaces/** | |
| `proxy.rs` | `POST /v1/chat/completions` — streaming + non-streaming, auth, billing, idempotency, cache |
| `completions.rs` | `POST /v1/completions` — legacy completions with billing |
| `embeddings.rs` | `POST /v1/embeddings` — passthrough, no billing |
| `provider.rs` | `GET /v1/models` — OpenRouter format + `?format=openai` |
| `admin.rs` | Admin handlers: dashboard, health, keys CRUD, pricing CRUD, transactions, metrics, cache stats, login |
| `admin_auth.rs` | Bearer middleware with rate limiting, CIDR allowlist, constant-time comparison, session store |
| `dto.rs` | Request/response types and serde DTOs |
| `error.rs` | GateError, ScopedInflightGuard, internal_gate_error, sanitise_upstream_error |
| `metrics.rs` | `GET /metrics` — Prometheus text |
| **support/** | |
| `security.rs` | SHA-256 API key hashing, HMAC-SHA256 payload hashing |
| `runtime_metrics.rs` | In-memory Prometheus counters with DynamoDB persistence |
| `request_cache.rs` | In-memory request cache with TTL + LRU eviction + memory tracking |
| `per_key_limiter.rs` | Per-API-key concurrency limiter with periodic cleanup |
| `state.rs` | AppState struct (shared across axum routes) |

## Runtime modes

| Mode | `RUN_MODE` | Transport | Used for |
|------|-----------|-----------|----------|
| Lambda | `lambda` | `lambda_http::run(app)` | AWS Lambda container image |
| Server | `server` | `axum::serve(TcpListener, app)` | Local dev + Docker |

The Lambda Dockerfile uses `public.ecr.aws/lambda/provided:al2023`. The local Dockerfile uses `gcr.io/distroless/cc-debian12`.

## DynamoDB tables

| Table | PK | SK | Purpose |
|-------|----|----|---------|
| `api_keys` | `key_hash` (S) | — | API key records |
| `pricing_config` | `model_name` (S) | — | Per-model pricing tiers |
| `transactions` | `id` (S) | — | Transaction ledger |
| `transactions_timeline` | `timeline_pk` (S) | `timeline_sk` (S) | Time-ordered recent tx lookup (sharded) |
| `idempotency_keys` | `idempotency_key` (S) | — | Idempotency registry (TTL on `expires_at`) |
| `metrics_global` | `metric_id` (S) | — | Aggregated counters + runtime metrics snapshot |
| `dead_letter` | `id` (S) | — | Failed TransactWriteItems (optional) |

## Request cache

- Content-addressable: hash of `model + request payload`
- Supports both **non-streaming** (returns cached JSON) and **streaming** (replays cached text as SSE chunk)
- Configurable TTL (`REQUEST_CACHE_TTL_SECONDS`, default 1h) and max entries (`REQUEST_CACHE_MAX_ENTRIES`, default 1000)
- Per-entry size cap (`REQUEST_CACHE_MAX_RESPONSE_BYTES`, default 64KB)
- Memory tracking via `GET /admin/cache`

## Testing

| Type | Command | Count |
|------|---------|-------|
| Unit | `cargo test --lib` | 50 |
| Integration | `cargo test --test integration_test -- --test-threads=1` | 9 |
| Clippy | `cargo clippy -- -D warnings` | — |
| Format | `cargo fmt --check` | — |

Integration tests require DynamoDB Local on `http://localhost:8000` and `AWS_REGION=us-east-1`.
