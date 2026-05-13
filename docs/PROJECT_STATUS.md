# TokenRouter — Project Status

**Last updated:** 2026-05-06

## Current State: Production-Ready MVP

TokenRouter is an inference gateway + billing engine for NVIDIA NIM, built in Rust with axum + aws-sdk-dynamodb. It proxies OpenAI-compatible requests, counts tokens with tiktoken-rs, applies tier-based pricing, caches responses, and persists everything atomically to DynamoDB.

### Tech Stack

| Component | Technology |
|-----------|-----------|
| Language | Rust 1.95+ |
| HTTP framework | axum 0.7 |
| Token counting | tiktoken-rs 0.6 |
| Database | DynamoDB (aws-sdk-dynamodb v1) |
| Frontend | React (Vite + Tailwind CSS) |
| Deploy | Docker + AWS Lambda (container image) |
| IaC | Terraform (AWS provider 5.x) |
| CI | GitHub Actions |

### API Endpoints (10 public, 15 admin)

See [API.md](API.md) for the complete reference.

### Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the Clean Architecture diagram and module map.

### Operations

See [OPERATIONS.md](OPERATIONS.md) for deployment, configuration, monitoring, and scaling.

### Test Coverage

| Suite | Count | Command |
|-------|-------|---------|
| Unit | 50 | `cargo test --lib` |
| Integration | 9 | `cargo test --test integration_test -- --test-threads=1` |

### Deployment

| Target | How | File |
|--------|-----|------|
| Local Docker | `docker compose up --build -d` | `docker-compose.yml` |
| AWS Lambda | `terraform apply` | `terraform/lambda.tf` |

### Health

```
GET /health → {"status": "ok"}                     (public, no auth)
GET /admin/health → ok=True, 3 checks               (admin auth required)
```

### Config

See [OPERATIONS.md](OPERATIONS.md) for the complete environment variable reference (35+ variables).

### What's Next

See [IMPLEMENTED_AND_PENDING.md](IMPLEMENTED_AND_PENDING.md) for the pending features list.
