#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

if ! cargo watch --version >/dev/null 2>&1; then
    echo "cargo-watch is required: cargo install cargo-watch" >&2
    exit 1
fi

# Export every variable defined in .env so the server inherits them.
set -a
source .env
set +a

# Rebuild and restart the server when backend sources or message catalogs
# change; frontend changes are handled by the Vite dev server below.
cargo watch -w src -w i18n -w build.rs -w Cargo.toml -w Cargo.lock -- cargo run "$@" &
BACKEND_PID=$!
# Stop cargo watch and the server it started, together with Vite, on exit.
trap 'trap - EXIT; kill 0 2>/dev/null' EXIT INT TERM

# Start Vite only once the backend answers, so its proxy has something to reach.
BACKEND_URL="http://127.0.0.1:${PORT:-8000}/healthz"
echo "Waiting for the backend at $BACKEND_URL…"
until curl -fs -o /dev/null "$BACKEND_URL"; do
    if ! kill -0 "$BACKEND_PID" 2>/dev/null; then
        echo "The backend stopped before it was ready." >&2
        exit 1
    fi
    sleep 1
done

npm run dev
