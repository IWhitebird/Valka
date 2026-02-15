#!/usr/bin/env bash
# Generate TypeScript gRPC stubs from shared proto files using ts-proto.
# Run from sdks/typescript/: bash generate_proto.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROTO_DIR="$REPO_ROOT/proto"
OUT_DIR="$SCRIPT_DIR/src/generated"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

protoc \
    --proto_path="$PROTO_DIR" \
    --plugin=protoc-gen-ts_proto="$SCRIPT_DIR/node_modules/.bin/protoc-gen-ts_proto" \
    --ts_proto_out="$OUT_DIR" \
    --ts_proto_opt=outputServices=nice-grpc,outputServices=generic-definitions \
    --ts_proto_opt=esModuleInterop=true \
    --ts_proto_opt=env=node \
    --ts_proto_opt=useOptionals=messages \
    --ts_proto_opt=forceLong=number \
    --ts_proto_opt=importSuffix=.js \
    valka/v1/common.proto \
    valka/v1/api.proto \
    valka/v1/worker.proto \
    valka/v1/events.proto

echo "TypeScript proto stubs generated in $OUT_DIR"
