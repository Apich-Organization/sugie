# Multi-stage build for Sugie GitHub Triage Bot
FROM rust:1.80-bullseye as builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY templates ./templates

RUN cargo build --release

FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/sugie /usr/local/bin/sugie
COPY templates/sugie.toml /app/sugie.toml

EXPOSE 8080

ENTRYPOINT ["sugie"]
CMD ["serve"]
