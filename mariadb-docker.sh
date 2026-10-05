#!/usr/bin/env bash
set -euo pipefail

# Run from the directory containing your trusted, Bash-compatible .env file.
if [[ ! -f .env ]]; then
  printf 'Error: .env not found in %s\n' "$PWD" >&2
  exit 1
fi

command -v docker >/dev/null 2>&1 || {
  printf 'Error: Docker is not installed.\n' >&2
  exit 1
}

# .env may contain NAME=value or NAME="value" assignments.
source ./.env

: "${DATABASE_NAME:?Set DATABASE_NAME in .env}"
: "${TOOL_TOOLSDB_USER:?Set TOOL_TOOLSDB_USER in .env}"
: "${TOOL_TOOLSDB_PASSWORD:?Set TOOL_TOOLSDB_PASSWORD in .env}"

export MARIADB_DATABASE="$DATABASE_NAME"
export MARIADB_USER="$TOOL_TOOLSDB_USER"
export MARIADB_PASSWORD="$TOOL_TOOLSDB_PASSWORD"

docker run --detach \
  --name findsmith-mariadb \
  --restart unless-stopped \
  --publish 127.0.0.1:3306:3306 \
  --volume findsmith-mariadb-data:/var/lib/mysql \
  --env MARIADB_DATABASE \
  --env MARIADB_USER \
  --env MARIADB_PASSWORD \
  --env MARIADB_RANDOM_ROOT_PASSWORD=1 \
  mariadb:lts

printf 'MariaDB container started. Address: 127.0.0.1:3306\n'
printf 'Initialization can take a few seconds. Data is stored in findsmith-mariadb-data.\n'
