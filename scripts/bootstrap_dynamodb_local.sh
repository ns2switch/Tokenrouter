#!/usr/bin/env bash
set -euo pipefail

ENDPOINT="${AWS_ENDPOINT_URL_DYNAMODB:-http://localhost:8000}"
REGION="${AWS_REGION:-us-east-1}"
API_KEY_HASH_SECRET="${API_KEY_HASH_SECRET:-dev-secret}"
IDEMPOTENCY_TTL_SECONDS="${IDEMPOTENCY_TTL_SECONDS:-86400}"

hash_key() {
  printf '%s' "${API_KEY_HASH_SECRET}:$1" | sha256sum | awk '{print $1}'
}

aws dynamodb create-table \
  --table-name api_keys \
  --attribute-definitions AttributeName=key_hash,AttributeType=S \
  --key-schema AttributeName=key_hash,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb create-table \
  --table-name pricing_config \
  --attribute-definitions AttributeName=model_name,AttributeType=S \
  --key-schema AttributeName=model_name,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb create-table \
  --table-name transactions \
  --attribute-definitions AttributeName=id,AttributeType=S \
  --key-schema AttributeName=id,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb create-table \
  --table-name transactions_timeline \
  --attribute-definitions AttributeName=timeline_pk,AttributeType=S AttributeName=timeline_sk,AttributeType=S \
  --key-schema AttributeName=timeline_pk,KeyType=HASH AttributeName=timeline_sk,KeyType=RANGE \
  --billing-mode PAY_PER_REQUEST \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb create-table \
  --table-name idempotency_keys \
  --attribute-definitions AttributeName=idempotency_key,AttributeType=S \
  --key-schema AttributeName=idempotency_key,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb update-time-to-live \
  --table-name idempotency_keys \
  --time-to-live-specification "Enabled=true, AttributeName=expires_at" \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb create-table \
  --table-name metrics_global \
  --attribute-definitions AttributeName=metric_id,AttributeType=S \
  --key-schema AttributeName=metric_id,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb create-table \
  --table-name providers \
  --attribute-definitions AttributeName=id,AttributeType=S \
  --key-schema AttributeName=id,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" >/dev/null 2>&1 || true

aws dynamodb put-item \
  --table-name api_keys \
  --item "{
    \"key_hash\": {\"S\": \"$(hash_key sk_demo_bit)\"},
    \"id\": {\"S\": \"11111111-1111-1111-1111-111111111111\"},
    \"tier\": {\"S\": \"bit\"},
    \"balance_accumulated\": {\"N\": \"0\"},
    \"active\": {\"BOOL\": true},
    \"created_at\": {\"S\": \"2026-01-01T00:00:00Z\"}
  }" \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION"

aws dynamodb put-item \
  --table-name api_keys \
  --item "{
    \"key_hash\": {\"S\": \"$(hash_key sk_demo_node)\"},
    \"id\": {\"S\": \"22222222-2222-2222-2222-222222222222\"},
    \"tier\": {\"S\": \"node\"},
    \"balance_accumulated\": {\"N\": \"0\"},
    \"active\": {\"BOOL\": true},
    \"created_at\": {\"S\": \"2026-01-01T00:00:00Z\"}
  }" \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION"

aws dynamodb put-item \
  --table-name api_keys \
  --item "{
    \"key_hash\": {\"S\": \"$(hash_key sk_demo_cluster)\"},
    \"id\": {\"S\": \"33333333-3333-3333-3333-333333333333\"},
    \"tier\": {\"S\": \"cluster\"},
    \"balance_accumulated\": {\"N\": \"0\"},
    \"active\": {\"BOOL\": true},
    \"created_at\": {\"S\": \"2026-01-01T00:00:00Z\"}
  }" \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION"

aws dynamodb put-item \
  --table-name pricing_config \
  --item '{
    "model_name": {"S": "google/gemma-4-31b-it"},
    "provider_id": {"S": "default"},
    "provider_cost_input_per_1m": {"N": "0.124"},
    "provider_cost_output_per_1m": {"N": "1.20"},
    "bit_price_per_1m": {"N": "1.80"},
    "node_price_per_1m": {"N": "1.50"},
    "cluster_price_per_1m": {"N": "1.30"},
    "updated_at": {"S": "2026-01-01T00:00:00Z"}
  }' \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION"

aws dynamodb put-item \
  --table-name metrics_global \
  --item '{
    "metric_id": {"S": "global"},
    "tx_count": {"N": "0"},
    "total_input_tokens": {"N": "0"},
    "total_output_tokens": {"N": "0"},
    "total_cost": {"N": "0"},
    "total_revenue": {"N": "0"},
    "updated_at": {"S": "2026-01-01T00:00:00Z"}
  }' \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION"

NOW_EPOCH="$(date +%s)"
EXPIRES_AT="$((NOW_EPOCH + IDEMPOTENCY_TTL_SECONDS))"
aws dynamodb put-item \
  --table-name idempotency_keys \
  --item "{
    \"idempotency_key\": {\"S\": \"seed-idempotency-sample\"},
    \"transaction_id\": {\"S\": \"seed\"},
    \"created_at\": {\"S\": \"2026-01-01T00:00:00Z\"},
    \"request_hash\": {\"S\": \"seed\"},
    \"expires_at\": {\"N\": \"$EXPIRES_AT\"}
  }" \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION"

echo "DynamoDB local bootstrap complete"
