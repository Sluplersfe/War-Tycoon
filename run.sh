#!/usr/bin/env bash
# Builds and launches the Frontline Foundry client.
#   ./run.sh                 connect to the public server (76.179.76.99)
#   ./run.sh 127.0.0.1       connect to another host
#   ./run.sh --local         start a local in-memory server, then the client
set -euo pipefail
cd "$(dirname "$0")"

host="${FF_SERVER:-76.179.76.99}"
server_pid=""

case "${1:-}" in
  --local)
    host=127.0.0.1
    (cd server && go build -o /tmp/ffserver ./cmd/server)
    /tmp/ffserver -memory -shared "$PWD/shared" &
    server_pid=$!
    trap '[ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true' EXIT
    sleep 1
    ;;
  "") ;;
  *) host="$1" ;;
esac

cd client
FF_SERVER="$host" cargo run --release -p frontline
