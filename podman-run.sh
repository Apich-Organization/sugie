#!/usr/bin/env bash
set -euo pipefail

# Sugie Podman Runner Script
# Usage: ./podman-run.sh [start|stop|restart|logs|status|build]

IMAGE_NAME="localhost/sugie:latest"
CONTAINER_NAME="sugie-bot"
CONFIG_FILE="$(pwd)/sugie.toml"
ENV_FILE="$(pwd)/.env"
DATA_DIR="$(pwd)/data"

# Publish the port the bot actually listens on. Reading it from the config avoids
# the classic "container is running but nothing answers" mismatch.
PORT="${SUGIE_PORT:-$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9]\+\).*/\1/p' "${CONFIG_FILE}" 2>/dev/null | head -1)}"
PORT="${PORT:-8800}"

mkdir -p "${DATA_DIR}"

build_image() {
    echo "📦 Building Podman container image: ${IMAGE_NAME}..."
    podman build -t "${IMAGE_NAME}" -f Containerfile .
}

start_container() {
    if [ ! -f "${CONFIG_FILE}" ]; then
        echo "❌ Configuration file ${CONFIG_FILE} not found."
        echo "💡 Run './target/release/sugie init' or copy templates/sugie.toml first."
        exit 1
    fi

    echo "🚀 Starting Sugie Bot with Podman..."
    # The image ships no secrets: without .env the bot starts but every GitHub
    # delivery is rejected, so refuse to run rather than fail later.
    if [ ! -f "${ENV_FILE}" ]; then
        echo "❌ Environment file ${ENV_FILE} not found."
        echo "💡 Copy templates/.env.example to .env and fill in the secrets."
        exit 1
    fi

    podman run -d \
        --name "${CONTAINER_NAME}" \
        --replace \
        --restart always \
        --userns=keep-id:uid=1000,gid=1000 \
        -p "${PORT}:${PORT}" \
        -v "${DATA_DIR}:/app/data:Z" \
        -v "${CONFIG_FILE}:/app/sugie.toml:rw,Z" \
        -v "${ENV_FILE}:/app/.env:rw,Z" \
        -e RUST_LOG=info \
        -e SUGIE_DB_PATH=/app/data/sugie.db \
        "${IMAGE_NAME}" serve --config /app/sugie.toml

    echo "✅ Sugie Bot is running! Listening on port ${PORT}."
    echo "📋 Logs: podman logs -f ${CONTAINER_NAME}"
}

stop_container() {
    echo "🛑 Stopping Sugie Bot container..."
    podman stop "${CONTAINER_NAME}" || true
}

logs_container() {
    podman logs -f "${CONTAINER_NAME}"
}

status_container() {
    podman ps -a --filter "name=${CONTAINER_NAME}"
}

COMMAND="${1:-start}"

case "${COMMAND}" in
    build)
        build_image
        ;;
    start)
        if ! podman image exists "${IMAGE_NAME}"; then
            build_image
        fi
        start_container
        ;;
    stop)
        stop_container
        ;;
    restart)
        stop_container
        start_container
        ;;
    logs)
        logs_container
        ;;
    status)
        status_container
        ;;
    *)
        echo "Usage: $0 [start|stop|restart|logs|status|build]"
        exit 1
        ;;
esac
