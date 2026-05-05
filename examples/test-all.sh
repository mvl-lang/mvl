#!/usr/bin/env bash
# Run `make test` for every example subdirectory.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pass=0; fail=0

echo ""
for dir in "$SCRIPT_DIR"/*/; do
    [ -f "$dir/Makefile" ] || continue
    name="$(basename "$dir")"
    printf "  %-20s  " "$name"
    if out=$(make -C "$dir" --no-print-directory test 2>&1); then
        printf "\033[32m✓  PASS\033[0m\n"
        pass=$((pass + 1))
    else
        printf "\033[31m✗  FAIL\033[0m\n"
        printf "%s\n" "$out" | sed 's/^/         /'
        fail=$((fail + 1))
    fi
done
echo ""

if [ "$fail" -eq 0 ]; then
    printf "  \033[32m✓  All %d example(s) passed\033[0m\n\n" "$pass"
else
    printf "  \033[31m✗  %d of %d example(s) failed\033[0m\n\n" "$fail" "$((pass + fail))"
    exit 1
fi
