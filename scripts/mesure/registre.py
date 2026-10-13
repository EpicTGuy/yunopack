"""Registre des depots : chacun valide, ou explique.

Assemble la derniere passe statique (plan des listes), les verdicts G3 de
toutes les campagnes (la plus recente l'emporte) et les explications
etablies a la main (listes/explications.toml). Chaque depot recoit un etat,
une categorie et une cause :

  valide        G3 franchi sur l'hote de test
  incompatible  refus fonde sur une regle de YunoHost ou du catalogue
  amont         defaut du projet lui-meme, constate et documente
  a_debloquer   ce que yunopack ne sait pas encore faire : le travail a mener

    registre.py <passe> <campagne>... [--md sortie.md]
    registre.py 59 camp53 camp54 camp56
"""
import sys, re, pathlib, collections, tomllib

RACINE = pathlib.Path(__file__).resolve().parents[2]
MESURE = RACINE / "target" / "mesure"
LISTES = pathlib.Path(__file__).resolve().parent / "listes"

# Regles de refus qui traduisent une incompatibilite, et non un manque.
INCOMPATIBLES = {
    "LIC001": "licence non libre ou absente",
    "LIC002": "licence non libre",
    "RUN001": "le fonctionnement exige Docker",
    "RUN002": "langage que YunoHost ne fournit pas",
    "DB002": "base de donnees non prise en charge",
}

# Causes d'un echec G3, de la plus specifique a la plus generale. Chaque motif
# est cherche dans le compte rendu du test.
CAUSES = [
    ("reseau", r"Could not handshake|Temporary failure in name resolution|ETIMEDOUT|ECONNRESET|Broken pipe|client_loop|reseau :"),
    ("memoire", r"heap out of memory|Reached heap limit|Out of memory|Killed process|oom-kill|code 137\b|exit code 137|^\s*WARNING Killed\s*$|exited \(134\)|code 134\b|^\s*WARNING Aborted\s*$"),
    ("dependances systeme", r"Failed to build gem native extension|pkg-config .* not found|fatal error: .*\.h: No such file"),
    ("outil absent", r"Composer\\Pcre|composer: not found"),
    ("apt", r"Unable to install apt dependencies|Depends: .* but it is not going to be installed|Unable to locate package"),
    ("outil absent", r"command not found|commande introuvable|not found: [a-z]|No such file or directory.*bin/"),
    ("script absent", r"Missing script|missing script:"),
    ("fichier absent", r"ENOENT|No such file or directory|Cannot find module|Could not read package.json"),
    ("permission", r"EACCES|Permission denied|Permission non accord"),
    ("dependances du langage", r"ERESOLVE|Could not resolve dependency|composer.*requirements could not be resolved|No matching distribution|ResolutionImpossible|Your requirements could not be resolved"),
    ("compilation", r"error\[E\d+\]|gyp ERR|error: could not compile|compilation terminated|ld returned"),
    ("base de donnees", r"SQLSTATE|password authentication failed|Access denied for user|could not connect to server|ECONNREFUSED 127\.0\.0\.1:(5432|3306)"),
    ("configuration", r"must not be empty|is required|Invalid input|doit faire|not set|missing required|ImproperlyConfigured"),
    ("chemin", r"404 sur .* l'application ne sert pas le chemin"),
    ("navigateur", r"repond .*json a un navigateur"),
    ("ecoute", r"rien n'ecoute sur"),
    ("service", r"Main process exited|Failed with result"),
]


def lire(p):
    return p.read_text(errors="replace") if p.exists() else ""


def etape_en_echec(texte):
    m = re.search(r"^\s+(.+?)\s+ECHEC\b", texte, re.M)
    return m.group(1).strip() if m else "?"


def cause_g3(texte):
    # psql signale en passant qu'il ne peut entrer dans le repertoire de
    # l'utilisateur root : sans rapport avec l'echec.
    texte = "\n".join(l for l in texte.splitlines() if "could not change directory" not in l)
    for nom, motif in CAUSES:
        if re.search(motif, texte, re.M):
            return nom
    return "non classee"


def bloquants(sortie):
    bloc = sortie.split("── BLOQUANT ──", 1)
    if len(bloc) < 2:
        return []
    corps = re.split(r"\n\s*── ", bloc[1], maxsplit=1)[0]
    return re.findall(r"\[([A-Z]{2,6}\d{3})\]\s*(.*)", corps)


def statique(d):
    """Etat d'un depot d'apres son plan : complet, question ou refus."""
    sortie = lire(d / "sortie.txt") or lire(d / "plan.txt")
    spec = d / "appspec.toml"
    if not spec.exists():
        regles = bloquants(sortie)
        if regles:
            return "refus", regles
        err = re.findall(r"Erreur : (.{0,90})", sortie)
        return "erreur", [("ERR", err[0] if err else "plan sans specification")]
    inconnus = re.findall(r"^\[([a-z0-9_.]+)\]\nunknown = ", spec.read_text(), re.M)
    return ("question", inconnus) if inconnus else ("complet", [])


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    md = sys.argv[sys.argv.index("--md") + 1] if "--md" in sys.argv else None
    if md in args:
        args.remove(md)
    passe, campagnes = args[0], args[1:]

    explications = {}
    f = LISTES / "explications.toml"
    if f.exists():
        explications = tomllib.loads(f.read_text())

    depots = {}
    for prefixe in ("m", "l", "t"):
        d = MESURE / f"{prefixe}{passe}"
        if not d.is_dir():
            continue
        for r in sorted(p for p in d.iterdir() if p.is_dir()):
            depots.setdefault(r.name, r)

    g3 = {}
    for c in campagnes:
        bilan = MESURE / c / "bilan.txt"
        for ligne in lire(bilan).splitlines():
            parts = ligne.split(None, 1)
            if len(parts) < 2:
                continue
            nom, reste = parts
            texte = lire(MESURE / c / nom / "test.txt")
            # Le detail complet (journal de l'application compris) est dans
            # test.json ; le compte rendu n'en montre que le debut.
            j = MESURE / c / nom / "test.json"
            if j.exists():
                try:
                    import json
                    d = json.loads(j.read_text())
                    texte += "\n" + "\n".join(e.get("detail", "") for e in d.get("etapes", []))
                except Exception:
                    pass
            g3[nom] = (reste.split()[0], texte, c)

    lignes = []
    for nom in sorted(set(depots) | set(g3)):
        if nom in explications:
            e = explications[nom]
            lignes.append((nom, e["categorie"], e.get("etat", "explique"), e["cause"]))
            continue
        if nom in g3 and g3[nom][0] == "VALIDE":
            lignes.append((nom, "valide", "G3", g3[nom][2]))
            continue
        if nom in g3 and g3[nom][0] == "ECHEC":
            texte = g3[nom][1]
            lignes.append((nom, "a_debloquer", "G3 : " + etape_en_echec(texte), cause_g3(texte)))
            continue
        if nom not in depots:
            lignes.append((nom, "a_debloquer", g3[nom][0], "sans passe statique"))
            continue
        etat, detail = statique(depots[nom])
        if etat == "refus":
            incompat = [(r, t) for r, t in detail if r in INCOMPATIBLES]
            if incompat:
                r, t = incompat[0]
                lignes.append((nom, "incompatible", r, t.strip() or INCOMPATIBLES[r]))
            else:
                lignes.append((nom, "a_debloquer", "refus " + ",".join(r for r, _ in detail),
                               detail[0][1].strip()))
        elif etat == "question":
            lignes.append((nom, "a_debloquer", "question", ", ".join(detail)))
        elif etat == "complet":
            lignes.append((nom, "a_debloquer", "complet non teste", "a passer en G3"))
        else:
            lignes.append((nom, "a_debloquer", "erreur", detail[0][1]))

    cats = collections.Counter(l[1] for l in lignes)
    n = len(lignes)
    expliques = n - cats["a_debloquer"]
    print(f"{n} depots — " + ", ".join(f"{k} {v}" for k, v in cats.most_common()))
    print(f"expliques ou valides : {expliques}/{n} ({100 * expliques // max(n, 1)} %)")
    sous = collections.Counter((l[2].split(" : ")[0] if l[1] == "a_debloquer" else "") for l in lignes if l[1] == "a_debloquer")
    print("\nA DEBLOQUER, par etat")
    for k, v in sous.most_common():
        print(f"  {v:>3}  {k}")
    causes = collections.Counter(l[3] for l in lignes if l[1] == "a_debloquer" and l[2].startswith("G3"))
    print("\nECHECS G3, par cause")
    for k, v in causes.most_common():
        print(f"  {v:>3}  {k}")
    champs = collections.Counter()
    for l in lignes:
        if l[2] == "question":
            for c in l[3].split(", "):
                champs[c] += 1
    print("\nQUESTIONS, par champ")
    for k, v in champs.most_common():
        print(f"  {v:>3}  {k}")

    if md:
        with open(md, "w") as out:
            out.write("| depot | categorie | etat | cause |\n|---|---|---|---|\n")
            for l in lignes:
                out.write("| " + " | ".join(x.replace("|", "/") for x in l) + " |\n")


if __name__ == "__main__":
    main()
