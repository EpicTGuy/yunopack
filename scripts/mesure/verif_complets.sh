#!/usr/bin/env bash
# generate + verify sur chaque depot complet d'un dossier de passe : un
# « complet » du plan ne compte que s'il passe aussi G2.
#   verif_complets.sh <dossier>
source "$(dirname "$0")/commun.sh"
for d in "$1"/*/; do
  d=${d%/}; f=$d/appspec.toml; [ -f "$f" ] || continue
  python3 - "$f" <<'PY' || continue
import sys,tomllib
t=tomllib.load(open(sys.argv[1],'rb'))
def q(x):
    if isinstance(x,dict): return 'unknown' in x or any(q(v) for v in x.values())
    if isinstance(x,list): return any(q(v) for v in x)
    return False
sys.exit(1 if q(t) else 0)
PY
  "$Y" generate --out "$d" > "$d/generate.txt" 2>&1; g=$?
  "$Y" verify --out "$d" > "$d/verify.txt" 2>&1; v=$?
  b=$(grep -oE "\[(LINT|SCH|SHELL|CONF|G2)[A-Z0-9]*\]" "$d/verify.txt" | sort -u | tr '\n' ' ')
  echo "$(basename "$d") gen=$g verify=$v $b"
done
