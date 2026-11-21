#!/usr/bin/env bash
# End-to-end demo against a running server. Requires: curl, jq.
#   BASE=http://localhost:8080 API_KEY=change-me-dev-key WEBHOOK_TOKEN=change-me-webhook-token ./scripts/demo.sh
set -euo pipefail
BASE=${BASE:-http://localhost:8080}
API_KEY=${API_KEY:-change-me-dev-key}
WEBHOOK_TOKEN=${WEBHOOK_TOKEN:-change-me-webhook-token}
SUFFIX=$(date +%s)
H=(-H "Authorization: Bearer $API_KEY" -H "Content-Type: application/json")

say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }

say "1. Create accounts (a funding source and two customer wallets)"
SEED=$(curl -s "${H[@]}" -X POST "$BASE/v1/accounts" -d "{\"code\":\"SEED-$SUFFIX\",\"name\":\"Seed capital\",\"kind\":\"equity\",\"currency\":\"KES\",\"allow_negative\":true}" | jq -r .id)
ALICE=$(curl -s "${H[@]}" -X POST "$BASE/v1/accounts" -d "{\"code\":\"ALICE-$SUFFIX\",\"name\":\"Alice wallet\",\"kind\":\"liability\",\"currency\":\"KES\"}" | jq -r .id)
BOB=$(curl -s "${H[@]}" -X POST "$BASE/v1/accounts" -d "{\"code\":\"BOB-$SUFFIX\",\"name\":\"Bob wallet\",\"kind\":\"liability\",\"currency\":\"KES\"}" | jq -r .id)
echo "seed=$SEED alice=$ALICE bob=$BOB"

say "2. Fund Alice with KES 100.00 (10000 minor units)"
curl -s "${H[@]}" -H "Idempotency-Key: fund-$SUFFIX" -X POST "$BASE/v1/transactions" -d "{
  \"description\":\"initial funding\",\"currency\":\"KES\",
  \"entries\":[{\"account_id\":\"$SEED\",\"direction\":\"debit\",\"amount_minor\":10000},
               {\"account_id\":\"$ALICE\",\"direction\":\"credit\",\"amount_minor\":10000}]}" | jq -c '{id, description}'

say "3. Alice pays Bob KES 25.00 -- sent TWICE with the same Idempotency-Key"
BODY="{\"description\":\"Alice pays Bob\",\"currency\":\"KES\",
  \"entries\":[{\"account_id\":\"$ALICE\",\"direction\":\"debit\",\"amount_minor\":2500},
               {\"account_id\":\"$BOB\",\"direction\":\"credit\",\"amount_minor\":2500}]}"
curl -s -D - -o /dev/null "${H[@]}" -H "Idempotency-Key: pay-$SUFFIX" -X POST "$BASE/v1/transactions" -d "$BODY" | grep -iE "^HTTP|idempotent" || true
curl -s -D - -o /dev/null "${H[@]}" -H "Idempotency-Key: pay-$SUFFIX" -X POST "$BASE/v1/transactions" -d "$BODY" | grep -iE "^HTTP|idempotent" || true

say "4. Balances (Alice should be 7500, Bob 2500 -- not double charged)"
curl -s "${H[@]}" "$BASE/v1/accounts/$ALICE" | jq -c '{code, balance_minor}'
curl -s "${H[@]}" "$BASE/v1/accounts/$BOB"   | jq -c '{code, balance_minor}'

say "5. Overdraft attempt is rejected"
curl -s "${H[@]}" -H "Idempotency-Key: overdraft-$SUFFIX" -X POST "$BASE/v1/transactions" -d "{
  \"description\":\"too much\",\"currency\":\"KES\",
  \"entries\":[{\"account_id\":\"$ALICE\",\"direction\":\"debit\",\"amount_minor\":999999},
               {\"account_id\":\"$BOB\",\"direction\":\"credit\",\"amount_minor\":999999}]}" | jq -c .

say "6. M-Pesa deposit: create intent, then simulate Daraja's callback (sent twice)"
INTENT=$(curl -s "${H[@]}" -X POST "$BASE/v1/deposits/mpesa" -d "{\"account_id\":\"$BOB\",\"amount_minor\":50000,\"msisdn\":\"254708374149\"}")
echo "$INTENT" | jq -c .
CHECKOUT=$(echo "$INTENT" | jq -r .checkout_request_id)
CB="{\"Body\":{\"stkCallback\":{\"MerchantRequestID\":\"29115-34620561-1\",\"CheckoutRequestID\":\"$CHECKOUT\",\"ResultCode\":0,\"ResultDesc\":\"The service request is processed successfully.\",\"CallbackMetadata\":{\"Item\":[{\"Name\":\"Amount\",\"Value\":500.00},{\"Name\":\"MpesaReceiptNumber\",\"Value\":\"NLJ7RT61SV\"}]}}}}"
curl -s -X POST "$BASE/v1/webhooks/mpesa/$WEBHOOK_TOKEN" -H "Content-Type: application/json" -d "$CB" | jq -c .
curl -s -X POST "$BASE/v1/webhooks/mpesa/$WEBHOOK_TOKEN" -H "Content-Type: application/json" -d "$CB" | jq -c .
echo "(waiting for the background worker...)"; sleep 2.5
curl -s "${H[@]}" "$BASE/v1/accounts/$BOB" | jq -c '{code, balance_minor}  # expect 52500'

say "7. Reconciliation: books must balance"
curl -s "${H[@]}" "$BASE/v1/reconcile" | jq -c .

say "8. Metrics"
curl -s "$BASE/metrics" | grep -E '^ledger_' | head -12
