"""Bilan d'un lot de `yunopack plan` : qui passe, qui bloque, sur quoi."""
import sys, re, pathlib, collections
R = pathlib.Path(sys.argv[1])
etats, champs, refus = collections.Counter(), collections.Counter(), collections.Counter()
detail = collections.defaultdict(list)
for d in sorted(p for p in R.iterdir() if p.is_dir()):
    code = (d/"code").read_text().strip() if (d/"code").exists() else "?"
    sortie = (d/"sortie.txt").read_text(errors="replace") if (d/"sortie.txt").exists() else ""
    spec = d/"appspec.toml"
    if code == "142":
        etats["delai"] += 1; detail["delai"].append(d.name); continue
    if not spec.exists():
        # Refus ou erreur : le code de la regle, sinon la premiere erreur.
        codes = re.findall(r"\b([A-Z]{2,5}\d{3})\b", sortie)
        cle = codes[0] if codes else ("erreur: " + (re.findall(r"Erreur : (.{0,60})", sortie) or ["?"])[0])
        refus[cle] += 1; etats["refus"] += 1; detail[cle].append(d.name); continue
    t = spec.read_text()
    inconnus = re.findall(r"^\[([a-z0-9_.]+)\]\nunknown = ", t, re.M)
    if not inconnus: etats["complet"] += 1; detail["complet"].append(d.name)
    else:
        etats["questions"] += 1
        for c in inconnus: champs[c] += 1; detail[c].append(d.name)
n = sum(etats.values())
print(f"{n} depots — " + ", ".join(f"{k} {v}" for k, v in etats.most_common()))
print("\nCHAMPS QUI BLOQUENT (depots touches)")
for c, k in champs.most_common(): print(f"  {k:>3}  {c}")
print("\nREFUS ET ERREURS")
for c, k in refus.most_common(): print(f"  {k:>3}  {c}")
if len(sys.argv) > 2:
    print(f"\n{sys.argv[2]} :", " ".join(detail.get(sys.argv[2], [])))
