#!/usr/bin/env bash
# Generate Go gRPC stubs from shared proto files.
# Run from sdks/go/: bash generate_proto.sh
# Or: go generate ./...

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROTO_DIR="$REPO_ROOT/proto"
OUT_DIR="$SCRIPT_DIR/proto/valkav1"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

GO_MODULE="github.com/valka-queue/valka/sdks/go"

# Use a temp dir for generation, then flatten the output
TMPDIR="$(mktemp -d)"
trap "rm -rf $TMPDIR" EXIT

protoc \
    --proto_path="$PROTO_DIR" \
    --go_out="$TMPDIR" \
    --go_opt=paths=source_relative \
    --go_opt=Mvalka/v1/common.proto="${GO_MODULE}/proto/valkav1" \
    --go_opt=Mvalka/v1/api.proto="${GO_MODULE}/proto/valkav1" \
    --go_opt=Mvalka/v1/worker.proto="${GO_MODULE}/proto/valkav1" \
    --go_opt=Mvalka/v1/events.proto="${GO_MODULE}/proto/valkav1" \
    --go-grpc_out="$TMPDIR" \
    --go-grpc_opt=paths=source_relative \
    --go-grpc_opt=Mvalka/v1/common.proto="${GO_MODULE}/proto/valkav1" \
    --go-grpc_opt=Mvalka/v1/api.proto="${GO_MODULE}/proto/valkav1" \
    --go-grpc_opt=Mvalka/v1/worker.proto="${GO_MODULE}/proto/valkav1" \
    --go-grpc_opt=Mvalka/v1/events.proto="${GO_MODULE}/proto/valkav1" \
    valka/v1/common.proto \
    valka/v1/api.proto \
    valka/v1/worker.proto \
    valka/v1/events.proto

# Flatten: move from nested valka/v1/ to the output dir
mv "$TMPDIR"/valka/v1/*.go "$OUT_DIR/"

echo "Go proto stubs generated in $OUT_DIR"
