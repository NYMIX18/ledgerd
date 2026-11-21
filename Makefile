.PHONY: db run test lint bench demo down

DATABASE_URL ?= postgres://ledger:ledger@localhost:5432/ledger

db:        ## start a local Postgres in Docker
	docker compose up -d db

run:       ## run the API locally (needs `make db` and a .env)
	cargo run

test:      ## run unit + integration tests (needs Postgres; tests create their own databases)
	DATABASE_URL=postgres://ledger:ledger@localhost:5432/postgres cargo test

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

bench:     ## posting-path benchmark (release build)
	DATABASE_URL=$(DATABASE_URL) cargo run --release --example bench

demo:      ## walk through the API with curl (server must be running)
	./scripts/demo.sh

down:
	docker compose down -v
