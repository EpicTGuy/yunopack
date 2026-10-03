//! Detecteurs : ce qui est dans le depot, sans interpretation.
//!
//! Regle de separation du pipeline : ce crate collecte, il ne decide pas. Un
//! detecteur qui ecrirait « puisque c'est du Node, le port est 3000 » violerait
//! cette frontiere — il constate `EXPOSE 3000`, et c'est `ynp-spec` qui en tire
//! une consequence.
//!
//! C'est ce qui rend chaque detecteur testable sur une arborescence fabriquee,
//! sans reseau, et donc parallelisable entre agents.

pub mod config;
pub mod health;
pub mod knowledge;
pub mod services;
pub mod stack;

use ynp_core::facts::{Release, RepoFacts, RepoMeta, SourceRef, FACTS_SCHEMA_VERSION};
use ynp_core::tree::RepoTree;

/// Ce que la forge a fourni, en amont de l'analyse du contenu.
///
/// Sortir ces donnees du detecteur permet de tester l'analyse sans reseau :
/// `ynp-forge` remplira cette structure, les tests la fabriquent.
#[derive(Debug, Clone, Default)]
pub struct ForgeData {
    pub source: SourceRef,
    pub meta: RepoMeta,
    pub releases: Vec<Release>,
    pub tags: Vec<String>,
}

/// Assemble les faits d'un depot.
///
/// L'ordre des appels n'est pas arbitraire : le Dockerfile et le compose
/// alimentent la detection de stack, qui alimente celle des services.
pub fn analyze(forge: ForgeData, tree: &RepoTree) -> RepoFacts {
    let mut facts = RepoFacts::new(forge.source.clone());
    facts.schema_version = FACTS_SCHEMA_VERSION;
    facts.meta = forge.meta;
    facts.releases = forge.releases;
    facts.tags = forge.tags;
    facts.tree = tree.paths();
    for (indice, fichier) in tree.ecoutes() {
        match indice {
            ynp_core::ecoutes::Indice::Option(o) => {
                facts.ecoute.options.push((o.clone(), fichier.clone()))
            }
            ynp_core::ecoutes::Indice::PortEnDur(p) => {
                facts.ecoute.ports_en_dur.push((*p, fichier.clone()))
            }
        }
    }

    // `is_none_or` demanderait Rust 1.82 ; le projet en vise 1.79.
    if !facts
        .meta
        .description
        .as_deref()
        .is_some_and(|d| !d.trim().is_empty())
    {
        facts.meta.description_readme = resume_du_readme(tree);
    }

    let dockerfile = ynp_dockerfile::choose_dockerfile(tree);
    facts.build = dockerfile.as_ref().and_then(|path| {
        tree.text(path)
            .map(|content| ynp_dockerfile::parse_dockerfile(path, content))
    });

    let repo_hint = forge.source.repo.as_str();
    facts.compose = ynp_dockerfile::find_compose(&facts.tree).and_then(|path| {
        let content = tree.text(&path)?;
        ynp_dockerfile::parse_compose(&path, content, Some(repo_hint))
    });

    // La forge fait foi quand elle sait classer la licence ; sinon on lit le
    // fichier, ce qui evite de refuser a tort un depot parfaitement libre.
    // Le texte de licence est recopie tel quel dans le paquet : le linter
    // officiel exige un fichier LICENSE, et resumer une licence n'a pas de sens.
    if let Some((_, texte)) = tree.first_text(&[
        "LICENSE",
        "LICENSE.md",
        "LICENSE.txt",
        "LICENCE",
        "LICENCE.md",
        "LICENCE.txt",
        "COPYING",
        "COPYING.md",
        "LICENSE.rst",
    ]) {
        facts.meta.license_text = Some(texte.to_string());
        // La forge fait foi quand elle sait classer la licence ; sinon on
        // reconnait le fichier, ce qui evite de refuser a tort un depot libre.
        if facts.meta.license_spdx.is_none() {
            facts.meta.license_spdx = knowledge::get()
                .license_from_text(texte)
                .map(str::to_string);
        }
    }

    // GitHub rend encore des identifiants SPDX retires : on les remet dans
    // leur forme actuelle, celle qu'emploient les paquets officiels.
    if let Some(spdx) = facts.meta.license_spdx.take() {
        facts.meta.license_spdx = Some(knowledge::get().spdx_actuel(&spdx).to_string());
    }

    facts.stack = stack::detect(tree, facts.build.as_ref());

    // Une unite systemd livree par le projet, faute de Procfile ou de point
    // d'entree declare.
    if facts.stack.commande_declaree.is_none() {
        facts.stack.commande_declaree = commande_d_unite_systemd(tree, &forge.source.repo)
            .map(|(commande, ou)| ynp_core::facts::CommandeDeclaree { commande, ou });
    }
    // Une commande declaree qui lance un script (`web: bash Procfile.sh`
    // chez babybuddy) : on suit le script, comme celui d'une image, pour en
    // tirer le vrai processus et ses migrations.
    if let Some(d) = facts.stack.commande_declaree.clone() {
        if let Some(script) = premier_mot_de_script(&d.commande) {
            let aucune = indexmap::IndexMap::new();
            if let Some(e) = ligne_exec_suivant_les_copies(tree, &script, &aucune) {
                facts.stack.migrations_du_point_d_entree =
                    migrations_du_script(tree, &script, &aucune);
                facts.stack.commande_declaree = Some(ynp_core::facts::CommandeDeclaree {
                    commande: e,
                    ou: format!("{} ({})", d.ou, script),
                });
            }
        }
    }

    // Apres `detect`, qui remplace la structure entiere : le script d'entree,
    // s'il y en a un. Sa ligne `exec` nomme le vrai processus, et c'est le
    // premier motif de blocage de l'outil.
    if let Some(script) = facts
        .build
        .as_ref()
        .and_then(|b| b.start_command())
        .and_then(|c| premier_mot_de_script(&c))
    {
        // Une destination relative se lit depuis le WORKDIR : `COPY
        // apps/server/scripts apps/server/scripts` sous `WORKDIR /app` range
        // le script dans `/app/apps/server/scripts`.
        let workdir = facts
            .build
            .as_ref()
            .and_then(|b| b.workdir.clone())
            .unwrap_or_else(|| "/".into());
        let copies: indexmap::IndexMap<String, String> = facts
            .build
            .as_ref()
            .map(|b| {
                b.copies
                    .iter()
                    .map(|(vers, depuis)| {
                        let absolu = if vers.starts_with('/') {
                            vers.clone()
                        } else {
                            format!(
                                "{}/{}",
                                workdir.trim_end_matches('/'),
                                vers.trim_start_matches("./")
                            )
                        };
                        (absolu, depuis.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        facts.stack.exec_du_point_d_entree = ligne_exec_suivant_les_copies(tree, &script, &copies);
        facts.stack.migrations_du_point_d_entree = migrations_du_script(tree, &script, &copies);
        // Un script qui lance un autre script (`exec bash /entrypoint.sh`
        // chez fireshare) : on suit le second, un niveau seulement.
        if let Some(second) = facts
            .stack
            .exec_du_point_d_entree
            .as_deref()
            .and_then(premier_mot_de_script)
            .filter(|s| s.rsplit('/').next() != script.rsplit('/').next())
        {
            if let Some(e) = ligne_exec_suivant_les_copies(tree, &second, &copies) {
                facts.stack.exec_du_point_d_entree = Some(e);
                facts
                    .stack
                    .migrations_du_point_d_entree
                    .extend(migrations_du_script(tree, &second, &copies));
            }
        }
    }
    // Spring Boot lit SERVER_PORT et SPRING_DATASOURCE_URL quelle que soit
    // l'application : le reconnaitre suffit a lier le port et la base. Seuls
    // comptent le fichier de build de la racine et ceux des repertoires que
    // le Dockerfile retenu construit ou copie : Dataverse, qui n'est pas du
    // Spring Boot, en mentionne dans scripts/zipdownload, un outil annexe.
    let construits: Vec<String> = facts
        .build
        .as_ref()
        .map(|b| {
            b.copies
                .values()
                .cloned()
                .chain(b.build_steps.iter().flat_map(|e| {
                    e.split_whitespace()
                        .map(|m| m.trim_matches(|c| c == '"' || c == '\'').to_string())
                        .collect::<Vec<_>>()
                }))
                .map(|d| d.trim_start_matches("./").trim_end_matches('/').to_string())
                .filter(|d| !d.is_empty() && d != ".")
                .collect()
        })
        .unwrap_or_default();
    let spring = facts.stack.primary == ynp_core::facts::Technology::Java
        && tree.paths().iter().any(|p| {
            let (dossier, nom) = p.rsplit_once('/').unwrap_or(("", p));
            matches!(nom, "pom.xml" | "build.gradle" | "build.gradle.kts")
                && (dossier.is_empty() || construits.iter().any(|d| d == dossier || d == p))
                && tree.text(p).is_some_and(|t| {
                    t.contains("org.springframework.boot") || t.contains("spring-boot-starter")
                })
        });
    if spring {
        facts.stack.cadre_web = Some("spring-boot".into());
    }
    facts.stack.dependances_ruby = tree
        .text("Gemfile")
        .map(|t| {
            t.lines()
                .filter_map(|l| l.trim().strip_prefix("gem "))
                .filter_map(|r| {
                    let r = r.trim_start();
                    let q = r.chars().next().filter(|c| *c == '"' || *c == '\'')?;
                    r[1..].split(q).next().map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    // Rails : ActiveRecord lit `DATABASE_URL` et `rails server` lit `PORT`,
    // dans toutes les applications, sans qu'aucune ne le repete.
    let rails = facts.stack.primary == ynp_core::facts::Technology::Ruby
        && facts.stack.dependances_ruby.iter().any(|g| g == "rails")
        && tree.paths().iter().any(|p| p == "config/application.rb");
    if rails {
        facts.stack.cadre_web = Some("rails".into());
    }
    facts.stack.binaire_rust = binaire_rust(tree, &forge.source.repo);
    facts.config = config::detect_avec_build(tree, facts.compose.as_ref(), facts.build.as_ref());
    config::ajouter_les_lectures(&mut facts.config, tree, &forge.source.repo);
    facts.config.moteurs_cites = tree.moteurs_cites().clone();
    config::reconnaitre_le_port_prefixe(&mut facts.config, &forge.source.repo);
    facts.services = services::detect(
        tree,
        facts.compose.as_ref(),
        &facts.config,
        &facts.stack,
        dockerfile.is_some(),
    );

    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::{Database, Forge, Technology};

    fn forge_data(repo: &str) -> ForgeData {
        ForgeData {
            source: SourceRef {
                forge: Forge::GitHub,
                owner: "acme".into(),
                repo: repo.into(),
                url: format!("https://github.com/acme/{repo}"),
                default_branch: Some("main".into()),
                commit: None,
            },
            meta: RepoMeta {
                license_spdx: Some("AGPL-3.0".into()),
                pushed_at: Some("2026-09-01".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Une application Node avec base PostgreSQL, telle qu'on en rencontre.
    fn depot_realiste() -> RepoTree {
        RepoTree::from_pairs([
            (
                "Dockerfile",
                "FROM node:20-bookworm-slim\nWORKDIR /app\nRUN apt-get install -y libvips-dev \
                 && npm ci && npm run build\nEXPOSE 3000\nCMD [\"node\", \"dist/main.js\"]\n",
            ),
            (
                "docker-compose.yml",
                "services:\n  db:\n    image: postgres:16\n  app:\n    build: .\n    ports: [\"3000:3000\"]\n",
            ),
            (
                "package.json",
                r#"{"engines":{"node":">=20"},"scripts":{"build":"tsc"},"dependencies":{"sharp":"^0.33"}}"#,
            ),
            ("package-lock.json", "{}"),
            (".env.example", "PORT=3000\nDATABASE_URL=postgresql://u:p@db/app\nJWT_SECRET=changeme\n"),
        ])
    }

    #[test]
    fn un_depot_realiste_est_entierement_decode() {
        let f = analyze(forge_data("widget"), &depot_realiste());

        assert_eq!(f.stack.primary, Technology::NodeJs);
        assert_eq!(f.stack.runtime_version.as_deref(), Some("20"));
        assert_eq!(f.stack.native_deps, vec!["sharp"]);
        assert_eq!(f.services.database, Database::PostgreSql);

        let build = f.build.as_ref().unwrap();
        assert_eq!(build.expose, vec![3000]);
        assert_eq!(build.apt_packages, vec!["libvips-dev"]);
        assert_eq!(build.start_command().as_deref(), Some("node dist/main.js"));

        assert_eq!(f.config.variables.len(), 3);
        assert!(
            f.config
                .variables
                .iter()
                .find(|v| v.name == "JWT_SECRET")
                .unwrap()
                .secret
        );
        assert!(!f.services.requires_container_runtime);
    }

    #[test]
    fn le_nom_du_depot_aide_a_designer_le_service_applicatif() {
        let tree = RepoTree::from_pairs([(
            "docker-compose.yml",
            "services:\n  db:\n    image: postgres:16\n  widget:\n    image: ghcr.io/acme/widget\n",
        )]);
        let f = analyze(forge_data("widget"), &tree);

        let app: Vec<_> = f
            .compose
            .unwrap()
            .services
            .into_iter()
            .filter(|s| s.is_app)
            .collect();
        assert_eq!(app.len(), 1);
        assert_eq!(app[0].name, "widget");
    }

    #[test]
    fn un_depot_vide_produit_des_faits_vides_et_non_une_erreur() {
        let f = analyze(forge_data("rien"), &RepoTree::new());

        assert_eq!(f.stack.primary, Technology::Unknown);
        assert_eq!(f.services.database, Database::None);
        assert!(f.build.is_none());
        assert!(f.compose.is_none());
        assert!(f.tree.is_empty());
    }

    #[test]
    fn les_faits_font_un_aller_retour_json_sans_perte() {
        // `analyze` produit facts.json, relu par `assess` et `plan`.
        let f = analyze(forge_data("widget"), &depot_realiste());
        let text = serde_json::to_string(&f).unwrap();
        let back: RepoFacts = serde_json::from_str(&text).unwrap();
        assert_eq!(f, back);
    }
}

/// La premiere phrase de prose d'un README.
///
/// Le plan prevoyait ce repli des le depart et il n'avait jamais ete ecrit :
/// un depot sans description sur sa forge — le cas de Taiga — demandait la
/// description a l'humain sans rien proposer, alors que son README l'annonce
/// en toutes lettres.
///
/// Elle est proposee, jamais retenue d'office : un README commence souvent
/// par un bandeau, une rangee de badges ou un sommaire, et une description de
/// catalogue fausse est pire qu'un champ a remplir.
pub fn resume_du_readme(tree: &RepoTree) -> Option<String> {
    let (_, texte) = tree.first_text(&["README.md", "readme.md", "README"])?;

    let mut titres = 0;
    for ligne in texte.lines() {
        let l = ligne.trim();
        // Une description vit sous le premier titre. Passe le deuxieme, on
        // est dans les sections — « Documentation », « Installation » — et
        // leur premiere phrase ne dit pas ce qu'est le logiciel. Le README de
        // Taiga faisait ainsi proposer « Currently, we have authored three
        // main documentation hubs: » comme description de catalogue.
        if l.starts_with('#') {
            titres += 1;
            if titres > 1 {
                return None;
            }
        }
        // Titres, images, badges, HTML, citations, listes, tableaux, code :
        // tout cela habille un README sans le decrire.
        if l.is_empty()
            || l.starts_with('#')
            || l.starts_with('<')
            || l.starts_with('!')
            || l.starts_with('>')
            || l.starts_with('|')
            || l.starts_with('-')
            || l.starts_with('*')
            || l.starts_with("```")
            || l.starts_with('[')
        {
            continue;
        }
        // Une amorce de liste n'est pas une description.
        if l.ends_with(':') {
            continue;
        }
        // Une phrase, pas un fragment : on s'arrete au premier point.
        let phrase = l.split_once(". ").map(|(a, _)| a).unwrap_or(l);
        let phrase = phrase.trim_end_matches('.').trim();
        if phrase.chars().filter(char::is_ascii_alphabetic).count() < 20 {
            continue;
        }
        return Some(phrase.to_string());
    }
    None
}

#[cfg(test)]
mod readme {
    use super::*;

    #[test]
    fn le_bandeau_et_les_badges_sont_passes() {
        let arbre = RepoTree::from_pairs([(
            "README.md",
            "<p align=\"center\"><img src=\"logo.png\"></p>\n\n# Taiga\n\n\
             [![build](https://x/b.svg)](https://x)\n\n\
             Taiga is a project management platform. It is free and open source.\n",
        )]);
        assert_eq!(
            resume_du_readme(&arbre).as_deref(),
            Some("Taiga is a project management platform")
        );
    }

    #[test]
    fn un_readme_qui_ne_decrit_rien_ne_propose_rien() {
        // Cas de Taiga : un titre, des badges, puis les sections. Nulle part
        // une phrase disant ce qu'est le logiciel.
        let arbre = RepoTree::from_pairs([(
            "README.md",
            "## Taiga Backend\n\n[![badge](https://x/b.svg)](https://x)\n\n             ## Documentation\n\nCurrently, we have authored three main documentation hubs:\n",
        )]);
        assert_eq!(resume_du_readme(&arbre), None);
    }

    #[test]
    fn un_readme_sans_prose_ne_propose_rien() {
        let arbre = RepoTree::from_pairs([("README.md", "# Projet\n\n- a\n- b\n")]);
        assert_eq!(resume_du_readme(&arbre), None);
    }
}

/// Le binaire que produit un projet Rust : le premier `[[bin]]` ou le paquet
/// du Cargo.toml racine ; dans un espace de travail, le membre qui porte le
/// nom du depot (fx). Sans eux, on ne devine pas.
fn binaire_rust(tree: &RepoTree, depot: &str) -> Option<String> {
    let nom_du_paquet = |chemin: &str| -> Option<String> {
        let doc: toml::Value = toml::from_str(tree.text(chemin)?).ok()?;
        let paquet = doc.get("package")?.get("name")?.as_str()?.to_string();
        let bin = doc
            .get("bin")
            .and_then(|b| b.as_array())
            .and_then(|b| b.first())
            .and_then(|b| b.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_string);
        Some(bin.unwrap_or(paquet))
    };
    let depot = depot.to_lowercase();
    nom_du_paquet("Cargo.toml").or_else(|| {
        tree.paths()
            .into_iter()
            .filter(|p| p.ends_with("/Cargo.toml") && p.matches('/').count() == 1)
            .filter_map(|p| nom_du_paquet(&p))
            .find(|n| n.to_lowercase() == depot)
    })
}

/// La commande que lance un script d'entree, lue sur sa ligne `exec`.
///
/// Une image Docker passe souvent par un script avant de lancer son
/// application : il prepare le terrain, puis cede la place avec `exec`. Cette
/// derniere ligne nomme le vrai processus, et c'est du texte structure, pas
/// de la prose — `exec "$@"` veut dire « lance les arguments », c'est-a-dire
/// le `CMD`, et `exec gunicorn …` nomme la commande directement.
///
/// C'etait le premier motif de blocage de l'outil : sept depots sur les
/// treize qui s'arretaient sur la commande de demarrage le faisaient sur un
/// script d'entree que personne n'avait ouvert.
pub fn ligne_exec_du_point_d_entree(tree: &RepoTree, script: &str) -> Option<String> {
    ligne_exec_suivant_les_copies(tree, script, &indexmap::IndexMap::new())
}

/// Meme chose, en suivant les `COPY` du Dockerfile : le script que l'image
/// lance sous `/app/run.sh` est, dans le depot, `docker/entry.sh`.
pub fn ligne_exec_suivant_les_copies(
    tree: &RepoTree,
    script: &str,
    copies: &indexmap::IndexMap<String, String>,
) -> Option<String> {
    let texte = source_copiee(script, copies)
        .and_then(|source| tree.text(&source))
        .or_else(|| texte_du_script(tree, script))?;

    // Les commandes longues se poursuivent d'une ligne a l'autre.
    let recolle = texte.replace("\\\n", " ");

    let mut exec = None;
    let mut execs_distincts: Vec<String> = Vec::new();
    let mut derniere = None;
    for ligne in recolle.lines() {
        let l = ligne.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(reste) = l.strip_prefix("exec ") {
            // `"${@}"` est `"$@"` (Rails l'ecrit ainsi).
            let reste = reste.trim().replace("${@}", "$@");
            let reste = reste.as_str();
            // Le script qui se relance lui-meme sous un autre utilisateur
            // (`exec gosu $PUID "$BASH_SOURCE" "$@"` chez Mealie) ne designe
            // pas le processus : c'est une re-entree.
            let nom_du_script = script.rsplit('/').next().unwrap_or(script);
            let reentree = ["$0", "${0}", "BASH_SOURCE"]
                .iter()
                .any(|m| reste.contains(m))
                || (!nom_du_script.is_empty() && reste.contains(nom_du_script));
            if !reste.is_empty() && !reentree {
                exec = Some(reste.to_string());
                let renvoie_au_cmd = reste == "\"$@\"" || reste == "$@";
                if !renvoie_au_cmd && !execs_distincts.iter().any(|e| e == reste) {
                    execs_distincts.push(reste.to_string());
                }
            }
        }
        derniere = Some(l.to_string());
    }
    // Plusieurs `exec` differents : le processus depend d'une condition —
    // application ou worker chez active-pieces. Prendre le dernier serait
    // choisir au hasard.
    if execs_distincts.len() > 1 {
        return None;
    }
    if let Some(e) = exec {
        return Some(avec_les_variables_du_script(&e, &recolle));
    }

    // Sans `exec`, la derniere ligne fait foi — mais seulement si elle
    // repasse les arguments du conteneur, ou si elle lance manifestement
    // l'application. `uwsgi "$@"` termine le script de Kitchenowl ; `node
    // build/server.js` celui de Ferdium, apres ses migrations. Une derniere
    // ligne quelconque, elle, peut etre un nettoyage.
    derniere.filter(|l| l.ends_with("\"$@\"") || l.ends_with("$@") || lance_une_application(l))
}

/// Remplace les variables de la ligne par les valeurs que le script leur
/// donne : `CMD="uvicorn main:app --port 8080"` puis `exec $CMD` chez
/// endurain, `LOG_LEVEL="${DOWNTIFY_LOG_LEVEL:-info}"` chez downtify. La
/// premiere affectation fait foi : les suivantes sont souvent des ajouts
/// conditionnels (`CMD="$CMD --proxy-headers"`). `${X:-defaut}` prend le
/// defaut quand le script ne donne rien a X. Une valeur calculee (`$(...)`)
/// n'est pas evaluee : la variable reste, et la commande sera refusee plus
/// loin plutot que devinee.
fn avec_les_variables_du_script(ligne: &str, script: &str) -> String {
    use std::sync::OnceLock;
    static AFFECTATION: OnceLock<regex::Regex> = OnceLock::new();
    static REFERENCE: OnceLock<regex::Regex> = OnceLock::new();
    let affectation = AFFECTATION.get_or_init(|| {
        regex::Regex::new(
            r#"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=("([^"]*)"|'([^']*)'|(\S*))\s*$"#,
        )
        .expect("motif valide")
    });
    let reference = REFERENCE.get_or_init(|| {
        regex::Regex::new(
            r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}|\$([A-Za-z_][A-Za-z0-9_]*)",
        )
        .expect("motif valide")
    });
    let resoudre = |texte: &str, valeurs: &std::collections::HashMap<String, String>| {
        reference
            .replace_all(texte, |m: &regex::Captures| {
                let nom = m.get(1).or(m.get(3)).map_or("", |x| x.as_str());
                match (valeurs.get(nom), m.get(2)) {
                    (Some(v), _) => v.clone(),
                    (None, Some(defaut)) => defaut.as_str().to_string(),
                    (None, None) => m[0].to_string(),
                }
            })
            .into_owned()
    };
    // Chaque valeur se resout au moment de son affectation, avec ce qui est
    // connu jusque-la : `LOG_LEVEL="${LOG_LEVEL:-info}"` vaut `info`.
    let mut valeurs: std::collections::HashMap<String, String> = Default::default();
    for l in script.lines().map(str::trim) {
        if let Some(c) = affectation.captures(l) {
            let v = c
                .get(3)
                .or(c.get(4))
                .or(c.get(5))
                .map_or("", |m| m.as_str());
            if v.contains("$(") || v.contains('`') || valeurs.contains_key(&c[1]) {
                continue;
            }
            let v = resoudre(v, &valeurs);
            valeurs.insert(c[1].to_string(), v);
        }
    }
    resoudre(ligne, &valeurs)
}

/// Une commande simple qui commence par un lanceur d'application.
fn lance_une_application(ligne: &str) -> bool {
    const LANCEURS: &[&str] = &[
        "node",
        "npm",
        "pnpm",
        "yarn",
        "python",
        "python3",
        "gunicorn",
        "uvicorn",
        "daphne",
        "hypercorn",
        "uwsgi",
        "java",
        "ruby",
        "bundle",
        "puma",
        "unicorn",
        "rails",
        "php",
        "php-fpm",
        "dotnet",
    ];
    let compose = ["&&", "||", ";", "|", ">", "<", "$(", "`", "&"]
        .iter()
        .any(|m| ligne.contains(m));
    // Les affectations de tete ne comptent pas : documenso termine par
    // `HOSTNAME=0.0.0.0 node build/server/main.js`.
    let premier = ligne
        .split_whitespace()
        .find(|m| !m.contains('=') || m.starts_with('-'))
        .unwrap_or("");
    let nom = premier.rsplit('/').next().unwrap_or(premier);
    !compose && LANCEURS.contains(&nom)
}

/// Les migrations qu'un script d'entree lance avant de ceder la place.
///
/// your_spotify lance `node build/index.js --migrate` puis `exec node
/// build/index.js` ; ne reprendre que l'`exec` demarrait l'application sur
/// une base jamais migree. Seules les commandes simples sont retenues : une
/// ligne de shell composee (`&&`, `if`, redirection) ne se transpose pas en
/// l'etat.
pub fn migrations_du_script(
    tree: &RepoTree,
    script: &str,
    copies: &indexmap::IndexMap<String, String>,
) -> Vec<String> {
    use std::sync::OnceLock;
    static MOTIF: OnceLock<regex::Regex> = OnceLock::new();
    let motif = MOTIF.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(?:\bmigrate\b|--migrate\b|\bmigration:run\b|\bdb:migrate\b|\bdb:prepare\b|\bmigrate:latest\b|\balembic\s+upgrade\b|\bmigrate\s+deploy\b|\bdb\s+upgrade\b)",
        )
        .expect("motif de migration invalide")
    });
    let Some(texte) = source_copiee(script, copies)
        .and_then(|source| tree.text(&source))
        .or_else(|| texte_du_script(tree, script))
    else {
        return Vec::new();
    };
    let recolle = texte.replace("\\\n", " ");
    let mut out = Vec::new();
    for ligne in recolle.lines() {
        let l = ligne.trim();
        if l.starts_with("exec ") {
            break;
        }
        let compose = [
            "&&", "||", ";", "|", ">", "<", "$(", "`", "if ", "then", "echo ",
        ]
        .iter()
        .any(|m| l.contains(m));
        if l.is_empty() || l.starts_with('#') || compose || !motif.is_match(l) {
            continue;
        }
        out.push(l.to_string());
    }
    out
}

/// Le chemin dans le depot d'un fichier de l'image, d'apres les `COPY` : le
/// fichier lui-meme, ou le repertoire copie le plus proche qui le contient.
///
/// your_spotify copie `apps/server/scripts` et lance
/// `/app/apps/server/scripts/run/run.sh`. Faute de suivre la copie du
/// repertoire, l'outil cherchait un `run.sh` par son nom et prenait celui du
/// client, premier dans l'ordre alphabetique.
fn source_copiee(script: &str, copies: &indexmap::IndexMap<String, String>) -> Option<String> {
    if let Some(s) = copies.get(script) {
        return Some(s.clone());
    }
    copies
        .iter()
        .filter(|(vers, _)| script.starts_with(&format!("{}/", vers.trim_end_matches('/'))))
        .max_by_key(|(vers, _)| vers.len())
        .map(|(vers, depuis)| {
            let reste = &script[vers.trim_end_matches('/').len() + 1..];
            let depuis = depuis.trim_end_matches('/');
            if depuis == "." || depuis.is_empty() {
                reste.to_string()
            } else {
                format!("{depuis}/{reste}")
            }
        })
}

/// Retrouve un script dans l'arbre a partir du chemin qu'il a dans l'image.
///
/// Le Dockerfile le designe par son emplacement une fois copie — `/app/bin/
/// entrypoint.sh` — qui n'est pas celui du depot. On se rabat donc sur le nom
/// du fichier, et on prend le chemin le plus court quand il y en a plusieurs.
fn texte_du_script<'a>(tree: &'a RepoTree, script: &str) -> Option<&'a str> {
    if let Some(t) = tree.text(script.trim_start_matches("./")) {
        return Some(t);
    }
    let nom = script.rsplit('/').next()?;
    let mut trouves = tree.find_by_name(|n| n == nom);
    trouves.sort_by_key(|p| (p.matches('/').count(), p.len(), p.clone()));
    tree.text(trouves.first()?)
}

#[cfg(test)]
mod point_d_entree {
    use super::*;

    #[test]
    fn exec_avec_les_arguments_renvoie_au_cmd() {
        // Cas de zigbee2mqtt : le script fixe un repertoire de donnees, puis
        // cede la place a ce que le Dockerfile a mis en CMD.
        let arbre = RepoTree::from_pairs([(
            "docker/docker-entrypoint.sh",
            "#!/bin/sh\nset -e\n\necho \"Using $DATA\"\n\nexec \"$@\"\n",
        )]);
        assert_eq!(
            ligne_exec_du_point_d_entree(&arbre, "/app/docker-entrypoint.sh").as_deref(),
            Some("\"$@\"")
        );
    }

    #[test]
    fn une_commande_sur_plusieurs_lignes_est_recollee() {
        // Cas de Taiga : la commande s'etale sur huit lignes.
        let arbre = RepoTree::from_pairs([(
            "docker/entrypoint.sh",
            "chown -R taiga:taiga /taiga-back\n\nexec gosu taiga gunicorn taiga.wsgi:application \\\n\
             --bind 0.0.0.0:8000 \\\n    --workers 3\n",
        )]);
        let c = ligne_exec_du_point_d_entree(&arbre, "./docker/entrypoint.sh").unwrap();
        assert!(
            c.starts_with("gosu taiga gunicorn taiga.wsgi:application"),
            "{c}"
        );
        assert!(c.contains("--bind 0.0.0.0:8000"), "{c}");
    }

    #[test]
    fn un_script_absent_de_l_arbre_ne_conclut_rien() {
        let arbre = RepoTree::from_pairs([("README.md", "x")]);
        assert_eq!(ligne_exec_du_point_d_entree(&arbre, "entrypoint.sh"), None);
    }
}

/// Le script shell qu'une commande lance, une fois ecartes ce qui l'enveloppe.
///
/// C'est le seul cas ou il vaut la peine d'aller ouvrir un fichier : une
/// commande qui commence par un binaire se traduit deja. Le script n'est pas
/// toujours en tete — `sh /app/scripts/run.sh`, `tini -- /bin/dash
/// /entrypoint.sh` — et ce qui le precede ne fait que le lancer.
pub fn premier_mot_de_script(commande: &str) -> Option<String> {
    const ENVELOPPES: &[&str] = &[
        "tini",
        "dumb-init",
        "gosu",
        "su-exec",
        "env",
        "exec",
        "sh",
        "bash",
        "ash",
        "dash",
        "zsh",
    ];
    for mot in commande.split_whitespace() {
        let mot = mot.trim_matches(|c| c == '"' || c == '\'');
        let nom = mot.rsplit('/').next().unwrap_or(mot);
        if mot.starts_with('-') || ENVELOPPES.contains(&nom) {
            continue;
        }
        let bas = mot.to_lowercase();
        return (bas.ends_with(".sh") || bas.contains("entrypoint")).then(|| mot.to_string());
    }
    None
}

/// La commande d'une unite systemd que le projet livre lui-meme.
///
/// Un projet qui fournit `flaresolverr.service` ou `opencast.service` enonce
/// en toutes lettres comment il tourne en demon : c'est la ligne `ExecStart`
/// que le paquet doit ecrire. Seize depots sur un echantillon de quatre-vingt-
/// six demandaient leur commande faute de Dockerfile ; trois livraient une
/// telle unite.
///
/// Deux garde-fous, tires des cas reels. Une unite dont la commande appelle
/// des variables definies ailleurs — `$JAVA_ARGS` chez Suwayomi, lu d'un
/// fichier de `/etc` — donnerait une commande cassee : on la laisse. Et un
/// depot qui livre plusieurs unites auxiliaires — chatmail en a deux, pour
/// des certificats et des politiques de courrier — ne dit pas laquelle est
/// l'application : on ne retient que celle dont le nom rappelle le depot, ou
/// l'unique unite s'il n'y en a qu'une.
pub fn commande_d_unite_systemd(tree: &RepoTree, depot: &str) -> Option<(String, String)> {
    let unites: Vec<String> = tree
        .find_by_name(|n| n.ends_with(".service"))
        .into_iter()
        .filter(|p| tree.text(p).is_some())
        .collect();

    // Le nom de l'unite, ramene a l'essentiel : `chatwoot-web.1.service`
    // donne `chatwootweb`, `gollum@.service` donne `gollum`.
    let simple = |s: &str| s.to_lowercase().replace(['-', '_', '.', '@'], "");
    let nom_depot = simple(depot);
    let tige = |p: &String| {
        let t = p
            .rsplit('/')
            .next()
            .unwrap_or(p)
            .trim_end_matches(".service");
        let t = t.split('@').next().unwrap_or(t);
        simple(t.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.'))
    };
    // Le nom du depot exact d'abord, puis suivi d'un role de serveur, puis
    // un nom voisin : crowdsec livre aussi crowdsec-hubupdate.service.
    let rang = |p: &String| -> Option<u8> {
        let t = tige(p);
        if t.is_empty() {
            None
        } else if t == nom_depot {
            Some(0)
        } else if ["web", "server", "app", "daemon", "api"]
            .iter()
            .any(|r| t == format!("{nom_depot}{r}"))
        {
            Some(1)
        } else if t.contains(&nom_depot) || nom_depot.contains(&t) {
            Some(2)
        } else {
            None
        }
    };
    let choisie = match unites
        .iter()
        .filter_map(|p| rang(p).map(|r| (r, p.len(), p)))
        .min()
    {
        Some((_, _, p)) => p.clone(),
        // Une unite unique sans rapport de nom n'est retenue que si elle n'est
        // pas un modele : `xvfb@.service`, seule unite d'asqatasun, lance un
        // serveur d'affichage auxiliaire, pas l'application.
        None if unites.len() == 1 && !unites[0].contains('@') => unites[0].clone(),
        None => return None,
    };

    let texte = tree.text(&choisie)?;
    let commande = texte
        .lines()
        .find_map(|l| l.trim().strip_prefix("ExecStart="))?
        .trim()
        // `-` en tete demande a systemd d'ignorer un echec : sans rapport ici.
        .trim_start_matches('-')
        .to_string();
    if commande.is_empty() {
        return None;
    }
    let commande = variables_de_l_unite(&commande, texte)?;
    Some((ramener_a_l_installation(&commande), choisie))
}

/// Resout les variables d'une ligne `ExecStart` comme systemd le ferait.
///
/// Chatwoot definit `PORT=3000` et `RAILS_ENV=production` dans l'unite meme ;
/// Duplicati passe `$DAEMON_OPTS`, lu d'un fichier facultatif de `/etc` et vide
/// par defaut. La variable est remplacee par sa valeur quand l'unite la donne,
/// retiree quand c'est une liste d'options (`_OPTS`, `_ARGS`...) ou qu'elle ne
/// peut venir que d'un fichier facultatif — systemd la developpe alors a vide.
/// `$PORT` reste : c'est le port, que la liaison du port remplace. Toute autre
/// variable fait renoncer : la commande dependrait d'un fichier qu'on n'a pas.
fn variables_de_l_unite(commande: &str, unite: &str) -> Option<String> {
    let mut valeurs: std::collections::HashMap<String, String> = Default::default();
    let mut fichier_facultatif = false;
    let mut fichier_obligatoire = false;
    for l in unite.lines().map(str::trim) {
        if let Some(v) = l.strip_prefix("Environment=") {
            for affectation in v.split_whitespace() {
                let a = affectation.trim_matches('"');
                if let Some((k, val)) = a.split_once('=') {
                    valeurs.insert(k.to_string(), val.trim_matches('"').to_string());
                }
            }
        }
        if l.starts_with("EnvironmentFile=-") {
            fichier_facultatif = true;
        } else if l.starts_with("EnvironmentFile=") {
            fichier_obligatoire = true;
        }
    }
    let motif = regex::Regex::new(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?").expect("motif valide");
    let mut echec = false;
    let resultat = motif.replace_all(commande, |c: &regex::Captures| {
        let nom = &c[1];
        if nom == "PORT" {
            return c[0].to_string();
        }
        if let Some(v) = valeurs.get(nom) {
            return v.clone();
        }
        let options = ["OPTS", "ARGS", "FLAGS", "OPTIONS", "PARAMS"]
            .iter()
            .any(|s| nom.ends_with(s));
        // Un fichier d'environnement obligatoire, que le depot ne fournit
        // pas, peut donner a n'importe quelle variable une valeur necessaire.
        if !fichier_obligatoire && (options || fichier_facultatif) {
            return String::new();
        }
        echec = true;
        String::new()
    });
    if echec {
        return None;
    }
    Some(resultat.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// `/opt/opencast/bin/start` devient `./bin/start` : dans une unite livree
/// par le projet, `/opt/<appli>/` designe son propre repertoire
/// d'installation, que YunoHost place ailleurs.
fn ramener_a_l_installation(commande: &str) -> String {
    commande
        .split_whitespace()
        .map(
            |mot| match mot.strip_prefix("/opt/").and_then(|r| r.split_once('/')) {
                Some((_, reste)) => format!("./{reste}"),
                None => mot.to_string(),
            },
        )
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod unite_systemd {
    use super::*;

    #[test]
    fn l_unite_du_projet_donne_sa_commande() {
        let arbre = RepoTree::from_pairs([(
            "flaresolverr.service",
            "[Service]\nWorkingDirectory=/opt/flaresolverr\nExecStart=/opt/flaresolverr/flaresolverr\n",
        )]);
        assert_eq!(
            commande_d_unite_systemd(&arbre, "FlareSolverr"),
            Some(("./flaresolverr".into(), "flaresolverr.service".into()))
        );
    }

    #[test]
    fn les_arguments_suivent_et_le_chemin_est_ramene() {
        let arbre = RepoTree::from_pairs([(
            "docs/scripts/service/opencast.service",
            "[Service]\nExecStart=/opt/opencast/bin/start-opencast server\n",
        )]);
        assert_eq!(
            commande_d_unite_systemd(&arbre, "opencast").unwrap().0,
            "./bin/start-opencast server"
        );
    }

    #[test]
    fn des_variables_definies_ailleurs_font_renoncer() {
        // Cas de Suwayomi : $JAVA_ARGS vient d'un fichier obligatoire de /etc.
        let arbre = RepoTree::from_pairs([(
            "suwayomi-server.service",
            "[Service]\nEnvironmentFile=/etc/suwayomi/server.conf\n\
             ExecStart=/usr/bin/suwayomi-server $JAVA_ARGS\n",
        )]);
        assert_eq!(commande_d_unite_systemd(&arbre, "Suwayomi-Server"), None);
    }

    #[test]
    fn un_modele_sans_rapport_avec_le_depot_n_est_pas_l_application() {
        let arbre = RepoTree::from_pairs([(
            "web-app/resources/installation/xvfb@.service",
            "[Service]\nExecStart=/usr/bin/Xvfb %I -screen 0 1280x1024x24\n",
        )]);
        assert_eq!(commande_d_unite_systemd(&arbre, "Asqatasun"), None);
    }

    #[test]
    fn les_variables_de_l_unite_se_resolvent_comme_le_fait_systemd() {
        // Chatwoot : valeurs donnees par l'unite, $PORT laisse a la liaison.
        let arbre = RepoTree::from_pairs([
            (
                "deployment/chatwoot-web.1.service",
                "[Service]\nEnvironment=\"PORT=3000\"\nEnvironment=\"RAILS_ENV=production\"\n\
                 ExecStart=/bin/bash -lc 'bin/rails server -p $PORT -e $RAILS_ENV'\n",
            ),
            (
                "deployment/chatwoot-worker.1.service",
                "[Service]\nExecStart=/bin/bash -lc 'bundle exec sidekiq'\n",
            ),
        ]);
        let (c, ou) = commande_d_unite_systemd(&arbre, "chatwoot").unwrap();
        assert_eq!(ou, "deployment/chatwoot-web.1.service");
        assert!(c.contains("-p $PORT -e production"), "{c}");
        // Duplicati : options d'un fichier facultatif, vides par defaut.
        let arbre = RepoTree::from_pairs([(
            "debian/systemd/duplicati.service",
            "[Service]\nEnvironmentFile=-/etc/default/duplicati\n\
             ExecStart=/usr/bin/duplicati-server $DAEMON_OPTS\n",
        )]);
        assert_eq!(
            commande_d_unite_systemd(&arbre, "duplicati").unwrap().0,
            "/usr/bin/duplicati-server"
        );
        // Crowdsec : l'unite du nom du depot, pas l'auxiliaire.
        let arbre = RepoTree::from_pairs([
            (
                "build/crowdsec-hubupdate.service",
                "[Service]\nExecStart=/usr/bin/cscli hub update\n",
            ),
            (
                "config/crowdsec.service",
                "[Service]\nExecStart=/usr/local/bin/crowdsec -c /etc/crowdsec/config.yaml\n",
            ),
        ]);
        assert_eq!(
            commande_d_unite_systemd(&arbre, "crowdsec").unwrap().1,
            "config/crowdsec.service"
        );
    }

    #[test]
    fn des_unites_auxiliaires_ne_designent_pas_l_application() {
        // Cas de chatmail : deux unites, aucune n'est l'application.
        let arbre = RepoTree::from_pairs([
            (
                "a/tls-cert-reload.service",
                "[Service]\nExecStart=/usr/bin/reload\n",
            ),
            (
                "b/mta-sts-daemon.service",
                "[Service]\nExecStart=/usr/bin/mta\n",
            ),
        ]);
        assert_eq!(commande_d_unite_systemd(&arbre, "chatmail"), None);
    }
}

#[cfg(test)]
mod script_enveloppe {
    use super::*;

    #[test]
    fn le_script_est_trouve_derriere_ce_qui_le_lance() {
        assert_eq!(
            premier_mot_de_script("sh /app/apps/server/scripts/run/run.sh").as_deref(),
            Some("/app/apps/server/scripts/run/run.sh")
        );
        assert_eq!(
            premier_mot_de_script("tini -- /bin/dash /entrypoint.sh bin/teslamate start")
                .as_deref(),
            Some("/entrypoint.sh")
        );
        // Un binaire en tete : rien a ouvrir.
        assert_eq!(premier_mot_de_script("node server.js"), None);
    }
}

#[cfg(test)]
mod copies_du_dockerfile {
    use super::*;

    #[test]
    fn le_script_d_un_repertoire_copie_est_celui_du_bon_repertoire() {
        // Cas de your_spotify : deux run.sh, celui du client et celui du
        // serveur ; l'image copie le repertoire du serveur.
        let arbre = RepoTree::from_pairs([
            (
                "apps/client/scripts/run/run.sh",
                "#!/bin/sh
exec serve -s build
",
            ),
            (
                "apps/server/scripts/run/run.sh",
                "#!/bin/sh
node build/index.js --migrate
exec node /app/apps/server/build/index.js
",
            ),
        ]);
        let mut copies = indexmap::IndexMap::new();
        copies.insert(
            "/app/apps/server/scripts".to_string(),
            "apps/server/scripts".to_string(),
        );
        assert_eq!(
            ligne_exec_suivant_les_copies(&arbre, "/app/apps/server/scripts/run/run.sh", &copies)
                .as_deref(),
            Some("node /app/apps/server/build/index.js")
        );
    }

    #[test]
    fn sans_exec_une_derniere_ligne_qui_lance_l_application_fait_foi() {
        // Cas de Ferdium : migrations, puis le serveur, sans exec.
        let arbre = RepoTree::from_pairs([(
            "docker/entrypoint.sh",
            "#!/bin/sh\nnode ace migration:run --force\nnode build/server.js\n",
        )]);
        assert_eq!(
            ligne_exec_du_point_d_entree(&arbre, "/entrypoint.sh").as_deref(),
            Some("node build/server.js")
        );
        // Une affectation de tete ne l'empeche pas : cas de documenso.
        let arbre = RepoTree::from_pairs([(
            "docker/start.sh",
            "#!/bin/sh\nnpx prisma migrate deploy\nHOSTNAME=0.0.0.0 node build/server/main.js\n",
        )]);
        assert_eq!(
            ligne_exec_du_point_d_entree(&arbre, "start.sh").as_deref(),
            Some("HOSTNAME=0.0.0.0 node build/server/main.js")
        );
        // Une derniere ligne de nettoyage ne dit rien.
        let arbre =
            RepoTree::from_pairs([("entrypoint.sh", "#!/bin/sh\nnode a.js\nrm -rf /tmp/x\n")]);
        assert_eq!(ligne_exec_du_point_d_entree(&arbre, "/entrypoint.sh"), None);
        // Un lancement en arriere-plan non plus.
        let arbre = RepoTree::from_pairs([("entrypoint.sh", "#!/bin/sh\nnode a.js &\n")]);
        assert_eq!(ligne_exec_du_point_d_entree(&arbre, "/entrypoint.sh"), None);
        // Le script que Rails genere : `exec "${@}"`, et sa migration dans un
        // `if`.
        let rails = "#!/bin/bash -e\nif [ \"${1}\" == \"./bin/thrust\" ]; then\n  \
                     MIGRATE=1 ./bin/rails db:prepare\nfi\n\nexec \"${@}\"\n";
        let arbre = RepoTree::from_pairs([("bin/docker-entrypoint", rails)]);
        assert_eq!(
            ligne_exec_du_point_d_entree(&arbre, "/rails/bin/docker-entrypoint").as_deref(),
            Some("\"$@\"")
        );
        assert_eq!(
            migrations_du_script(&arbre, "/rails/bin/docker-entrypoint", &Default::default()),
            vec!["MIGRATE=1 ./bin/rails db:prepare"]
        );
    }

    #[test]
    fn les_variables_du_script_se_resolvent_avant_l_exec() {
        // Cas d'endurain.
        let arbre = RepoTree::from_pairs([(
            "docker/start.sh",
            "#!/bin/bash\nLOG_LEVEL=\"${LOG_LEVEL:-info}\"\n\
             CMD=\"uvicorn main:app --host 0.0.0.0 --port 8080 --log-level $LOG_LEVEL\"\n\
             if [ -n \"$P\" ]; then\n    CMD=\"$CMD --proxy-headers\"\nfi\n\
             exec gosu \"$UID:$GID\" $CMD\n",
        )]);
        assert_eq!(
            ligne_exec_du_point_d_entree(&arbre, "/docker-entrypoint.d/start.sh").as_deref(),
            Some("gosu \"$UID:$GID\" uvicorn main:app --host 0.0.0.0 --port 8080 --log-level info")
        );
        // Une valeur calculee n'est pas devinee.
        let arbre =
            RepoTree::from_pairs([("run.sh", "#!/bin/sh\nBIN=$(which app)\nexec $BIN serve\n")]);
        assert_eq!(
            ligne_exec_du_point_d_entree(&arbre, "run.sh").as_deref(),
            Some("$BIN serve")
        );
    }

    #[test]
    fn la_reentree_du_script_n_est_pas_un_second_processus() {
        // Cas de Mealie.
        let arbre = RepoTree::from_pairs([(
            "docker/entry.sh",
            "#!/bin/bash\nPUID=${PUID:-911}\nif [ \"$(id -u)\" = 0 ]; then\n    \
             exec gosu $PUID \"$BASH_SOURCE\" \"$@\"\nfi\nexec mealie\n",
        )]);
        let mut copies = indexmap::IndexMap::new();
        copies.insert("/app/run.sh".to_string(), "docker/entry.sh".to_string());
        assert_eq!(
            ligne_exec_suivant_les_copies(&arbre, "/app/run.sh", &copies).as_deref(),
            Some("mealie")
        );
    }

    #[test]
    fn plusieurs_exec_differents_ne_designent_pas_un_processus() {
        // Cas d'active-pieces : application ou worker selon une variable.
        let arbre = RepoTree::from_pairs([(
            "docker-entrypoint.sh",
            "#!/bin/sh\nif [ $# -gt 0 ]; then exec \"$@\"; fi\n\
             case \"$T\" in\n app) exec node app.js ;;\n worker) exec node worker.js ;;\nesac\n",
        )]);
        assert_eq!(
            ligne_exec_du_point_d_entree(&arbre, "./docker-entrypoint.sh"),
            None
        );
    }

    #[test]
    fn les_migrations_d_avant_l_exec_sont_reprises() {
        let arbre = RepoTree::from_pairs([(
            "run.sh",
            "#!/bin/sh\nset -e\nsource deprecated.sh\ncd apps/server\n\
             node /app/apps/server/build/index.js --migrate\n\
             echo \"migrate done\"\n\
             python manage.py migrate --noinput && echo ok\n\
             exec node /app/apps/server/build/index.js\n\
             node after.js migrate\n",
        )]);
        assert_eq!(
            migrations_du_script(&arbre, "run.sh", &indexmap::IndexMap::new()),
            vec!["node /app/apps/server/build/index.js --migrate".to_string()]
        );
    }

    #[test]
    fn le_script_renomme_par_un_copy_est_retrouve() {
        // Cas de Mealie : `COPY docker/entry.sh /app/run.sh`.
        let arbre =
            RepoTree::from_pairs([("docker/entry.sh", "#!/bin/bash\necho go\nexec mealie\n")]);
        let mut copies = indexmap::IndexMap::new();
        copies.insert("/app/run.sh".to_string(), "docker/entry.sh".to_string());
        assert_eq!(
            ligne_exec_suivant_les_copies(&arbre, "/app/run.sh", &copies).as_deref(),
            Some("mealie")
        );
    }
}
