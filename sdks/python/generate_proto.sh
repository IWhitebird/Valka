#!/usr/bin/env bash
# Generate Python gRPC stubs from shared proto files.
# Run from sdks/python/: bash generate_proto.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROTO_DIR="$REPO_ROOT/proto"
OUT_DIR="$SCRIPT_DIR/src/valka/_proto"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

python3 -m grpc_tools.protoc \
    --proto_path="$PROTO_DIR" \
    --python_out="$OUT_DIR" \
    --grpc_python_out="$OUT_DIR" \
    --pyi_out="$OUT_DIR" \
    valka/v1/common.proto \
    valka/v1/api.proto \
    valka/v1/worker.proto \
    valka/v1/events.proto

# Create __init__.py files for the package hierarchy
touch "$OUT_DIR/__init__.py"
mkdir -p "$OUT_DIR/valka"
touch "$OUT_DIR/valka/__init__.py"
mkdir -p "$OUT_DIR/valka/v1"
touch "$OUT_DIR/valka/v1/__init__.py"

# Fix relative imports in generated files:
# grpc_tools generates "from valka.v1 import X" but we need relative imports
# within the _proto package.
if [[ "$(uname)" == "Darwin" ]]; then
    SED_I="sed -i ''"
else
    SED_I="sed -i"
fi

for f in "$OUT_DIR"/valka/v1/*_pb2*.py; do
    [ -f "$f" ] || continue
    $SED_I 's/^from valka\.v1 import/from . import/g' "$f"
    $SED_I 's/^from valka\.v1\./from ./g' "$f"
done

echo "Proto stubs generated in $OUT_DIR"
