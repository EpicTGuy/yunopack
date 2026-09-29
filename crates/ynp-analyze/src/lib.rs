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
    if let Some((_, texte)) = tree.first_text(&["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"])
    {
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

    // Apres `detect`, qui remplace la structure entiere : le script d'entree,
    // s'il y en a un. Sa ligne `exec` nomme le vrai processus, et c'est le
    // premier motif de blocage de l'outil.
    if let Some(script) = facts
        .build
        .as_ref()
        .and_then(|b| b.start_command())
        .and_then(|c| premier_mot_de_script(&c))
    {
        facts.stack.exec_du_point_d_entree = ligne_exec_du_point_d_entree(tree, &script);
    }
    facts.config = config::detect_avec_build(tree, facts.compose.as_ref(), facts.build.as_ref());
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
    let texte = texte_du_script(tree, script)?;

    // Les commandes longues se poursuivent d'une ligne a l'autre.
    let recolle = texte.replace("\\\n", " ");

    let mut exec = None;
    let mut derniere = None;
    for ligne in recolle.lines() {
        let l = ligne.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(reste) = l.strip_prefix("exec ") {
            let reste = reste.trim();
            if !reste.is_empty() {
                exec = Some(reste.to_string());
            }
        }
        derniere = Some(l.to_string());
    }
    if exec.is_some() {
        return exec;
    }

    // Sans `exec`, la derniere ligne fait foi — mais seulement si elle
    // repasse les arguments du conteneur. `uwsgi "$@"` termine le script de
    // Kitchenowl : c'est bien le processus qui tient l'application. Une
    // derniere ligne quelconque, elle, peut etre un nettoyage.
    derniere.filter(|l| l.ends_with("\"$@\"") || l.ends_with("$@"))
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

/// Le premier mot d'une commande, quand c'est un script shell.
///
/// C'est le seul cas ou il vaut la peine d'aller ouvrir un fichier : une
/// commande qui commence par un binaire se traduit deja.
fn premier_mot_de_script(commande: &str) -> Option<String> {
    let premier = commande.split_whitespace().next()?;
    let bas = premier.to_lowercase();
    (bas.ends_with(".sh") || bas.contains("entrypoint")).then(|| premier.to_string())
}
