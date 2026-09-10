#!/bin/sh
# Regenerates pb/plugin_pb2.py and pb/plugin_pb2_grpc.py from the single
# source of truth, ../crates/eumeaus-plugin-protocol/plugin.proto. Run
# this after that file changes upstream — the checked-in pb/*.py exist so
# a user installing this plugin doesn't need protoc/grpcio-tools at all,
# only grpcio (requirements.txt) at runtime.
#
# Needs grpcio-tools, which is NOT in requirements.txt or
# requirements-dev.txt on purpose (it bundles its own protoc binary and is
# only ever needed for this one maintenance action):
#   .venv/bin/pip install grpcio-tools
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROTO_DIR="$SCRIPT_DIR/../crates/eumeaus-plugin-protocol"

"$SCRIPT_DIR/.venv/bin/python" -m grpc_tools.protoc \
  -I "$PROTO_DIR" \
  --python_out="$SCRIPT_DIR/pb" \
  --grpc_python_out="$SCRIPT_DIR/pb" \
  "$PROTO_DIR/plugin.proto"

echo "Regenerated $SCRIPT_DIR/pb/plugin_pb2.py and plugin_pb2_grpc.py"
