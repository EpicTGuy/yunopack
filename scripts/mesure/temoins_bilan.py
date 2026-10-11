"""Bilan d'une passe sur les temoins : jusqu'ou chacun va, et ce qui a change."""
import sys, pathlib
def etat(d):
    lire = lambda f: (d/f).read_text().strip() if (d/f).exists() else None
    if lire("plan.code") != "0": return "plan", lire("plan.txt") or ""
    if lire("generate.code") != "0": return "generate", lire("generate.txt") or ""
    if lire("verify.code") != "0": return "verify", lire("verify.txt") or ""
    return "ok", ""
A = pathlib.Path(sys.argv[1]); B = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else None
ok = 0
for d in sorted(p for p in A.iterdir() if p.is_dir()):
    e, txt = etat(d)
    ok += e == "ok"
    avant = etat(B/d.name)[0] if B and (B/d.name).exists() else None
    marque = "" if avant in (None, e) else f"   <<< REGRESSION (etait {avant})" if avant == "ok" else f"   (etait {avant})"
    if e != "ok" or marque:
        derniere = [l for l in txt.splitlines() if l.strip()][-3:]
        print(f"{d.name:<26} arrete a {e:<9}{marque}")
        for l in derniere: print(f"      {l[:130]}")
print(f"\n{ok}/{len([p for p in A.iterdir() if p.is_dir()])} temoins passent plan + generate + verify")
