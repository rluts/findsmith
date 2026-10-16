#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

npm run build

# Export every variable defined in .env so cargo run inherits them.
set -a
source .env
set +a

cargo run "$@"
