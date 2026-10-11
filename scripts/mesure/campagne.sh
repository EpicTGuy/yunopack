#!/usr/bin/env bash
# Campagne G3 reelle : chaque depot de la liste est planifie, genere, verifie
# puis teste sur l'hote de test, sans retouche. Un verdict par ligne dans
# bilan.txt. Un echec du au reseau (registre injoignable, delai depasse)
# n'est pas un echec du paquet : il est classe RESEAU et rejoue une fois.
#   campagne.sh <liste> <repertoire>
#   liste : lignes « nom url » (voir liste_campagne.sh)
set -u
source "$(dirname "$0")/commun.sh"
H="${YUNOPACK_HOTE_TEST:?alias SSH requis (hote de test)}"
L="$1"; R="$2"; mkdir -p "$R"
# Un binaire fige : recompiler pendant la campagne ne la fausse pas.
YC="$R/yunopack"; cp "$Y" "$YC"
: > "$R/bilan.txt"; : > "$R/a-rejouer.txt"
RESEAU='reseau :|Broken pipe|client_loop|assign requested address|Connection reset by peer|ssh: connect to host|ETIMEDOUT|ECONNRESET|EAI_AGAIN|ENOTFOUND|fetch failed|Connect Timeout|Could not resolve host|Temporary failure in name resolution|Connection timed out|TLS handshake timeout|Read timed out|Failed to connect to'

un_depot() {
  local nom="$1" url="$2" d="$R/$1"
  rm -rf "$d"; mkdir -p "$d"
  perl -e 'alarm 300; exec @ARGV' "$YC" plan "$url" --out "$d" > "$d/plan.txt" 2>&1 \
    && "$YC" generate --out "$d" > "$d/generate.txt" 2>&1 \
    && "$YC" verify --out "$d" > "$d/verify.txt" 2>&1
  if [ $? -ne 0 ]; then
    if grep -qE "$RESEAU" "$d/plan.txt"; then echo "$nom RESEAU"; else echo "$nom STATIQUE"; fi
    return
  fi
  perl -e 'alarm 5400; exec @ARGV' "$YC" -o "$d" test --host "$H" > "$d/test.txt" 2>&1
  if [ $? -eq 0 ]; then echo "$nom VALIDE"; return; fi
  # Les journaux des dernieres operations sur l'hote.
  ssh "$H" "f=\$(ls -t /var/log/yunohost/operations/*.log | head -3); cat \$f" > "$d/journal.txt" 2>/dev/null
  if grep -qE "$RESEAU" "$d/test.txt" "$d/journal.txt"; then echo "$nom RESEAU"; return; fi
  local etape cause
  etape=$(grep -m1 "ECHEC" "$d/test.txt" | sed -E 's/ +ECHEC.*//; s/^ +//')
  cause=$(grep -m1 "CAUSE\|rien n'ecoute\|404 sur" "$d/test.txt" | sed 's/^ *//' | cut -c1-160)
  echo "$nom ECHEC [$etape] $cause"
}

while read -r nom url; do
  [ -z "$nom" ] && continue
  v=$(un_depot "$nom" "$url" < /dev/null); echo "$v" | tee -a "$R/bilan.txt"
  case "$v" in *" RESEAU") echo "$nom $url" >> "$R/a-rejouer.txt";; esac
done < "$L"
while read -r nom url; do
  [ -z "$nom" ] && continue
  echo "$(un_depot "$nom" "$url" < /dev/null) (rejoue)" | tee -a "$R/bilan.txt"
done < "$R/a-rejouer.txt"
echo FIN >> "$R/bilan.txt"
