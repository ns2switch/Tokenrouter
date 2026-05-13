#!/usr/bin/env bash
set -euo pipefail

ENDPOINT="${AWS_ENDPOINT_URL_DYNAMODB:-http://localhost:8000}"
REGION="${AWS_REGION:-us-east-1}"
TABLE="${DDB_IDEMPOTENCY_TABLE:-idempotency_keys}"
TTL_SECONDS="${IDEMPOTENCY_TTL_SECONDS:-86400}"
NOW_EPOCH="$(date +%s)"
EXPIRES_AT="$((NOW_EPOCH + TTL_SECONDS))"

KEYS=$(aws dynamodb scan \
  --table-name "$TABLE" \
  --endpoint-url "$ENDPOINT" \
  --region "$REGION" \
  --query "Items[?!contains(keys(@), 'expires_at')].idempotency_key.S" \
  --output text)

if [[ -z "${KEYS// }" ]]; then
  echo "No idempotency records require backfill"
  exit 0
fi

for key in $KEYS; do
  aws dynamodb update-item \
    --table-name "$TABLE" \
    --endpoint-url "$ENDPOINT" \
    --region "$REGION" \
    --key "{\"idempotency_key\":{\"S\":\"$key\"}}" \
    --update-expression "SET expires_at = :exp" \
    --expression-attribute-values "{\":exp\":{\"N\":\"$EXPIRES_AT\"}}" >/dev/null
  echo "Backfilled expires_at for $key"
done

echo "Backfill completed"
