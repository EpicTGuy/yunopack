#!/usr/bin/env bash
# Passe statique complete : temoins (plan + generate + verify), echantillon et
# lot de la liste de souhaits (plan, puis generate + verify des complets),
# avec rejeu des depots touches par une coupure reseau, puis bilan et
# differences de valeurs avec la passe precedente.
#   passe.sh <N> [<precedente>]
set -u
source "$(dirname "$0")/commun.sh"
N="$1"; P="${2:-}"
L="$MESURE/listes"
lancer() { # <script> <liste> <dossier> <parallelisme>
  xargs -P "$4" -L 1 sh -c '"$0" "$1" "$2" "'"$3"'"' "$1" < "$2" 2>/dev/null
}
rejouer() { # <script> <liste> <dossier>
  for _ in 1 2 3; do
    : > "$3.rejouer"
    while read -r nom url; do
      [ -z "$nom" ] && continue
      f="$3/$nom/sortie.txt"; [ -f "$f" ] || f="$3/$nom/plan.txt"
      c="$3/$nom/code"; [ -f "$c" ] || c="$3/$nom/plan.code"
      if [ ! -f "$f" ] || [ "$(cat "$c" 2>/dev/null)" = 142 ] \
        || grep -qE "reseau :|Alarm|timed out|error sending|error decoding|quota d'API" "$f" 2>/dev/null; then
        echo "$nom $url" >> "$3.rejouer"
      fi
    done < "$2"
    [ -s "$3.rejouer" ] || break
    sleep 20; lancer "$1" "$3.rejouer" "$3" 3
  done
}
for j in t m l; do rm -rf "$OUT/$j$N"; mkdir -p "$OUT/$j$N"; done
# Le binaire de la passe, fige au depart.
cp "$Y" "$OUT/yunopack-p$N"; export YUNOPACK_BINAIRE="$OUT/yunopack-p$N"
lancer "$MESURE/temoin1.sh" "$L/temoins.txt" "$OUT/t$N" 6;  rejouer "$MESURE/temoin1.sh" "$L/temoins.txt" "$OUT/t$N"
lancer "$MESURE/plan1.sh" "$L/echantillon.txt" "$OUT/m$N" 6; rejouer "$MESURE/plan1.sh" "$L/echantillon.txt" "$OUT/m$N"
lancer "$MESURE/plan1.sh" "$L/lot2.txt" "$OUT/l$N" 6;        rejouer "$MESURE/plan1.sh" "$L/lot2.txt" "$OUT/l$N"
echo "######## TEMOINS"
if [ -n "$P" ] && [ -d "$OUT/t$P" ]; then python3 "$MESURE/temoins_bilan.py" "$OUT/t$N" "$OUT/t$P" | tail -3
else python3 "$MESURE/temoins_bilan.py" "$OUT/t$N" | tail -3; fi
for j in m l; do
  echo "######## $([ $j = m ] && echo ECHANTILLON || echo LOT2)"; python3 "$MESURE/mesure.py" "$OUT/$j$N" | head -2
  [ -n "$P" ] && [ -d "$OUT/$j$P" ] && python3 "$MESURE/diff.py" "$OUT/$j$P" "$OUT/$j$N"
done
echo "######## GARDE-FOUS"
grep -lE '^(execstart|port_binding|database_binding) = ".*(0\.0\.0\.0|\$)' "$OUT"/[tml]$N/*/appspec.toml || echo "aucune valeur dangereuse"
echo "######## COMPLETS CONFORMES (generate + verify)"
for j in m l; do
  "$MESURE/verif_complets.sh" "$OUT/$j$N" > "$OUT/$j$N.verif" 2>&1
  ok=$(grep -c "verify=0 " "$OUT/$j$N.verif"); tot=$(grep -c "gen=" "$OUT/$j$N.verif")
  echo "$([ $j = m ] && echo echantillon || echo lot2) : $ok conformes sur $tot complets"
  grep -v "verify=0 " "$OUT/$j$N.verif" | sed 's/^/  bloque : /'
done
