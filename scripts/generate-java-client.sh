#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-$ROOT/build/java-client}"
SPEC="${OPENAPI_SPEC:-$ROOT/openapi/notes-server.yaml}"
CONFIG="${OPENAPI_GENERATOR_CONFIG:-$ROOT/openapi/java-client-config.yaml}"
GENERATOR_VERSION="${OPENAPI_GENERATOR_VERSION:-7.8.0}"
GENERATOR_CACHE="${OPENAPI_GENERATOR_CACHE:-$ROOT/.cache/openapi-generator}"
GENERATOR_JAR="$GENERATOR_CACHE/openapi-generator-cli-$GENERATOR_VERSION.jar"

mkdir -p "$GENERATOR_CACHE"
if [[ ! -f "$GENERATOR_JAR" ]]; then
  curl -fsSL \
    "https://repo1.maven.org/maven2/org/openapitools/openapi-generator-cli/$GENERATOR_VERSION/openapi-generator-cli-$GENERATOR_VERSION.jar" \
    -o "$GENERATOR_JAR.tmp"
  mv "$GENERATOR_JAR.tmp" "$GENERATOR_JAR"
fi

rm -rf "$OUT_DIR"
java -jar "$GENERATOR_JAR" validate -i "$SPEC"
java -jar "$GENERATOR_JAR" generate \
  -i "$SPEC" \
  -g java \
  -c "$CONFIG" \
  -o "$OUT_DIR"

if command -v mvn >/dev/null 2>&1 && [[ -f "$OUT_DIR/pom.xml" ]]; then
  (cd "$OUT_DIR" && mvn -q -DskipTests package)
fi
