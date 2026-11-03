#!/usr/bin/env bash
# Usage: check-version.sh TAG
# Prints the mqtt-zapp version if TAG is "v<version>", else fails: every
# package name carries the version, so a tag that disagrees would publish
# mislabelled files.
set -euo pipefail
tag=${1:?usage: check-version.sh TAG}
v=$(cargo metadata --no-deps --format-version 1 --manifest-path "$(dirname "$0")/../Cargo.toml" |
    jq -r '.packages[] | select(.name == "mqtt-zapp") | .version')
if [ "$tag" != "v$v" ]; then
    echo "tag $tag does not match Cargo version $v" >&2
    exit 1
fi
echo "$v"
