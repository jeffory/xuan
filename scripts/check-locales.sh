#!/usr/bin/env bash
# Reports how much of the interface each locale in assets/locales translates: the strings of
# en.tsv it leaves in English (untranslated) and the keys it has that en.tsv no longer lists
# (stale, so never shown). Exits with 1 when a locale has stale keys or is missing.
#
# Usage: scripts/check-locales.sh [--summary] [TAG...]
#   TAG        a locale to check, such as zh-CN (default: all but en)
#   --summary  counts only, without the lists
#
# `cargo test` checks the rest: that every tr("…") key is in en.tsv and every file reads cleanly.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

dir=assets/locales
summary=false
tags=()
for arg in "$@"; do
    case $arg in
        --summary) summary=true ;;
        -h | --help)
            sed -n '2,/^$/s/^# \{0,1\}//p' "$0"
            exit 0
            ;;
        -*)
            echo "Unknown option $arg; see --help" >&2
            exit 2
            ;;
        *) tags+=("$arg") ;;
    esac
done
if ((${#tags[@]} == 0)); then
    for file in "$dir"/*.tsv; do
        tag=$(basename -- "$file" .tsv)
        [[ $tag == en ]] || tags+=("$tag")
    done
fi

status=0
for tag in "${tags[@]}"; do
    file=$dir/$tag.tsv
    if [[ ! -f $file ]]; then
        echo "$tag: there is no $file" >&2
        status=1
        continue
    fi
    # Keys are compared as written, escapes and all: both files use the same escapes.
    awk -F '\t' -v tag="$tag" -v summary="$summary" '
        { sub(/\r$/, "") }
        /^@name\t/ && FILENAME != ARGV[1] { name = $2 }
        /^$/ || /^#/ || /^@/ { next }
        FILENAME == ARGV[1] {
            if (!($1 in english)) { english[$1] = 1; keys[++count] = $1 }
            next
        }
        {
            if (!($1 in listed)) { listed[$1] = 1; order[++locale_count] = $1 }
            if ($2 != "") translated[$1] = 1
        }
        END {
            done = 0
            for (i = 1; i <= count; i++) if (keys[i] in translated) done++
            stale = 0
            for (i = 1; i <= locale_count; i++) if (!(order[i] in english)) stale++
            percent = count ? int(100 * done / count) : 100
            printf "%s (%s): %d of %d strings translated (%d%%), %d untranslated, %d stale\n",
                tag, name == "" ? "no @name" : name, done, count, percent, count - done, stale
            if (summary != "true") {
                if (done < count) print "Untranslated:"
                for (i = 1; i <= count; i++) if (!(keys[i] in translated)) print "  " keys[i]
                if (stale) print "Stale (not in en.tsv):"
                for (i = 1; i <= locale_count; i++) if (!(order[i] in english)) print "  " order[i]
            }
            exit stale > 0
        }
    ' "$dir/en.tsv" "$file" || status=1
done
exit "$status"
