#!/usr/bin/env sh
# Sert le dépôt en local pour que l'aperçu puisse lire page.html et ses relevés.
set -e
cd "$(dirname "$0")/.."
echo "→ http://127.0.0.1:8080/tools/apercu.html"
exec python3 -m http.server 8080 --bind 127.0.0.1
