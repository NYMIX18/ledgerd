# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS build
WORKDIR /app
# Cache dependencies separately from source
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
    && cargo build --release && rm -rf src
COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs src/lib.rs && cargo build --release

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home ledger
COPY --from=build /app/target/release/ledgerd /usr/local/bin/ledgerd
USER ledger
EXPOSE 8080
ENV BIND_ADDR=0.0.0.0:8080 RUST_LOG=info
ENTRYPOINT ["ledgerd"]
