#!/usr/bin/env bash
# Un depot : plan, en temps borne, resultat dans son propre repertoire.
#   plan1.sh <nom> <url> <repertoire>
source "$(dirname "$0")/commun.sh"
nom="$1"; url="$2"; r="$3/$nom"; mkdir -p "$r"
perl -e 'alarm 240; exec @ARGV' "$Y" plan "$url" --force --out "$r" > "$r/sortie.txt" 2>&1
echo $? > "$r/code"
