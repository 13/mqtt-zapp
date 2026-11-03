#!/usr/bin/env bash
# Checks for the packaging scripts; run from the repo root.
set -u
cd "$(dirname "$0")/.."
fail=0
v=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "mqtt-zapp") | .version')

out=$(packaging/check-version.sh "v$v" 2>&1) && [ "$out" = "$v" ] ||
    { echo "FAIL: matching tag: $out"; fail=1; }
out=$(packaging/check-version.sh v9.9.9 2>&1) &&
    { echo "FAIL: mismatched tag accepted"; fail=1; }
[[ $out == *"tag v9.9.9 does not match Cargo version $v"* ]] ||
    { echo "FAIL: mismatch message: $out"; fail=1; }
out=$(packaging/check-version.sh 0.1.0 2>&1) &&
    { echo "FAIL: tag without v accepted"; fail=1; }

[ $fail = 0 ] && echo "packaging tests: OK"
exit $fail
