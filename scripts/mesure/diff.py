"""Compare deux passes : ce qui change de valeur, pas seulement d'etat."""
import re, sys, pathlib
A, B = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
CHAMPS = ["execstart", "port_binding", "database_binding"]
def lire(d):
    s = d/"appspec.toml"
    if not s.exists(): return "refus", {}
    t = s.read_text()
    v = {}
    for c in CHAMPS:
        m = re.search(rf'^{c} = "(.*)"$', t, re.M)
        v[c] = m.group(1) if m else ("?" if re.search(rf"^\[runtime\.{c}\]\nunknown", t, re.M) else None)
    e = "complet" if not re.search(r"^\[[a-z0-9_.]+\]\nunknown = ", t, re.M) else "question"
    return e, v
for da in sorted(p for p in A.iterdir() if p.is_dir()):
    db = B/da.name
    if not db.exists(): continue
    (ea, va), (eb, vb) = lire(da), lire(db)
    diffs = [f"{c}: {va.get(c)!r} -> {vb.get(c)!r}" for c in CHAMPS if va.get(c) != vb.get(c)]
    if ea != eb or diffs:
        print(f"{da.name:<24} {ea:>8} -> {eb:<8}")
        for x in diffs: print(f"      {x[:150]}")
