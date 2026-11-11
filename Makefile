.PHONY: help proto server-test server-run client client-run client-test deploy

help:
	@echo "targets: proto server-test server-run client client-run client-test deploy"

# Regenerates the Go bindings (the Rust ones are built by client/proto/build.rs).
proto:
	protoc -I proto --go_out=server/internal/pb --go_opt=paths=source_relative proto/frontline.proto

server-test:
	cd server && go vet ./... && go test -race ./...

# In-memory server on the default ports (nothing is saved).
server-run:
	cd server && go run ./cmd/server -memory -shared ../shared

client:
	cd client && cargo build --release -p frontline

# Usage: make client-run SERVER=127.0.0.1
client-run:
	cd client && FF_SERVER=$(or $(SERVER),76.179.76.99) cargo run --release -p frontline

client-test:
	cd client && cargo test --workspace

deploy:
	deploy/deploy.sh
