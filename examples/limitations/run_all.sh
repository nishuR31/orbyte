#!/usr/bin/env bash
# Run all limitation tests against the engine and report results
ENGINE=/home/nishu/.local/bin/engine
DIR=/home/nishu/Techstack/codes/engine/examples/limitations

PASS=0
FAIL=0
REJECT=0

declare -A RESULTS
declare -A ERRORS
declare -A PASS_OUTPUT

for f in "$DIR"/[0-9]*.js "$DIR"/[0-9]*.ts; do
  base=$(basename "$f")
  out=$("$ENGINE" run "$f" 2>&1)
  code=$?
  if [ $code -eq 0 ]; then
    RESULTS["$base"]="PASS"
    PASS_OUTPUT["$base"]="$out"
    PASS=$((PASS+1))
  else
    RESULTS["$base"]="REJECT"
    ERRORS["$base"]="$out"
    REJECT=$((REJECT+1))
  fi
done

echo ""
echo "======================================================="
echo "   ENGINE LIMITATION PROBE RESULTS"
echo "======================================================="
echo ""

for key in $(echo "${!RESULTS[@]}" | tr ' ' '\n' | sort); do
  if [ "${RESULTS[$key]}" = "PASS" ]; then
    echo "✅ PASS  [$key]"
    echo "   Output: $(echo "${PASS_OUTPUT[$key]}" | head -2)"
  else
    # Extract just the first meaningful error line
    err_line=$(echo "${ERRORS[$key]}" | grep -v "^$" | head -3 | tr '\n' ' ')
    echo "🚫 REJECT [$key]"
    echo "   Error: $err_line"
  fi
  echo ""
done

echo "---"
echo "✅ Supported (passes):  $PASS"
echo "🚫 Rejected (safe err): $REJECT"
echo "Total:                  $((PASS+REJECT))"
