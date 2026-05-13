#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
OUT="$(curl -sS "$BASE_URL/v1/models")"

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for provider_compliance_check.sh"
  exit 1
fi

count=$(echo "$OUT" | jq '.data | length')
if [[ "$count" -lt 1 ]]; then
  echo "FAIL: /v1/models returned empty data[]"
  exit 1
fi

required='[.data[] | has("id") and has("name") and has("created") and has("pricing") and has("supported_sampling_parameters") and has("supported_features")] | all'
if [[ "$(echo "$OUT" | jq -r "$required")" != "true" ]]; then
  echo "FAIL: missing required model fields"
  exit 1
fi

pricing_ok='[.data[].pricing | has("prompt") and has("completion") and has("image") and has("request") and has("input_cache_read")] | all'
if [[ "$(echo "$OUT" | jq -r "$pricing_ok")" != "true" ]]; then
  echo "FAIL: missing required pricing fields"
  exit 1
fi

string_pricing='[.data[].pricing | (.prompt|type=="string") and (.completion|type=="string") and (.image|type=="string") and (.request|type=="string") and (.input_cache_read|type=="string")] | all'
if [[ "$(echo "$OUT" | jq -r "$string_pricing")" != "true" ]]; then
  echo "FAIL: pricing fields must be strings"
  exit 1
fi

echo "PASS: /v1/models basic provider compliance checks"
