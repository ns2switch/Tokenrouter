# TokenRouter API Reference

## Authentication

### Public API (customer keys)
Authenticate with the API key created via the admin panel:
```
Authorization: Bearer sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```
The raw key is returned once at creation time. The gateway stores only its SHA-256 hash.

### Admin API (operator tokens)
Authenticate with the admin bearer token:
```
Authorization: Bearer <admin-token>
```
Or use the browser login flow: `POST /admin/login` with `{"token":"..."}` → sets an HttpOnly session cookie.

---

## Public Endpoints

### `POST /v1/chat/completions`

OpenAI-compatible chat completions.

| Feature | Supported |
|---------|-----------|
| Non-streaming | ✅ |
| Streaming SSE (`stream: true`) | ✅ |
| `stream_options.include_usage` | ✅ |
| Tools / function calling | ✅ (passed through + token counted) |
| `response_format` (json_object, json_schema) | ✅ (passed through) |
| `max_tokens` / `max_completion_tokens` | ✅ (capped at `MAX_OUTPUT_TOKENS`) |
| `Idempotency-Key` header | ✅ (pre-upstream check) |
| Multimodal (images) | ✅ (text extracted for token counting) |

**Request:**
```json
{
  "model": "google/gemma-2-2b-it",
  "messages": [{"role": "user", "content": "Hello"}],
  "stream": false,
  "max_tokens": 100,
  "temperature": 0.7
}
```

**Response (non-streaming):**
```json
{
  "id": "chatcmpl-xxx",
  "object": "chat.completion",
  "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hi!"}, "finish_reason": "stop"}],
  "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
}
```

**Response (streaming):**
```
data: {"choices":[{"delta":{"content":"Hi"}}],"model":"google/gemma-2-2b-it"}
data: {"choices":[{"delta":{"content":"!"}}],"model":"google/gemma-2-2b-it"}
data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}
data: [DONE]
```

**Headers returned:**
- `x-request-id` — UUID for request tracing
- `x-ratelimit-remaining-requests` — available slots in the global semaphore

**Errors:**
```json
{"error": {"message": "api key not found", "type": "gateway_error", "code": "invalid_api_key"}}
```

| Code | HTTP | Meaning |
|------|------|---------|
| `invalid_api_key` | 401 | API key not found or inactive |
| `model_not_found` | 400 | No pricing configured for this model |
| `rate_limit_exceeded` | 429 | Global or per-key concurrency limit hit |
| `insufficient_quota` | 402 | Credit limit exhausted |
| `invalid_value` | 400 | Missing `model` or `messages` |
| `idempotency_key_reused` | 409 | Same key, different payload |

---

### `POST /v1/completions`

Legacy completions endpoint. Same auth and billing as chat completions.

```json
{"model": "google/gemma-2-2b-it", "prompt": "Hello"}
```

---

### `POST /v1/embeddings`

Passthrough to upstream. No token counting or billing. Requires API key authentication.

---

### `GET /v1/models`

Returns available models with pricing. Supports two formats:

**Default (OpenRouter provider format):**
```json
{"data": [{"id": "TokenRouter/google/gemma-2-2b-it", "pricing": {"prompt": "0.0000001000", ...}}]}
```

**OpenAI format** (`?format=openai`):
```json
{"object": "list", "data": [{"id": "TokenRouter/google/gemma-2-2b-it", "owned_by": "TokenRouter"}]}
```

---

### `GET /health`

Public health check for load balancers (no auth required):
```json
{"status": "ok"}
```

---

## Admin Endpoints

All require authentication via Bearer token or session cookie.

### Authentication

#### `POST /admin/login`
```json
{"token": "admin-test-token"}
```
→ 302 redirect to `/admin` with `Set-Cookie: admin_token={session_id}; HttpOnly; Secure; SameSite=Lax`

#### `GET /admin/logout`
→ Clears session cookie, redirects to `/`

---

### Health & Monitoring

#### `GET /admin/health`
```json
{
  "ok": true,
  "ttl_enabled": true,
  "dynamodb_write_ok": true,
  "upstream_ok": true,
  "inflight_requests": 0,
  "checks": [
    {"name": "idempotency_ttl", "ok": true, "detail": "ttl_enabled=true"},
    {"name": "dynamodb_write_probe", "ok": true, "detail": "ok"},
    {"name": "upstream_connectivity", "ok": true, "detail": "reachable"}
  ]
}
```

#### `GET /admin/metrics`
```json
{"inflight_requests": 0, "snapshot": {"requests_total": 42, "status_counts": [[200, 40], [429, 2]], ...}}
```

#### `GET /admin/cache`
```json
{"entries": 2, "hits": 5, "misses": 10, "memory_bytes": 2048, "max_entries": 1000}
```

---

### Dashboard

#### `GET /admin/dashboard`
```json
{
  "metrics": {"total_margin": 0.5, "total_input_tokens": 1000, "total_output_tokens": 500, "tx_count": 10},
  "pricing_count": 3,
  "inflight_requests": 0
}
```

---

### API Keys

#### `GET /admin/keys` — List all keys
```json
[{"id": "11111111-...", "tier": "bit", "balance_accumulated": 0.05, "credit_limit": null, "active": true, "created_at": "2026-01-01T00:00:00+00:00"}]
```

#### `POST /admin/keys` — Create a key
```json
{"tier": "bit", "credit_limit": 100.0}
```
→ 201 Created
```json
{"id": "uuid", "raw_key": "sk-xxxx...", "tier": "bit"}
```
The `raw_key` is returned **only once**. Store it securely.

#### `GET /admin/keys/toggle?id=<id>&active=<bool>` — Enable/disable
```json
{"id": "uuid", "active": false}
```

#### `GET /admin/keys/limit?id=<id>&credit_limit=<n>` — Update credit limit
```json
{"id": "uuid", "credit_limit": 500.0}
```

#### `GET /admin/keys/limit?id=<id>` — Remove credit limit
```json
{"id": "uuid", "credit_limit": null}
```

#### `GET /admin/keys/delete?id=<id>` — Delete key
```json
{"deleted": "uuid"}
```

---

### Pricing

#### `GET /admin/pricing` — List all pricing
```json
[{"model_name": "google/gemma-2-2b-it", "provider_cost_input_per_1m": 0.0, "provider_cost_output_per_1m": 0.0, "bit_price_per_1m": 0.1, "node_price_per_1m": 0.08, "cluster_price_per_1m": 0.05}]
```

#### `POST /admin/pricing` — Create/update pricing
```json
{
  "model_name": "google/gemma-2-2b-it",
  "provider_cost_input_per_1m": 0.124,
  "provider_cost_output_per_1m": 1.2,
  "bit_price_per_1m": 1.8,
  "node_price_per_1m": 1.5,
  "cluster_price_per_1m": 1.3
}
```

#### `GET /admin/pricing/delete?model=<name>` — Delete pricing
```json
{"deleted": "google/gemma-2-2b-it"}
```

---

### Transactions

#### `GET /admin/transactions` — Recent transactions (paginated)
```
GET /admin/transactions
GET /admin/transactions?cursor=<next_cursor>
GET /admin/transactions?prev=<prev_cursor>
```
```json
{
  "transactions": [{"id": "...", "model_name": "...", "input_tokens": 10, "output_tokens": 5, "cost_basis": 0.001, "revenue_generated": 0.002, "timestamp": "..."}],
  "next_cursor": "shard1|timestamp#uuid",
  "prev_cursor": null
}
```

---

### Prometheus

#### `GET /metrics` — Prometheus text format (admin auth)
```
# HELP tokenrouter_inflight_requests Current in-flight requests
# TYPE tokenrouter_inflight_requests gauge
tokenrouter_inflight_requests 0
# HELP tokenrouter_requests_total Total completed requests
# TYPE tokenrouter_requests_total counter
tokenrouter_requests_total 42
# HELP tokenrouter_requests_by_status_total Requests grouped by status code
# TYPE tokenrouter_requests_by_status_total counter
tokenrouter_requests_by_status_total{status="200"} 40
...
```
