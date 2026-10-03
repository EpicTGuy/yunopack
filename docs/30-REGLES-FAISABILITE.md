# Règles de faisabilité

Chaque règle porte un identifiant stable, une sévérité, une preuve et une remédiation. Le catalogue
est versionné : ajouter une règle, c'est ajouter une entrée ici **et** un test positif plus un test
négatif dans `ynp-rules`.

## Sévérités

| Sévérité | Effet sur le score | Effet sur le verdict |
|---|---|---|
| `Blocker` | −100 | Un seul suffit : `NON FAISABLE` |
| `Major` | −20 | Dégrade, ne refuse pas |
| `Minor` | −5 | Dégrade à la marge |
| `Info` | 0 | Renseigne l'`AppSpec`, n'est pas un défaut |

Score = 100 − somme des pénalités, borné à 0. Il est indicatif : c'est **l'absence de bloquant** qui
décide. Le seuil (60 par défaut) ne sépare que « faisable » de « faisable avec travail ».

## Bloquants

### `LIC001` — Licence non libre ou absente
Le catalogue YunoHost n'accepte que du logiciel libre, ou éthique au cas par cas
(`docs/yunohost/90-policy.md`). Sans identifiant SPDX exploitable, on ne peut ni remplir
`upstream.license` ni trancher sur l'admissibilité.
**Preuve** : `license.spdx_id` de la forge, ou absence de `LICENSE`.
**Remédiation** : ouvrir le fichier `LICENSE` ; s'il porte une licence libre, renseigner
`upstream.license` à la main.

> Un repli déterministe limite les faux positifs : quand la forge rend `NOASSERTION`, le fichier
> `LICENSE` est lu et comparé aux en-têtes standards (`assets/knowledge/license-headers.toml`).
> Constaté sur gotify, dont GitHub ne classe pas le `LICENSE` alors qu'il commence par
> « MIT License ». Une licence **inconnue mais présente** produit un constat *majeur*
> (« à vérifier »), pas un refus : refuser une licence libre par méconnaissance serait pire.

### `LIC002` — Licence reconnue et non libre
BSL, SSPL, Commons Clause, Creative Commons « NonCommercial », Elastic License : ces licences
restreignent l'usage commercial ou la mise à disposition en service. Elles sont reconnues à leur
en-tête et refusées sous leur nom, plutôt que comme « non identifiables ». Un cœur libre doublé
de parties commerciales (« open core ») et une licence libre modifiée restent, eux, des constats
*majeurs* `LIC001` à vérifier.
**Remédiation** : aucune, le projet n'est pas libre.

### `SRC001` — Aucune archive téléchargeable
Ni release, ni tag, ni branche par défaut : il n'y a rien à figer dans `[resources.sources]`, dont
l'`url` et le `sha256` sont obligatoires.
**Remédiation** : vérifier que le dépôt n'est pas vide et qu'il est bien public.

> Note : l'absence de release ou de tag n'est **pas** bloquante — la stratégie
> `latest_github_commit` permet de packager quand même. C'est l'objet de `SRC002`, majeur.

### `RUN001` — Le runtime exige un moteur de conteneurs
Une application YunoHost tourne en natif sur l'hôte. Une application distribuée **uniquement** sous
forme d'image, sans procédure d'installation native, sort du modèle.
**Nuance importante** : la présence d'un Dockerfile n'est pas un blocage — c'est au contraire notre
meilleure source d'information. Le blocage porte sur les applications dont le lancement suppose un
orchestrateur (services multiples interdépendants, réseau de conteneurs, `depends_on` sur des
composants non packageables).
**Remédiation** : vérifier si l'upstream documente une installation native.

### `DB002` — Base de données non supportée
`[resources.database]` ne provisionne que MySQL et PostgreSQL ; MongoDB et Redis passent par des
helpers. Elasticsearch, ClickHouse, Cassandra et consorts n'ont pas d'équivalent.
**Remédiation** : aucune dans le cadre du catalogue.

### `K8S001` — Déploiement uniquement Helm/Kubernetes
Variante de `RUN001` à une autre échelle : pas de chemin d'installation sur une machine unique.

## Majeurs

### `SRC002` — Ni release ni tag de version
L'application reste packageable avec `latest_github_commit`, mais sa version devient la date du
commit : l'administrateur n'a aucun repère pour savoir ce qu'il installe, et les propositions de
mise à jour n'ont pas de journal des modifications.
**Remédiation** : vérifier si l'amont publie ses versions ailleurs ; sinon, accepter la stratégie
par commit en connaissance de cause.

### `STACK001` — Technologie non identifiée
Ni Dockerfile exploitable, ni fichier de projet reconnu. Le packager ne peut pas déduire comment
l'application se construit — et il ne le devinera pas.
**Remédiation** : renseigner `runtime.technology`, `runtime.build_steps` et `runtime.execstart`
à la main dans `appspec.toml`.

### `PY001` — Python autre que celui de bookworm
Il existe des `[resources]` pour nodejs, ruby, go et composer — **pas pour Python**. Une application
exigeant un Python différent du 3.11 de Debian bookworm impose un venv ou un pyenv écrit à la main.
**Remédiation** : vérifier si l'application tourne réellement sur 3.11 ; sinon, prévoir du travail manuel.

### `BUILD001` — Build gourmand en mémoire
Un build de frontal JavaScript dépasse fréquemment 1,5 Go. Beaucoup d'instances YunoHost tournent
sur ARM avec 1 à 2 Go : le build échouera chez une partie des utilisateurs.
**Remédiation** : privilégier une release contenant les assets déjà construits, et renseigner
`ram.build` honnêtement.

### `ARCH001` — Binaires spécifiques à une architecture
L'upstream ne publie que de l'amd64.
**Remédiation** : `architectures = ["amd64"]` — l'application sera masquée sur les autres plateformes.

### `PORT001` — Port fixe privilégié
L'application exige le 80 ou le 443, occupés par le nginx de YunoHost qui assure le reverse-proxy.
**Remédiation** : vérifier si le port est configurable ; sinon, incompatible.

## Mineurs

### `MAINT001` — Dépôt archivé ou inactif
Dépôt archivé, ou sans commit depuis plus de vingt-quatre mois. Le niveau 8 du catalogue exige une
maintenance effective ; un paquet abandonné devient une dette pour la communauté.

## Informatifs

Ces règles ne sont pas des défauts : elles transportent vers `plan` une information déduite.

| Id | Constat | Conséquence dans l'`AppSpec` |
|---|---|---|
| `DB001` | MySQL ou PostgreSQL détecté | `resources.database` |
| `APK001` | Dépendances Alpine à traduire | `resources.apt_packages`. Passe en **mineur** si un paquet n'a pas d'équivalent connu — il faut alors enrichir `assets/knowledge/apk-to-deb.toml` |
| `NPM001` | Modules npm à compilation native | Paquets `-dev` à ajouter aux dépendances apt. Cause la plus fréquente d'un `npm ci` en échec |

## Ajouter une règle

1. Choisir un identifiant dans une famille existante (`LIC`, `SRC`, `RUN`, `DB`, `BUILD`, `ARCH`,
   `PORT`, `MAINT`, `STACK`, `APK`, `NPM`, `K8S`) ou en ouvrir une, documentée ici.
2. Implémenter `Rule` dans `ynp-rules`, en citant systématiquement une preuve.
3. Écrire un test qui déclenche la règle et un test qui ne la déclenche pas. Le second est le plus
   important : c'est lui qui attrape les faux positifs.
4. Documenter la règle ici, avec sa remédiation.

Une règle sans remédiation actionnable n'a pas sa place dans le catalogue.
