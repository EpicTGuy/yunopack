# Mesure

Ce que yunopack mene seul jusqu'au bout, sur des depots reels.

- `listes/temoins.txt` : depots deja menes au bout, filet anti-regression.
- `listes/echantillon.txt` : un depot sur six du corpus de la liste de souhaits.
- `listes/lot2.txt` : 120 autres depots de la liste de souhaits.

```sh
cargo build --release
scripts/mesure/passe.sh 1          # passe statique : plan, generate, verify
scripts/mesure/passe.sh 2 1        # ... et differences avec la passe 1
scripts/mesure/liste_campagne.sh 2 > target/mesure/campagne.txt
YUNOPACK_HOTE_TEST=<alias ssh> scripts/mesure/attendre_reseau.sh
YUNOPACK_HOTE_TEST=<alias ssh> scripts/mesure/campagne.sh target/mesure/campagne.txt target/mesure/camp1
```

Un « complet » ne compte que s'il passe aussi `generate` et `verify` ; la vraie
mesure est la campagne : installation, service, mise a jour, sauvegarde,
restauration et retrait sans reste sur un hote YunoHost reel. Les resultats
vont sous `target/mesure` (`YUNOPACK_MESURE` pour changer).
