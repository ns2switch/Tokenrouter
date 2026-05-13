#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
ADMIN_TOKEN="${ADMIN_BEARER_TOKEN:-change-me}"

echo "[1/5] /v1/models compliance"
./scripts/provider_compliance_check.sh

echo "[2/5] /admin/health unauthorized"
code=$(curl -sS -o /dev/null -w "%{http_code}" "$BASE_URL/admin/health")
[[ "$code" == "401" ]] || { echo "FAIL: expected 401, got $code"; exit 1; }

echo "[3/5] /admin/health authorized"
code=$(curl -sS -o /dev/null -w "%{http_code}" "$BASE_URL/admin/health" -H "Authorization: Bearer $ADMIN_TOKEN")
[[ "$code" == "200" ]] || { echo "FAIL: expected 200, got $code"; exit 1; }

echo "[4/5] idempotency"
k="e2e-idem-$(date +%s%N)"
first=$(curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/v1/chat/completions" \
  -H "Authorization: Bearer sk_demo_bit" \
  -H "Idempotency-Key: $k" \
  -H "Content-Type: application/json" \
  -d '{"model":"google/gemma-4-31b-it","messages":[{"role":"user","content":"e2e"}],"stream":false}')
second=$(curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/v1/chat/completions" \
  -H "Authorization: Bearer sk_demo_bit" \
  -H "Idempotency-Key: $k" \
  -H "Content-Type: application/json" \
  -d '{"model":"google/gemma-4-31b-it","messages":[{"role":"user","content":"e2e"}],"stream":false}')
[[ "$first" == "200" && "$second" == "409" ]] || { echo "FAIL: expected 200 then 409, got $first then $second"; exit 1; }

echo "[5/5] load smoke"
REQUESTS=${REQUESTS:-20} CONCURRENCY=${CONCURRENCY:-20} ./scripts/load_smoke.sh

echo "PASS: e2e verification completed"
