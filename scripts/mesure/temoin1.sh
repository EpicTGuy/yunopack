#!/usr/bin/env bash
# Un temoin : plan, generate, verify. Chaque etape laisse son code.
#   temoin1.sh <nom> <url> <repertoire>
source "$(dirname "$0")/commun.sh"
nom="$1"; url="$2"; r="$3/$nom"; mkdir -p "$r"
perl -e 'alarm 240; exec @ARGV' "$Y" plan "$url" --out "$r" > "$r/plan.txt" 2>&1; echo $? > "$r/plan.code"
[ "$(cat "$r/plan.code")" = 0 ] || exit 0
"$Y" generate --out "$r" > "$r/generate.txt" 2>&1; echo $? > "$r/generate.code"
[ "$(cat "$r/generate.code")" = 0 ] || exit 0
"$Y" verify --out "$r" > "$r/verify.txt" 2>&1; echo $? > "$r/verify.code"
