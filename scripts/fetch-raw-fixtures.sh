#!/usr/bin/env bash
# Download the real camera files listed in testdata/raw/fixtures.txt, verifying
# each against its pinned SHA-256. Files already present and valid are kept.
#
#   scripts/fetch-raw-fixtures.sh
#   XUAN_RAW_FIXTURE_DIR=~/raw-cache scripts/fetch-raw-fixtures.sh
#
# The RAW tests read the same directory (XUAN_RAW_FIXTURE_DIR, default
# testdata/raw/cache) and skip fixtures that are missing.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
manifest="$root/testdata/raw/fixtures.txt"
cache=${XUAN_RAW_FIXTURE_DIR:-$root/testdata/raw/cache}
mkdir -p "$cache"

# Callers pass "-" for standard input: newer macOS has its own sha256sum, which
# prints its usage instead of reading standard input when given no file.
if command -v sha256sum >/dev/null; then
    sha256() { sha256sum "$@"; }
else
    # Older macOS ships Perl's shasum rather than GNU coreutils; it takes the same check options.
    sha256() { shasum -a 256 "$@"; }
fi

fetched=0
while IFS='|' read -r id file sha size _format _camera _width _height _expect url; do
    id=${id//[[:space:]]/}
    case $id in '' | '#'*) continue ;; esac
    # Fields are padded with spaces; none of them contain any.
    file=${file//[[:space:]]/}
    sha=${sha//[[:space:]]/}
    size=${size//[[:space:]]/}
    url=${url//[[:space:]]/}
    target="$cache/$file"
    if [[ -f $target ]] && echo "$sha  $target" | sha256 --check --status -; then
        echo "ok      $file (cached)"
        continue
    fi
    echo "fetch   $file ($size bytes)"
    partial="$target.part"
    curl --fail --location --silent --show-error --retry 3 --output "$partial" "$url"
    echo "$sha  $partial" | sha256 --check --quiet -
    mv -- "$partial" "$target"
    fetched=$((fetched + 1))
done <"$manifest"

echo "RAW fixtures ready in $cache ($fetched downloaded)"
