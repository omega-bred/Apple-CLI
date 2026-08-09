#!/usr/bin/env bash
set -euo pipefail

workflow=".github/workflows/build.yml"
config="openapi/java-client-config.yaml"
script="scripts/generate-java-client.sh"

test -f "$workflow"
test -f "$config"
test -x "$script"

grep -q "openapi/notes-server.yaml" "$workflow"
grep -q "scripts/generate-java-client.sh" "$workflow"
grep -q "openapi-generator-cli" "$script"
grep -q "apple-notes-java-client" "$workflow"
grep -q "actions/upload-artifact" "$workflow"

grep -q "artifactId: apple-notes-java-client" "$config"
grep -q "groupId: land.bre.apple" "$config"
grep -q "apiPackage: land.bre.apple.notes.api" "$config"
grep -q "modelPackage: land.bre.apple.notes.model" "$config"
