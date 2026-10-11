#!/usr/bin/env bash
# Les depots complets et conformes (G2) d'une passe, temoins compris, au
# format de campagne.sh.
#   liste_campagne.sh <N>
source "$(dirname "$0")/commun.sh"
N="$1"; L="$MESURE/listes"
{
  for d in "$OUT/t$N"/*/; do
    n=$(basename "$d"); [ "$(cat "$d/verify.code" 2>/dev/null)" = 0 ] && grep -m1 "^$n " "$L/temoins.txt"
  done
  for j in m l; do
    grep "verify=0 " "$OUT/$j$N.verif" 2>/dev/null | awk '{print $1}' | while read -r n; do
      grep -h -m1 "^$n " "$L/echantillon.txt" "$L/lot2.txt" | head -1
    done
  done
} | sort -u
