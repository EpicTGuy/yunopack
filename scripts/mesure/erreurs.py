"""Lignes decisives de l'etape en echec d'un test G3.

    erreurs.py <repertoire du test>...
"""
import json, sys, re

MOTS = re.compile(r"rror|ERR!|No such|not found|failed|Cannot|FATAL|Traceback|exited|denied", re.I)
for rep in sys.argv[1:]:
    try:
        d = json.load(open(f"{rep}/test.json"))
    except Exception:
        continue
    for e in d.get("etapes", []):
        if not e.get("reussie", True):
            lignes = [l[:240] for l in e.get("detail", "").splitlines() if MOTS.search(l)]
            lignes = [l for l in lignes if "Here's an extract" not in l and "yunohost log share" not in l]
            print(f"######## {rep} — {e['nom']}")
            print("\n".join(lignes[-14:]))
            break
