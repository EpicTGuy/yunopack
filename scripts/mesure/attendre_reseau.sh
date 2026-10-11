#!/usr/bin/env bash
# Attend que l'hote de test telecharge a plus de 200 Ko/s : sous ce seuil, un
# test ne mesure que la connexion. Controle toutes les 5 minutes.
source "$(dirname "$0")/commun.sh"
H="${YUNOPACK_HOTE_TEST:?alias SSH requis (hote de test)}"
while true; do
  v=$(ssh "$H" 'curl -sL -o /dev/null -m 30 -w "%{speed_download}" https://github.com/seaweedfs/seaweedfs/releases/download/4.48/linux_amd64.tar.gz' 2>/dev/null | cut -d. -f1)
  [ "${v:-0}" -ge 200000 ] && { echo "reseau ok ($v o/s) $(date +%H:%M)"; exit 0; }
  echo "reseau lent (${v:-0} o/s) $(date +%H:%M)"; sleep 300
done
