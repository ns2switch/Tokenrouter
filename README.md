# TokenRouter — Multi-Provider LLM API Gateway + Billing Engine

**EN:** A high-performance, multi-provider LLM API gateway written in Rust. Proxies OpenAI-compatible endpoints to upstream inference providers with **per-token billing**, **tiered pricing** (Bit/Node/Cluster), **API key management**, **idempotency**, **request caching**, **Prometheus metrics**, and a **React admin dashboard** — all backed by **DynamoDB**. Deploys as an **AWS Lambda container** or standalone **axum TCP server**.

**ES:** Un gateway de APIs LLM multi-proveedor de alto rendimiento escrito en Rust. Hace proxy de endpoints compatibles con OpenAI hacia proveedores de inferencia con **facturación por token**, **precios por tier** (Bit/Node/Cluster), **gestión de API keys**, **idempotencia**, **caché de requests**, **métricas Prometheus** y un **panel de administración en React** — todo respaldado por **DynamoDB**. Se despliega como **contenedor AWS Lambda** o como **servidor TCP axum** independiente.

---

## Características / Features

### Gateway / API pública

| Endpoint | Auth | Facturación | Descripción |
|----------|------|:---:|---|
| `POST /v1/chat/completions` | Bearer key | Si | Chat completions con streaming y no-streaming. Límite 512 mensajes. Reintentos upstream (3 intentos, 5xx). Idempotencia vía header `idempotency-key`. |
| `POST /v1/completions` | Bearer key | Si | Endpoint legacy (OpenAI v1). Sin streaming. Timeout 60s. |
| `POST /v1/embeddings` | Bearer key | No | Passthrough de embeddings. Timeout 30s. |
| `POST /v1/audio/transcriptions` | Bearer key | No | Passthrough de transcripción de audio. Timeout 60s. |
| `POST /v1/audio/translations` | Bearer key | No | Passthrough de traducción de audio. Timeout 60s. |
| `POST /v1/images/generations` | Bearer key | No | Passthrough de generación de imágenes. Timeout 120s. |
| `GET /v1/models` | No | — | Lista de modelos agregada de todos los proveedores. Formato OpenRouter u OpenAI (`?format=openai`). |
| `GET /v1/openai/models` | No | — | Alias para formato OpenAI. |
| `GET /health` | No | — | Health check público. |
| `GET /metrics` | Admin | — | Métricas en formato texto Prometheus. |

### Multi-proveedor / Multi-provider

- **Proveedor por defecto** configurado vía `NIM_BASE_URL` + `NIM_API_KEY` (solo en memoria, nunca en DynamoDB).
- **Proveedores adicionales** almacenados en la tabla `providers` de DynamoDB, cacheados en `ProviderRegistry`.
- Resolución: `modelo → pricing_config → provider_id → provider`. Si no hay pricing, se usa el proveedor por defecto.
- `/v1/models` agrega modelos de **todos** los proveedores cacheados.
- Refresco de caché al iniciar, tras CRUD de providers, o manual (`POST /admin/providers/refresh`).

### Facturación / Billing

- Tokenización con `tiktoken-rs` (`cl100k_base`, mismo tokenizer que GPT-4).
- Precios por modelo y por tier (Bit / Node / Cluster) para tokens de entrada y salida.
- Costo del proveedor + margen por tier. Límites de crédito por API key (opcional).
- Escritura **atómica** en DynamoDB (`TransactWriteItems`): transacción + timeline + idempotencia + balance + métricas.
- Si `TransactWriteItems` falla, la transacción se escribe en la tabla `dead_letter` para recuperación manual.

### Seguridad / Security

- **Hashing de API keys:** SHA-256 con secret pepper (`API_KEY_HASH_SECRET`). Las claves raw nunca se almacenan.
- **Hashing de payloads:** HMAC-SHA-256 para idempotencia (domain-separated del hashing de keys).
- **Admin auth:** Bearer token con comparación en tiempo constante. Soporta rotación multi-token (`ADMIN_BEARER_TOKENS` CSV).
- **Admin sessions:** Cookies HttpOnly/Secure firmadas con HMAC — sin estado, sin base de datos.
- **Rate limiting de admin:** Por IP, ventana de 60s, máx 5 fallos (configurable).
- **IP allowlist:** Filtro CIDR para rutas admin (`ADMIN_IP_ALLOWLIST`).
- **Límites de concurrencia:** Global (`MAX_INFLIGHT_REQUESTS`, default 200) y por API key (`MAX_INFLIGHT_PER_KEY`, default 10).
- **Security headers:** `X-Content-Type-Options`, `X-Frame-Options`, `CSP`, `HSTS`, `Referrer-Policy`.
- **Body limit:** 8 MB. Sanitización de paths en SPA fallback.

### Panel de administración / Admin Dashboard

SPA en React 19 + Vite 8 + Tailwind CSS 4 con páginas para:

| Página | Funcionalidad |
|--------|-------------|
| Dashboard | Métricas en vivo: margen, tokens, transacciones, inflight, histogramas TTFT/duración, throughput. Auto-refresh 15s. |
| API Keys | CRUD completo: crear (`sk-{UUID}`), listar, activar/desactivar, límite de crédito, eliminar. |
| Pricing | CRUD de configuraciones de precio por modelo, proveedor y tier. |
| Providers | CRUD de proveedores upstream: URL, API key, cuantización, context length, etc. |
| Transactions | Lista paginada de transacciones con navegación por cursor. |
| Dead Letter | Entradas fallidas de `TransactWriteItems`. |
| Health | Estado de conectividad por proveedor. |

Servido como archivos estáticos desde `frontend/dist/`.

### Caché / Caching

| Caché | Tipo | Configuración |
|-------|------|-------------|
| Request cache | In-memory, content-addressable (hash del payload), TTL + LRU | `REQUEST_CACHE_TTL_SECONDS` (3600s), `REQUEST_CACHE_MAX_ENTRIES` (1000), `REQUEST_CACHE_MAX_RESPONSE_BYTES` (65536) |
| Provider registry | In-memory (`RwLock<HashMap>`), event-driven refresh | Sin TTL, refresco tras CRUD de providers |
| Tokenizer BPE | `Arc<CoreBPE>` (`cl100k_base`), cargado al iniciar | — |

### Métricas / Metrics (Prometheus)

Expuestas en `GET /metrics`:

- `tokenrouter_inflight_requests` — requests en vuelo
- `tokenrouter_requests_total` — total de requests completados
- `tokenrouter_requests_by_status_total` — por código HTTP
- `tokenrouter_requests_by_model_total` — por modelo
- `tokenrouter_ttft_ms` — histograma time-to-first-token (10 buckets)
- `tokenrouter_duration_ms` — histograma de duración total
- `tokenrouter_throughput_tps_x100_sum` — throughput de salida

Snapshots persistidos a DynamoDB cada `METRICS_FLUSH_INTERVAL_SECONDS` (default 300s). Se fusionan al reiniciar.

### DynamoDB (8 tablas)

| Tabla | PK | SK | Propósito |
|-------|-----|-----|-----------|
| `api_keys` | `key_hash` | — | API keys: id, tier, balance, credit_limit, active |
| `pricing_config` | `model_name` | — | Precios por modelo y tier |
| `transactions` | `id` | — | Registros individuales de facturación |
| `transactions_timeline` | `timeline_pk` | `timeline_sk` | Timeline shardeado para queries recientes |
| `idempotency_keys` | `idempotency_key` | — | Idempotencia con TTL en `expires_at` |
| `metrics_global` | `metric_id` | — | Agregados globales + snapshots de runtime |
| `providers` | `id` | — | Configuraciones de proveedores upstream |
| `dead_letter` | `id` | — | TransactWriteItems fallidos (opcional) |

### Modos de ejecución / Runtime modes

| Modo | `RUN_MODE` | Comportamiento |
|------|-----------|----------------|
| Lambda | `lambda` (default) | `lambda_http::run(app)` para AWS Lambda container image |
| Server | `server` | `axum::serve` TCP en `LISTEN_ADDR` (default `0.0.0.0:8080`) |

---

## Quickstart local

```bash
cp .env.example .env   # editar NIM_API_KEY y ADMIN_BEARER_TOKEN
docker compose up --build -d
./scripts/bootstrap_dynamodb_local.sh
```

- App: `http://localhost:8080`
- Admin dashboard: `http://localhost:8080/admin`
- DynamoDB local: `http://localhost:8000`

---

## Variables de entorno principales / Key env vars

### Requeridas / Required

| Variable | Descripción |
|----------|-------------|
| `NIM_BASE_URL` | URL base del proveedor por defecto |
| `NIM_API_KEY` | API key del proveedor por defecto |
| `API_KEY_HASH_SECRET` | Secreto para hashing SHA-256 de API keys y sesiones admin |
| `ADMIN_BEARER_TOKEN` | Token de admin (fallback si `ADMIN_BEARER_TOKENS` no está configurado) |

### Límites y facturación / Limits & billing

| Variable | Default | Descripción |
|----------|---------|-------------|
| `MAX_INFLIGHT_REQUESTS` | 200 | Concurrencia global máxima |
| `MAX_INFLIGHT_PER_KEY` | 10 | Concurrencia máxima por API key |
| `MAX_OUTPUT_TOKENS` | 16384 | Tope de `max_tokens` / `max_completion_tokens` |
| `IDEMPOTENCY_TTL_SECONDS` | 86400 | Expiración de claves de idempotencia |
| `MAX_STREAMING_DURATION_SECONDS` | 600 | Duración máxima de streaming |

### Admin

| Variable | Default | Descripción |
|----------|---------|-------------|
| `ADMIN_BEARER_TOKENS` | — | CSV de tokens (prioridad sobre token único) |
| `ADMIN_IP_ALLOWLIST` | — | CSV de CIDRs (bloquea IPs no incluidas) |
| `ADMIN_RATE_LIMIT_MAX_FAILURES` | 5 | Máx intentos fallidos de auth |
| `ADMIN_RATE_LIMIT_WINDOW_SECONDS` | 60 | Ventana de rate limiting |

---

## Build & test

```bash
cargo check                                      # Compile-check rápido
cargo build --release                            # Build release completo
cargo test --lib                                 # Tests unitarios (sin DynamoDB)
cargo test --test integration_test -- --test-threads=1  # Tests de integración (requiere DynamoDB local)
cargo clippy -- -D warnings                      # Lint
cargo fmt --check                                # Formato
```

Tests de integración requieren DynamoDB local en `localhost:8000`:

```bash
docker compose up -d dynamodb-local
AWS_REGION=us-east-1 AWS_ENDPOINT_URL_DYNAMODB=http://localhost:8000 \
  AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test \
  cargo test --test integration_test -- --test-threads=1
```

---

## IAM permissions (Lambda)

```
dynamodb:GetItem
dynamodb:PutItem
dynamodb:UpdateItem
dynamodb:DeleteItem
dynamodb:Scan
dynamodb:Query
dynamodb:TransactWriteItems
dynamodb:DescribeTable
dynamodb:DescribeTimeToLive
```

---

## Scripts

| Script | Descripción |
|--------|-------------|
| `scripts/bootstrap_dynamodb_local.sh` | Crea las 8 tablas en DynamoDB local |
| `scripts/backfill_idempotency_ttl.sh` | Agrega TTL a registros de idempotencia existentes |
| `scripts/run_local_stack.sh` | Todo en uno: DynamoDB + bootstrap + app |
| `scripts/provider_compliance_check.sh` | Valida formato de `/v1/models` |
| `scripts/e2e_verify.sh` | E2E: auth admin, health, idempotencia, smoke |
| `scripts/load_smoke.sh` | Test de carga ligero (200/429/5xx) |

---

## Arquitectura / Architecture

```
src/
├── main.rs                  — entrypoint, env loading, router assembly
├── lib.rs                   — re-exports
├── domain/
│   ├── entities.rs          — ApiKey, Tier, Transaction, PricingConfig, Provider
│   └── ports.rs             — Traits: Repository, ApiKeyPort, TransactionPort, etc.
├── application/
│   └── billing.rs           — Lógica de facturación + tokenización tiktoken-rs
├── infrastructure/
│   └── dynamodb.rs          — DynamoDbStore: implementación de todos los ports
├── interfaces/
│   ├── proxy.rs             — POST /v1/chat/completions (stream + no-stream)
│   ├── completions.rs       — POST /v1/completions (legacy)
│   ├── embeddings.rs        — POST /v1/embeddings (passthrough)
│   ├── images.rs            — POST /v1/images/generations (passthrough)
│   ├── audio.rs             — POST /v1/audio/* (passthrough)
│   ├── provider.rs          — GET /v1/models, GET /v1/openai/models
│   ├── admin.rs             — API JSON de administración
│   ├── admin_auth.rs        — Middleware Bearer token + rate limiting + CIDR
│   ├── metrics.rs           — GET /metrics (Prometheus)
│   ├── dto.rs               — DTOs de request/response
│   └── error.rs             — GateError, ScopedInflightGuard
├── security.rs              — SHA-256 hashing para API keys y payloads
├── runtime_metrics.rs       — Métricas Prometheus en memoria + flush a DynamoDB
├── state.rs                 — AppState compartido entre rutas axum
├── per_key_limiter.rs       — Limitador de concurrencia por API key (RAII)
├── request_cache.rs         — Caché de requests content-addressable (TTL + LRU)
└── provider_registry.rs     — Registro de proveedores cacheado (lazy load)
```

Patrón hexagonal / puertos y adaptadores. `AppState` contiene `Arc<dyn Repository>`.

## Descripción para el repositorio / Repository description

```
[EN] High-performance multi-provider LLM API gateway in Rust. OpenAI-compatible proxy with per-token billing, tiered pricing, DynamoDB persistence, Prometheus metrics, and a React admin dashboard. Deploys as AWS Lambda container or standalone server.

[ES] Gateway multi-proveedor de APIs LLM de alto rendimiento en Rust. Proxy compatible con OpenAI con facturación por token, precios por tier, persistencia en DynamoDB, métricas Prometheus y panel de administración en React. Se despliega como contenedor AWS Lambda o servidor independiente.
```

## Tags / Topics

`rust` `llm` `api-gateway` `openai` `dynamodb` `aws-lambda` `billing` `prometheus` `react` `multi-provider`
