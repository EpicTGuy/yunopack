# Reglages communs aux scripts de mesure. A sourcer.
#
# Les sorties vont sous target/ (ignore par git) : elles se regenerent, et un
# repertoire temporaire du systeme se vide de lui-meme — c'est ainsi que la
# premiere version de ces scripts a ete perdue.
#   YUNOPACK_MESURE      repertoire des resultats (defaut : target/mesure)
#   YUNOPACK_HOTE_TEST   alias SSH de l'hote YunoHost de test (campagnes reelles)
# A sourcer depuis bash : ailleurs, BASH_SOURCE est vide et la racine
# serait fausse.
if [ -z "${BASH_SOURCE[0]:-}" ]; then
  echo "commun.sh : a sourcer depuis bash" >&2; return 1 2>/dev/null || exit 1
fi
RACINE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MESURE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="${YUNOPACK_MESURE:-$RACINE/target/mesure}"
# Une passe fige son binaire (YUNOPACK_BINAIRE) : recompiler pendant qu'elle
# tourne ne la fausse pas.
Y="${YUNOPACK_BINAIRE:-$RACINE/target/release/yunopack}"
mkdir -p "$OUT"
if [ -z "${GITHUB_TOKEN:-}" ] && command -v gh >/dev/null; then
  GITHUB_TOKEN="$(gh auth token 2>/dev/null || true)"; export GITHUB_TOKEN
fi
