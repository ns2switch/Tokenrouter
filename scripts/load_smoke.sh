#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
API_KEY="${API_KEY:-sk_demo_bit}"
MODEL="${MODEL:-google/gemma-4-31b-it}"
REQUESTS="${REQUESTS:-40}"
CONCURRENCY="${CONCURRENCY:-20}"
TIMEOUT_SECS="${TIMEOUT_SECS:-30}"
EXPECT_429="${EXPECT_429:-0}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

payload='{"model":"'"$MODEL"'","messages":[{"role":"user","content":"load test ping"}],"stream":false}'

run_one() {
  local idx="$1"
  local code
  code=$(curl --max-time "$TIMEOUT_SECS" -sS -o /dev/null -w "%{http_code}" \
    "$BASE_URL/v1/chat/completions" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Idempotency-Key: load-${idx}-$(date +%s%N)" \
    -H "Content-Type: application/json" \
    -d "$payload")
  echo "$code" >> "$TMP_DIR/codes.txt"
}

active=0
for i in $(seq 1 "$REQUESTS"); do
  run_one "$i" &
  active=$((active + 1))
  if (( active >= CONCURRENCY )); then
    wait -n
    active=$((active - 1))
  fi
done
wait

TOTAL=$(wc -l < "$TMP_DIR/codes.txt" | tr -d ' ')
OK_200=$(grep -c '^200$' "$TMP_DIR/codes.txt" || true)
TOO_MANY_429=$(grep -c '^429$' "$TMP_DIR/codes.txt" || true)
SERVER_5XX=$(grep -Ec '^5[0-9][0-9]$' "$TMP_DIR/codes.txt" || true)
OTHER=$((TOTAL - OK_200 - TOO_MANY_429 - SERVER_5XX))

cat <<OUT
Load smoke results
- total: $TOTAL
- 200: $OK_200
- 429: $TOO_MANY_429
- 5xx: $SERVER_5XX
- other: $OTHER
OUT

if [[ "$EXPECT_429" == "1" && "$TOO_MANY_429" -eq 0 ]]; then
  echo "FAIL: EXPECT_429=1 but no 429 responses were observed"
  exit 1
fi
