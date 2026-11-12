#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/window-bridge-build.XXXXXX")"
OUTPUT_DIR="$SCRIPT_DIR/build"
OUTPUT_JAR="$OUTPUT_DIR/window-bridge-agent.jar"

cleanup() {
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

echo "========== Compile WindowBridge for Java 21 =========="
javac --release 21 \
    -d "$TEMP_DIR" \
    "$SCRIPT_DIR/WindowBridge.java"

echo
echo "========== Build independent Agent JAR =========="
mkdir -p "$OUTPUT_DIR"

jar --create \
    --file "$OUTPUT_JAR" \
    --manifest "$SCRIPT_DIR/manifest.mf" \
    -C "$TEMP_DIR" .

echo
echo "[OK] Built: $OUTPUT_JAR"
jar --describe-module --file "$OUTPUT_JAR" 2>/dev/null || true
