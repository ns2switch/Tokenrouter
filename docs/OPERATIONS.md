# TokenRouter Operations Guide

## Quick start (local)

```bash
cp .env.example .env                   # edit NIM_API_KEY
docker compose up --build -d
./scripts/bootstrap_dynamodb_local.sh
```

- App: http://localhost:8080
- Admin: http://localhost:8080/admin (login with `ADMIN_BEARER_TOKEN`)
- DynamoDB Local: http://localhost:8000

Or use the all-in-one script: `./scripts/run_local_stack.sh`

---

## Environment Variables

### Required

| Variable | Description |
|----------|-------------|
| `NIM_BASE_URL` | NVIDIA NIM base URL (e.g. `https://integrate.api.nvidia.com`) |
| `NIM_API_KEY` | NVIDIA NIM API key |
| `API_KEY_HASH_SECRET` | Secret for SHA-256 hashing of customer API keys |

### Optional — Auth

| Variable | Default | Description |
|----------|---------|-------------|
| `ADMIN_BEARER_TOKEN` | — | Single admin token (fallback) |
| `ADMIN_BEARER_TOKENS` | — | CSV of admin tokens (takes priority, supports rotation) |
| `ADMIN_IP_ALLOWLIST` | — | CSV CIDR list for admin access (empty = any IP) |
| `ADMIN_RATE_LIMIT_MAX_FAILURES` | 5 | Max login failures before rate limit |
| `ADMIN_RATE_LIMIT_WINDOW_SECONDS` | 60 | Rate limit window for admin logins |

### Optional — Rate Limiting

| Variable | Default | Description |
|----------|---------|-------------|
| `MAX_INFLIGHT_REQUESTS` | 200 | Global concurrent request limit |
| `MAX_INFLIGHT_PER_KEY` | 10 | Concurrent requests per API key |

### Optional — Upstream

| Variable | Default | Description |
|----------|---------|-------------|
| `UPSTREAM_TIMEOUT_SECONDS` | 300 | HTTP client timeout for upstream calls |
| `MAX_OUTPUT_TOKENS` | 16384 | Cap on `max_tokens` / `max_completion_tokens` |
| `MAX_STREAMING_DURATION_SECONDS` | 600 | Maximum duration for streaming requests |

### Optional — Caching

| Variable | Default | Description |
|----------|---------|-------------|
| `REQUEST_CACHE_TTL_SECONDS` | 3600 | TTL for cached responses (1 hour) |
| `REQUEST_CACHE_MAX_ENTRIES` | 1000 | Maximum cached entries (LRU eviction) |
| `REQUEST_CACHE_MAX_RESPONSE_BYTES` | 65536 | Max bytes per cached response |

### Optional — DynamoDB Tables

| Variable | Default |
|----------|---------|
| `DDB_API_KEYS_TABLE` | `api_keys` |
| `DDB_PRICING_TABLE` | `pricing_config` |
| `DDB_TRANSACTIONS_TABLE` | `transactions` |
| `DDB_TRANSACTIONS_TIMELINE_TABLE` | `transactions_timeline` |
| `DDB_IDEMPOTENCY_TABLE` | `idempotency_keys` |
| `DDB_METRICS_TABLE` | `metrics_global` |
| `DDB_DEAD_LETTER_TABLE` | — (optional) |

### Optional — AWS

| Variable | Default | Description |
|----------|---------|-------------|
| `AWS_REGION` | `us-east-1` | AWS region |
| `AWS_ACCESS_KEY_ID` | — | Credentials (optional for IAM roles) |
| `AWS_SECRET_ACCESS_KEY` | — | Credentials (optional for IAM roles) |
| `AWS_ENDPOINT_URL_DYNAMODB` | — | Override for DynamoDB Local |

### Optional — Runtime

| Variable | Default | Description |
|----------|---------|-------------|
| `RUN_MODE` | `lambda` | `lambda` or `server` |
| `LISTEN_ADDR` | `0.0.0.0:8080` | Bind address (server mode only) |
| `IDEMPOTENCY_TTL_SECONDS` | 86400 | Idempotency key expiry (24h) |
| `TIMELINE_SHARD_COUNT` | 1 | Timeline partition shards |
| `METRICS_FLUSH_INTERVAL_SECONDS` | 300 | Runtime metrics snapshot interval |
| `RUST_LOG` | — | Tracing log level (e.g. `info`, `debug`) |

### Optional — Provider Metadata

| Variable | Default | Description |
|----------|---------|-------------|
| `PROVIDER_NAME_PREFIX` | `TokenRouter` | Prefix for model IDs |
| `PROVIDER_QUANTIZATION` | `fp16` | Quantization metadata |
| `PROVIDER_CONTEXT_LENGTH` | 128000 | Context window length |
| `PROVIDER_MAX_OUTPUT_LENGTH` | 8192 | Max output tokens |
| `PROVIDER_DATACENTER_COUNTRY` | `US` | Datacenter country code |

---

## Deployment

### Local Docker

```bash
docker compose up --build -d
./scripts/bootstrap_dynamodb_local.sh
```

### AWS Lambda (production)

```bash
# Build and push to ECR
docker build -t tokenrouter -f Dockerfile .
aws ecr get-login-password | docker login --username AWS --password-stdin $ECR_URI
docker tag tokenrouter:latest $ECR_URI:latest
docker push $ECR_URI:latest

# Deploy infrastructure
cd terraform
terraform init
terraform apply -var="nim_base_url=https://integrate.api.nvidia.com" \
                -var="nim_api_key=..." \
                -var="api_key_hash_secret=..." \
                -var="admin_bearer_tokens=token1,token2"
```

Terraform creates:
- ECR repository
- Lambda function (container image, 300s timeout, 1024MB RAM)
- API Gateway HTTP API (public endpoint)
- DynamoDB tables (PAY_PER_REQUEST)
- IAM role (least privilege: GetItem, PutItem, UpdateItem, Scan, Query, TransactWriteItems, DescribeTable, DescribeTimeToLive)
- CloudWatch alarms (errors, throttles, p95 duration, DynamoDB throttles)
- SNS topic for alarm notifications
- CloudWatch log group (30 day retention)

### Bootstrap production data

After first deploy, seed the DynamoDB tables:

```bash
# Create an admin API key
aws dynamodb put-item --table-name api_keys --item '{
  "key_hash": {"S": "<sha256 of admin key>"},
  "id": {"S": "admin-key-uuid"},
  "tier": {"S": "bit"},
  "active": {"BOOL": true},
  "balance_accumulated": {"N": "0"},
  "created_at": {"S": "2026-01-01T00:00:00Z"}
}'

# Add pricing for your models
aws dynamodb put-item --table-name pricing_config --item '{
  "model_name": {"S": "google/gemma-2-2b-it"},
  "provider_cost_input_per_1m": {"N": "0"},
  "provider_cost_output_per_1m": {"N": "0"},
  "bit_price_per_1m": {"N": "0.10"},
  "node_price_per_1m": {"N": "0.08"},
  "cluster_price_per_1m": {"N": "0.05"},
  "updated_at": {"S": "2026-01-01T00:00:00Z"}
}'
```

---

## Monitoring

### Health checks

| Endpoint | Auth | Use |
|----------|------|-----|
| `GET /health` | None | Load balancer / API Gateway health check |
| `GET /admin/health` | Admin | Detailed health (TTL, DynamoDB write probe, upstream connectivity) |

### Prometheus

`GET /metrics` returns Prometheus text format with these metrics:

| Metric | Type | Description |
|--------|------|-------------|
| `tokenrouter_inflight_requests` | gauge | Current in-flight requests |
| `tokenrouter_requests_total` | counter | Total completed requests |
| `tokenrouter_requests_by_status_total` | counter | Requests grouped by status code |
| `tokenrouter_requests_by_model_total` | counter | Requests grouped by model |
| `tokenrouter_ttft_ms_bucket` | counter | TTFT histogram buckets (ms) |
| `tokenrouter_duration_ms_bucket` | counter | Total request duration buckets (ms) |
| `tokenrouter_throughput_tps_x100_sum` | counter | Throughput in tokens/s × 100 |

### CloudWatch

Terraform creates these alarms:

| Alarm | Threshold |
|-------|-----------|
| Lambda errors | > 5 per 5 min |
| Lambda throttles | > 1 per 5 min |
| Lambda p95 duration | > 30 seconds |
| DynamoDB throttled requests | > 0 per 1 min |

### Tracing

All logs include `request_id` and `key_id` via structured tracing spans. Set `RUST_LOG=info` for production, `RUST_LOG=debug` for debugging.

---

## Rate Limiting

Two independent limiters:

| Limiter | Config | How |
|--------|--------|-----|
| **Global** | `MAX_INFLIGHT_REQUESTS=200` | `tokio::sync::Semaphore` — acquires per-request |
| **Per-key** | `MAX_INFLIGHT_PER_KEY=10` | `HashMap<key_hash, count>` — increments/decrements with RAII guard |

Both return HTTP 429 with `code: "rate_limit_exceeded"` when exceeded.

**Admin rate limiting:** `ADMIN_RATE_LIMIT_MAX_FAILURES=5` in a `ADMIN_RATE_LIMIT_WINDOW_SECONDS=60` window, per IP.

---

## Request Caching

Content-addressable cache based on `model + request payload hash`.

| Scenario | Behavior |
|----------|----------|
| Cache hit (non-streaming) | Returns cached JSON response directly |
| Cache hit (streaming) | Replays cached text as single SSE chunk |
| Cache miss | Forwards to upstream, caches response on success |

Monitor with `GET /admin/cache`. Configure with `REQUEST_CACHE_TTL_SECONDS`, `REQUEST_CACHE_MAX_ENTRIES`, `REQUEST_CACHE_MAX_RESPONSE_BYTES`.

---

## Idempotency

Send `Idempotency-Key: <key>` header. The gateway:

1. Checks if the key exists **before** calling upstream
2. If same payload: returns cached response (non-streaming) or replays text (streaming)
3. If different payload: returns 409 `idempotency_key_reused`
4. If not found: calls upstream, stores result atomically

Keys expire after `IDEMPOTENCY_TTL_SECONDS` (default 24h).

---

## Operational Scripts

| Script | Purpose |
|--------|---------|
| `run_local_stack.sh` | Start DynamoDB + bootstrap + backfill + app |
| `bootstrap_dynamodb_local.sh` | Create tables + seed demo data |
| `backfill_idempotency_ttl.sh` | Add TTL to existing idempotency records |
| `e2e_verify.sh` | End-to-end tests (auth, health, idempotency, load) |
| `load_smoke.sh` | Light load test (200/429/5xx distribution) |
| `provider_compliance_check.sh` | Validate `/v1/models` OpenRouter format |

---

## Scaling

### Timeline sharding

`TIMELINE_SHARD_COUNT` partitions the `transactions_timeline` table across N shards. Shard 1 uses partition key `"global"` (backward compatible). Shards 2+ use `"shard-N"`. Recent transaction queries fan out across all shards and merge/sort results.

Increase this value when a single timeline partition becomes a hot spot under high write throughput.

### Lambda concurrency

The Lambda auto-scales with request volume up to the AWS account's concurrency limit. Consider setting **provisioned concurrency** for latency-sensitive models.

### Upstream timeout

Large models (31B+) may need 60-120 seconds for cold-start responses. Configure `UPSTREAM_TIMEOUT_SECONDS` accordingly and ensure the Lambda timeout in Terraform matches it.
