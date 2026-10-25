# Multi-stage Containerfile for Sugie GitHub Triage Bot (Podman native)
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

# Rootless container support
RUN useradd -m -u 1000 -U sugie && \
    mkdir -p /app/data && \
    chown -R sugie:sugie /app

USER sugie

EXPOSE 8080

ENTRYPOINT ["sugie"]
CMD ["serve"]
