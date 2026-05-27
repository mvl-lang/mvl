#!/usr/bin/env bash
# pkg/zmq/tools/check-sync.sh
#
# Detect drift between re-declared functions in zmq_test.mvl and their
# source implementations.  Workaround #96 requires test files to copy
# function bodies locally; this script catches when source and test
# copies diverge.
#
# Approach:  compare normalised function signatures (declaration line
# up to the opening brace).  This catches parameter, return type, and
# effect changes — the most critical forms of drift.
#
# Exit code: 0 = all in sync, 1 = drift detected

set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="$DIR/src"
TEST_FILE="$SRC_DIR/zmq_test.mvl"
SOURCE_FILES=("$SRC_DIR/zmq.mvl" "$SRC_DIR/zmtp.mvl" "$SRC_DIR/pubsub.mvl")

# Known intentional variants — test functions that deliberately differ
# from source (empty if all re-declarations match source signatures).
ALLOW_LIST=(
)

# ── Extract function/type signatures ──────────────────────────────────────────
#
# Emits:  NAME<TAB>NORMALISED_SIGNATURE
#
# Normalisation:
#   - Strip pub/total/partial qualifiers
#   - Strip val parameter qualifiers
#   - Strip refinement annotations (where ...)
#   - Strip postconditions (ensures ...)
#   - Strip preconditions (requires ...)
#   - Collapse whitespace

extract_signatures() {
  local file="$1"
  local skip_test="${2:-false}"

  awk -v skip_test="$skip_test" '
    # Match top-level fn/type declarations (not indented = not inside another block)
    /^(pub )?(total |partial )?(fn |type |test fn )/ {

      # Skip test fn
      if (skip_test == "true" && $0 ~ /test fn /) next

      line = $0

      # For multi-line signatures, gather lines until we see "{"
      while (line !~ /\{/) {
        if ((getline nextline) <= 0) break
        # Skip requires/ensures/postcondition lines
        if (nextline ~ /^[[:space:]]*(requires|ensures) /) continue
        line = line " " nextline
      }

      # Normalise
      sig = line

      # Remove everything from { onwards (body)
      sub(/\{.*/, "", sig)

      # Remove qualifiers
      gsub(/^pub /, "", sig)
      gsub(/^total /, "", sig)
      gsub(/^partial /, "", sig)

      # Remove val qualifier on parameters
      gsub(/val /, "", sig)

      # Remove refinement annotations: "where self ..." up to , or )
      gsub(/ where [^,)]*/, "", sig)

      # Collapse whitespace
      gsub(/[[:space:]]+/, " ", sig)
      gsub(/^ /, "", sig)
      gsub(/ $/, "", sig)

      # Extract name
      name = sig
      if (name ~ /^type /) {
        sub(/^type /, "", name)
        sub(/ .*/, "", name)
        # For types, normalise to just "type Name = kind"
        type_sig = sig
        gsub(/\[.*/, "", type_sig)  # strip generic params for matching
      } else if (name ~ /^fn /) {
        sub(/^fn /, "", name)
        sub(/[(\[:].*/, "", name)
        # Handle extension methods
        if (name ~ /::/) sub(/.*::/, "", name)
      } else {
        next
      }

      if (name != "") printf "%s\t%s\n", name, sig
    }
  ' "$file"
}

# ── Build maps ────────────────────────────────────────────────────────────────

declare -A SRC_SIGS
declare -A SRC_FILES
declare -A TEST_SIGS

for src in "${SOURCE_FILES[@]}"; do
  basename=$(basename "$src")
  while IFS=$'\t' read -r name sig; do
    [[ -z "$name" ]] && continue
    SRC_SIGS["$name"]="$sig"
    SRC_FILES["$name"]="$basename"
  done < <(extract_signatures "$src" false)
done

while IFS=$'\t' read -r name sig; do
  [[ -z "$name" ]] && continue
  TEST_SIGS["$name"]="$sig"
done < <(extract_signatures "$TEST_FILE" true)

# ── Compare ───────────────────────────────────────────────────────────────────

printf "  Checking zmq_test.mvl re-declarations against source signatures\n"
printf "  (non-pub helpers can't be imported — re-declarations must stay in sync)\n\n"

DRIFT=0
SYNCED=0
SKIPPED=0
TEST_ONLY=0
MATCHED_NAMES=()

is_allowed() {
  local name="$1"
  for a in "${ALLOW_LIST[@]}"; do
    [[ "$a" == "$name" ]] && return 0
  done
  return 1
}

for name in $(echo "${!TEST_SIGS[@]}" | tr ' ' '\n' | sort); do
  if is_allowed "$name"; then
    SKIPPED=$((SKIPPED + 1))
    continue
  fi

  if [[ -v "SRC_SIGS[$name]" ]]; then
    MATCHED_NAMES+=("$name")
    src_sig="${SRC_SIGS[$name]}"
    test_sig="${TEST_SIGS[$name]}"
    if [[ "$src_sig" == "$test_sig" ]]; then
      SYNCED=$((SYNCED + 1))
    else
      DRIFT=$((DRIFT + 1))
      src_file="${SRC_FILES[$name]}"
      printf "  \033[31mDRIFT\033[0m  %-30s  (source: %s)\n" "$name" "$src_file"
      printf "         src:  %s\n" "$src_sig"
      printf "         test: %s\n\n" "$test_sig"
    fi
  else
    TEST_ONLY=$((TEST_ONLY + 1))
  fi
done

# ── Report ────────────────────────────────────────────────────────────────────

MATCHED=$((SYNCED + DRIFT))
echo ""
printf "  %d matched, %d synced, %d drifted\n" "$MATCHED" "$SYNCED" "$DRIFT"
if [[ $SKIPPED -gt 0 ]]; then
  printf "  %d skipped (intentional variants: %s)\n" "$SKIPPED" "$(IFS=', '; echo "${ALLOW_LIST[*]}")"
fi
if [[ $TEST_ONLY -gt 0 ]]; then
  printf "  %d test-only (no source match)\n" "$TEST_ONLY"
fi

if [[ $DRIFT -gt 0 ]]; then
  printf "\n  \033[31m✗  pkg.zmq: sync-check FAIL (%d drifted)\033[0m\n\n" "$DRIFT"
  exit 1
else
  printf "\n  \033[32m✓  pkg.zmq: sync-check PASS\033[0m\n\n"
  exit 0
fi
