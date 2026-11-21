-- Double-entry ledger schema.
-- Amounts are stored as integer minor units (e.g. cents). No floats, ever.

CREATE TABLE accounts (
    id              UUID PRIMARY KEY,
    code            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('asset','liability','equity','revenue','expense')),
    currency        TEXT NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    -- Cached "natural" balance (debit-normal kinds: debits - credits; credit-normal: credits - debits).
    -- Always updated in the same DB transaction as the entries that change it.
    balance_minor   BIGINT NOT NULL DEFAULT 0,
    allow_negative  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT balance_not_negative CHECK (allow_negative OR balance_minor >= 0)
);

CREATE TABLE transactions (
    id               UUID PRIMARY KEY,
    idempotency_key  TEXT NOT NULL UNIQUE,
    request_hash     TEXT NOT NULL,
    description      TEXT NOT NULL,
    currency         TEXT NOT NULL,
    external_ref     TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE entries (
    id              BIGSERIAL PRIMARY KEY,
    transaction_id  UUID   NOT NULL REFERENCES transactions(id),
    account_id      UUID   NOT NULL REFERENCES accounts(id),
    direction       TEXT   NOT NULL CHECK (direction IN ('debit','credit')),
    amount_minor    BIGINT NOT NULL CHECK (amount_minor > 0),
    currency        TEXT   NOT NULL
);
CREATE INDEX entries_account_idx ON entries (account_id, id);
CREATE INDEX entries_txn_idx ON entries (transaction_id);

-- Invariant enforced by the database itself: every transaction balances.
-- Deferred, so all entries of a transaction can be inserted before the check runs at COMMIT.
CREATE FUNCTION check_transaction_balanced() RETURNS trigger AS $$
DECLARE
    d BIGINT;
    c BIGINT;
BEGIN
    SELECT COALESCE(SUM(amount_minor) FILTER (WHERE direction = 'debit'), 0)::BIGINT,
           COALESCE(SUM(amount_minor) FILTER (WHERE direction = 'credit'), 0)::BIGINT
      INTO d, c
      FROM entries WHERE transaction_id = NEW.transaction_id;
    IF d <> c THEN
        RAISE EXCEPTION 'unbalanced transaction %: debits=% credits=%', NEW.transaction_id, d, c
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER entries_balanced
    AFTER INSERT ON entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION check_transaction_balanced();

-- The ledger is append-only. Corrections are new (reversing) transactions.
CREATE FUNCTION forbid_mutation() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION '% on % is not allowed: ledger rows are append-only', TG_OP, TG_TABLE_NAME
        USING ERRCODE = '23514';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER entries_append_only BEFORE UPDATE OR DELETE ON entries
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER transactions_append_only BEFORE UPDATE OR DELETE ON transactions
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();

-- M-Pesa (Daraja) STK push bookkeeping.
CREATE TABLE payment_intents (
    id                    UUID PRIMARY KEY,
    checkout_request_id   TEXT NOT NULL UNIQUE,
    account_id            UUID NOT NULL REFERENCES accounts(id),
    amount_minor          BIGINT NOT NULL CHECK (amount_minor > 0),
    msisdn                TEXT NOT NULL,
    status                TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','completed','failed')),
    failure_reason        TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Raw callbacks are persisted first (fast ack), processed later by a worker.
CREATE TABLE mpesa_callbacks (
    id                    UUID PRIMARY KEY,
    checkout_request_id   TEXT NOT NULL UNIQUE,   -- duplicate deliveries collapse here
    result_code           INT  NOT NULL,
    payload               JSONB NOT NULL,
    status                TEXT NOT NULL DEFAULT 'received' CHECK (status IN ('received','processed','dead')),
    attempts              INT  NOT NULL DEFAULT 0,
    next_attempt_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error            TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at          TIMESTAMPTZ
);
CREATE INDEX mpesa_callbacks_due_idx ON mpesa_callbacks (next_attempt_at) WHERE status = 'received';
