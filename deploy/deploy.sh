#!/usr/bin/env bash
# Deploy the server to the SSH host (default: hermes).
# Usage: deploy/deploy.sh [host]
set -euo pipefail
HOST="${1:-${FF_DEPLOY_HOST:-hermes}}"
DIR="${FF_DEPLOY_DIR:-frontline}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> syncing to $HOST:~/$DIR"
ssh "$HOST" "mkdir -p ~/$DIR"
rsync -az --delete \
  --exclude '.git' --exclude 'client/' --exclude 'target/' --exclude 'deploy/.env' \
  "$ROOT/server" "$ROOT/shared" "$ROOT/proto" "$ROOT/deploy" "$HOST:~/$DIR/"

echo "==> ensuring deploy/.env exists on $HOST"
ssh "$HOST" "cd ~/$DIR/deploy && if [ ! -f .env ]; then umask 077; printf 'POSTGRES_PASSWORD=%s\n' \"\$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')\" > .env; echo 'created .env'; fi"

echo "==> building and starting"
ssh "$HOST" "cd ~/$DIR/deploy && docker compose up -d --build --remove-orphans && docker compose ps"
