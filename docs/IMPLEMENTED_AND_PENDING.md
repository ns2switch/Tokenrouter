# TokenRouter: Implemented vs Pending

## Implemented

### Core Gateway
- Rust gateway using Axum + Tokio
- `POST /v1/chat/completions` (streaming + non-streaming SSE)
- `POST /v1/completions` (legacy completions with billing)
- `POST /v1/embeddings` (passthrough, no billing)
- `POST /v1/audio/transcriptions` (passthrough, auth + rate limiting)
- `POST /v1/audio/translations` (passthrough, auth + rate limiting)
- `POST /v1/images/generations` (passthrough, auth + rate limiting)
- Full JSON passthrough to upstream (tools, response_format, max_tokens, etc.)
- Streaming SSE support with keep-alive comments + usage chunks
- Upstream retries: 3 attempts with 100ms/300ms delay (5xx only)
- Max tokens cap (`MAX_OUTPUT_TOKENS`, default 16384)
- Request body limit 8MB
- Global inflight concurrency limit (`MAX_INFLIGHT_REQUESTS`, default 200)
- Per-API-key concurrency limit (`MAX_INFLIGHT_PER_KEY`, default 10)
- Configurable upstream timeout (`UPSTREAM_TIMEOUT_SECONDS`, default 300s)
- Streaming max duration timeout (`MAX_STREAMING_DURATION_SECONDS`, default 600s)
- Graceful shutdown (SIGTERM handler)

### Billing Engine
- Token counting with `tiktoken-rs`
- Separate input/output provider costs (per model)
- Tier-based sell prices (bit/node/cluster)
- Real-time cost/revenue with `provider_cost(input, output, pricing)`
- Atomic write path via DynamoDB `TransactWriteItems`
- Tool definitions counted in input token estimation
- Tool call + function_call extraction from responses
- Image token estimation (low/high detail per OpenAI pricing: 85/1105 tokens)
- Credit limit enforcement per API key
- Per-tier model restrictions (`min_tier` in pricing config)

### Multi-Provider Support
- Multiple upstream providers via DynamoDB `providers` table
- Default provider from `NIM_BASE_URL`/`NIM_API_KEY` env vars (backward compatible)
- Per-model provider routing via `provider_id` in pricing config
- `ProviderRegistry` with in-memory cache + `RwLock`
- Admin CRUD for providers: `GET/POST /admin/providers`, `POST /admin/providers/delete`, `POST /admin/providers/refresh`
- Centralized URL builder fixing `/v1` suffix stripping inconsistency
- Provider metadata (quantization, context_length, max_output_length, datacenter_country) per provider

### Idempotency
- `Idempotency-Key` header support
- Payload hash validation (HMAC-SHA256)
- Early idempotency check BEFORE upstream call (avoids duplicate API calls)
- Configurable TTL (`IDEMPOTENCY_TTL_SECONDS`)
- Non-streaming replay: cached response returned
- Streaming replay: accumulated text replayed as SSE chunk

### Request Cache
- Content-addressable cache (hash of `model + request payload`)
- Non-streaming cache hit: returns cached JSON (36x faster)
- Streaming cache hit: replays cached text as SSE chunk (80x faster)
- Configurable TTL (`REQUEST_CACHE_TTL_SECONDS`, default 1h)
- LRU eviction (`REQUEST_CACHE_MAX_ENTRIES`, default 1000)
- Per-entry size cap (`REQUEST_CACHE_MAX_RESPONSE_BYTES`, default 64KB)
- Memory tracking via `GET /admin/cache`

### Admin Panel
- React SPA served from Rust binary (Vite + Tailwind CSS)
- Cookie-based auth (HttpOnly + Secure + SameSite=Lax)
- Session ID mapping (token never stored in cookie)
- Bearer token + cookie auth with constant-time comparison
- IP allowlist (`ADMIN_IP_ALLOWLIST`) + rate limiting (`ADMIN_RATE_LIMIT_*`)
- Health check: TTL, DynamoDB write probe, upstream connectivity
- Dead letter table monitoring: `GET /admin/dead-letter`
- Upstream model listing: `GET /admin/models/upstream`
- 18 admin JSON endpoints

### API Key Management
- List all keys (id, tier, balance, credit_limit, active, created_at)
- Create new keys (returns raw key once via HTTP 201)
- Enable/disable keys
- Update credit limit per key
- Delete keys
- Inactive keys treated as "not found" (no info leak)

### Pricing Management
- List all pricing configs
- Create/update pricing per model (input/output provider costs + 3 tier prices)
- Delete pricing per model

### Observability
- Structured request logs with `request_id` + `key_id` in tracing span
- Prometheus `/metrics` endpoint (inflight, requests, status/model counters, TTFT/duration histograms, throughput)
- Standard Prometheus histogram format: cumulative buckets, `_sum`, `_count`, `+Inf`
- Runtime metrics persistence to DynamoDB (merged on restart)
- CloudWatch alarms (Lambda errors, throttles, p95 duration, DynamoDB throttles)
- `x-request-id` and `x-ratelimit-remaining-requests` response headers

### Security
- API key SHA-256 hashing with secret pepper
- Payload HMAC-SHA256 for idempotency
- Admin auth: constant-time token comparison, rate limiting, CIDR allowlist
- CSP, HSTS, X-Frame-Options, X-Content-Type-Options, CORP, Referrer-Policy
- CORS (permissive origins, no credentials)
- Error sanitization: upstream + internal errors stripped before client response
- Real errors logged via tracing for debugging
- OpenAI-compatible error codes (`invalid_api_key`, `rate_limit_exceeded`, etc.)
- Path traversal protection in SPA static file serving

### Infrastructure
- AWS SDK: `aws-sdk-dynamodb` v1 (NOT deprecated rusoto)
- Rust 1.95+ (Dockerfile uses `rust:1.95-bookworm`)
- Docker (Lambda container + local distroless)
- docker-compose for local dev
- Terraform: DynamoDB tables, Lambda (300s timeout, 1GB RAM), API Gateway HTTP API, IAM (least privilege), CloudWatch alarms, SNS
- CI: cargo check, cargo test (unit + integration), clippy, rustfmt

### Gzip Compression
- All responses compressed with gzip (tower-http CompressionLayer)

### Testing
- 76 unit tests (domain, application, infrastructure, security, runtime_metrics, proxy)
- 12 integration tests (DynamoDB local, sequential execution)

---

## Pending / Not Yet Implemented

### API Compatibility
- Additional OpenAI endpoints (responses, etc.)

### Security
- OIDC/Cognito/IAM admin auth instead of static bearer tokens
- Secret rotation automation

### Testing
- Streaming edge case tests with real upstream
- Provider compatibility regression tests
- Load/perf test profiles
