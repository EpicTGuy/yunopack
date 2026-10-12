"""Constitue un corpus d'evaluation a partir du catalogue YunoHost.

Chaque paquet du catalogue dit, dans son manifeste, ou est le code amont :
c'est une verite terrain gratuite. On retient des paquets qui marchent (etat
« working », niveau >= 7), repartis entre les categories, et on ecrit le
corpus au format de `yunopack eval`.

    corpus.py <apps.toml> <sortie.toml> [nombre]
"""
import sys, json, subprocess, tomllib, collections

source, sortie = sys.argv[1], sys.argv[2]
nombre = int(sys.argv[3]) if len(sys.argv) > 3 else 120

catalogue = tomllib.load(open(source, "rb"))
candidats = sorted(
    (k, v) for k, v in catalogue.items()
    if v.get("state") == "working" and v.get("level", 0) >= 7
    and "github.com/YunoHost-Apps/" in v.get("url", "")
)
# Repartition : un tirage regulier dans chaque categorie, au prorata.
par_categorie = collections.defaultdict(list)
for k, v in candidats:
    par_categorie[v.get("category", "?")].append((k, v))
retenus = []
for cat, apps in sorted(par_categorie.items()):
    part = max(1, round(nombre * len(apps) / len(candidats)))
    pas = max(1, len(apps) // part)
    retenus += apps[::pas][:part]


def manifeste(depot, branche):
    r = subprocess.run(
        ["gh", "api", f"repos/{depot}/contents/manifest.toml?ref={branche}", "--jq", ".content"],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        return None
    import base64
    try:
        return tomllib.loads(base64.b64decode(r.stdout).decode())
    except Exception:
        return None


lignes = [
    "# Corpus tire du catalogue YunoHost (paquets « working », niveau >= 7).",
    "# Genere par scripts/mesure/corpus.py ; l'amont vient du manifeste de chaque paquet.",
    "",
]
n = 0
for k, v in retenus:
    depot = v["url"].split("github.com/", 1)[1].rstrip("/")
    m = manifeste(depot, v.get("branch", "master")) or manifeste(depot, "main")
    code = (m or {}).get("upstream", {}).get("code")
    if not code or "://" not in code:
        continue
    lignes += [
        "[[app]]",
        f'nom = "{k}"',
        f'amont = "{code.rstrip("/")}"',
        f'paquet = "{v["url"].rstrip("/")}"',
        f'difficulte = "{v.get("category", "?")}"',
        "",
    ]
    n += 1
open(sortie, "w").write("\n".join(lignes))
print(f"{n} applications ecrites dans {sortie}")
