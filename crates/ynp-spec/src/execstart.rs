//! Traduction d'une commande de demarrage Docker en `ExecStart` systemd.
//!
//! Docker lance l'application depuis une arborescence qu'il maitrise ; YunoHost
//! la lance depuis `$install_dir`, avec des runtimes provisionnes a des chemins
//! qu'il expose par des variables. La traduction consiste donc surtout a
//! replacer les chemins.
//!
//! Les placeholders `__MAJUSCULES__` sont remplaces par les helpers au moment
//! ou la configuration est installee : c'est le mecanisme natif de YunoHost.

use ynp_core::facts::{CommandeDeclaree, RepoFacts, Technology};
use ynp_core::known::Known;

/// Repertoires ou une image Docker range son application.
const RACINES: &[&str] = &[
    "/usr/local/bin/",
    "/usr/bin/",
    "/app/",
    "/srv/",
    "/opt/",
    "/home/app/",
    "/usr/src/app/",
];

pub fn derive(facts: &RepoFacts, app_id: &str) -> Known<String> {
    // Un binaire nu publie par l'amont est renomme d'apres l'app : son chemin
    // est connu sans qu'il faille interpreter quoi que ce soit.
    if let Some(sel) = &facts.selection {
        if sel.prebuilt.iter().any(|a| !a.extract) {
            return Known::resolved(format!("__INSTALL_DIR__/{app_id}"));
        }
    }

    // Sans Dockerfile, le projet peut tout de meme avoir enonce sa commande :
    // un Procfile, un point d'entree declare. C'est une declaration, pas une
    // deduction, et elle vaut donc autant qu'un CMD.
    let declaree = facts
        .stack
        .commande_declaree
        .as_ref()
        .and_then(|d| depuis_declaration(d, facts.stack.primary));

    let Some(build) = &facts.build else {
        return match declaree {
            Some(execstart) => Known::resolved(execstart),
            None => Known::unresolved(
                "aucun Dockerfile ni commande de demarrage declaree par le projet",
                &[
                    "README.md",
                    "Procfile",
                    "la documentation du projet d'origine",
                ],
            ),
        };
    };
    let Some(commande) = build.start_command() else {
        return Known::unresolved(
            "le Dockerfile ne declare ni CMD ni ENTRYPOINT",
            &[&build.dockerfile_path, "README.md", "Procfile"],
        );
    };

    match traduire(&commande, facts.stack.primary).or(declaree) {
        Some(execstart) => Known::resolved(execstart),
        None => Known::unresolved(
            format!("commande « {commande} » non transposable automatiquement"),
            &[
                &build.dockerfile_path,
                "la documentation du projet d'origine",
            ],
        ),
    }
}

/// Ce qu'une declaration du projet donne comme `ExecStart`.
///
/// Un Procfile enonce une vraie ligne de commande : elle passe par la meme
/// traduction que celle d'un Dockerfile. Un point d'entree Python n'est qu'un
/// nom : il designe un executable de l'environnement virtuel.
fn depuis_declaration(d: &CommandeDeclaree, tech: Technology) -> Option<String> {
    if d.ou.starts_with("pyproject.toml") {
        return Some(format!("__INSTALL_DIR__/venv/bin/{}", d.commande));
    }
    traduire(&d.commande, tech)
}

fn traduire(commande: &str, tech: Technology) -> Option<String> {
    let mots: Vec<&str> = commande.split_whitespace().collect();
    let premier = *mots.first()?;

    // Un script d'entree enveloppe le vrai demarrage : on ne sait pas ce qu'il
    // fait, et le deviner serait exactement ce que le projet refuse.
    if premier.ends_with(".sh") || premier.contains("entrypoint") || premier == "/bin/sh" {
        return None;
    }

    let reste = |depuis: usize| -> String {
        mots[depuis..]
            .iter()
            .map(|m| chemin(m))
            .collect::<Vec<_>>()
            .join(" ")
    };

    match premier {
        // L'interprete vient de la resource du manifest, pas du systeme.
        "node" => Some(format!("__NODEJS_DIR__/node {}", reste(1))),
        "npm" | "yarn" | "pnpm" => None, // demarrer par un gestionnaire de paquets est deconseille
        "ruby" | "bundle" => Some(format!("__RUBY_DIR__/{premier} {}", reste(1))),
        "php" | "php-fpm" => None, // pris en charge par la brique phpfpm
        "python" | "python3" => {
            // Pas de resource Python cote YunoHost : l'interprete vient d'un
            // environnement virtuel que le script d'installation cree.
            Some(format!("__INSTALL_DIR__/venv/bin/python {}", reste(1)))
        }
        // Les serveurs d'application Python s'installent dans ce meme
        // environnement virtuel, et s'y trouvent donc au meme endroit. Ce
        // n'est pas une supposition : c'est le script d'installation que
        // yunopack ecrit lui-meme qui les y met.
        "gunicorn" | "uvicorn" | "hypercorn" | "waitress-serve" | "daphne" | "celery"
            if tech == Technology::Python =>
        {
            Some(format!("__INSTALL_DIR__/venv/bin/{premier} {}", reste(1)))
        }
        _ => {
            let binaire = chemin(premier);
            // Un binaire hors des racines connues n'est pas localisable.
            if !binaire.starts_with("__INSTALL_DIR__") {
                if tech == Technology::Go || tech == Technology::Rust {
                    // Un executable compile nomme sans chemin se trouve dans
                    // le repertoire d'installation.
                    return Some(format!("__INSTALL_DIR__/{}", nom_de_fichier(premier)));
                }
                return None;
            }
            Some(if mots.len() > 1 {
                format!("{binaire} {}", reste(1))
            } else {
                binaire
            })
        }
    }
}

/// Replace un chemin de conteneur dans le repertoire d'installation.
fn chemin(mot: &str) -> String {
    if mot.starts_with('-') {
        return mot.to_string();
    }
    if let Some(ecoute) = adresse_d_ecoute(mot) {
        return ecoute;
    }
    for racine in RACINES {
        if let Some(reste) = mot.strip_prefix(racine) {
            return format!("__INSTALL_DIR__/{reste}");
        }
    }
    if let Some(reste) = mot.strip_prefix("./") {
        return format!("__INSTALL_DIR__/{reste}");
    }
    mot.to_string()
}

/// L'adresse d'ecoute d'un conteneur, ramenee a celle d'une app YunoHost.
///
/// Un conteneur ecoute sur toutes les interfaces et sur un port qu'il choisit.
/// Derriere le nginx de YunoHost, une app ecoute sur la boucle locale et sur
/// le port qui lui est attribue : laisser `0.0.0.0:7777` exposerait le service
/// en clair et sur le mauvais port. La reecriture est donc obligatoire, pas
/// cosmetique.
fn adresse_d_ecoute(mot: &str) -> Option<String> {
    let (hote, port) = mot.rsplit_once(':')?;
    if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match hote {
        "0.0.0.0" | "127.0.0.1" | "localhost" | "[::]" | "[::1]" | "" => {
            Some("127.0.0.1:__PORT__".to_string())
        }
        _ => None,
    }
}

fn nom_de_fichier(chemin: &str) -> &str {
    chemin.rsplit('/').next().unwrap_or(chemin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::{ArchAsset, BuildRecipe, CommandeDeclaree, SourceSelection, StackFacts};

    fn facts_avec(cmd: Option<Vec<String>>, tech: Technology) -> RepoFacts {
        RepoFacts {
            stack: StackFacts {
                primary: tech,
                ..Default::default()
            },
            build: Some(BuildRecipe {
                dockerfile_path: "Dockerfile".into(),
                cmd,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn facts_declarant(commande: &str, ou: &str, tech: Technology) -> RepoFacts {
        RepoFacts {
            stack: StackFacts {
                primary: tech,
                commande_declaree: Some(CommandeDeclaree {
                    commande: commande.into(),
                    ou: ou.into(),
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn un_procfile_dispense_de_demander_la_commande() {
        // Sans Dockerfile, l'outil demandait — en citant le Procfile dans les
        // pistes, qu'il n'ouvrait pas.
        let f = facts_declarant(
            "gunicorn app:server -b 0.0.0.0:8000",
            "Procfile",
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/gunicorn app:server -b 127.0.0.1:__PORT__")
        );
    }

    #[test]
    fn un_point_d_entree_python_designe_un_executable_du_venv() {
        let f = facts_declarant(
            "mon-serveur",
            "pyproject.toml, [project.scripts]",
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/mon-serveur")
        );
    }

    #[test]
    fn sans_dockerfile_ni_declaration_l_outil_demande() {
        let f = RepoFacts::default();
        let r = derive(&f, "x");
        assert!(r.value().is_none());
        assert!(r
            .reason()
            .unwrap()
            .unknown
            .contains("ni commande de demarrage declaree"));
    }

    #[test]
    fn un_serveur_python_est_cherche_dans_le_venv() {
        // Cas de SimpleLogin. gunicorn n'est pas sur le PATH du systeme : il
        // est installe dans l'environnement virtuel, au meme titre que python.
        let f = facts_avec(
            Some(vec![
                "gunicorn".into(),
                "wsgi:app".into(),
                "-b".into(),
                "0.0.0.0:7777".into(),
                "-w".into(),
                "2".into(),
            ]),
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "simplelogin").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/gunicorn wsgi:app -b 127.0.0.1:__PORT__ -w 2")
        );
    }

    #[test]
    fn le_venv_ne_vaut_que_pour_python() {
        // L'environnement virtuel n'existe que pour une application Python :
        // y chercher un binaire d'un autre monde serait une supposition.
        let f = facts_avec(Some(vec!["gunicorn".into()]), Technology::Go);
        let r = derive(&f, "x");
        assert!(!r.value().is_some_and(|v| v.contains("venv")), "{r:?}");
    }

    #[test]
    fn l_adresse_du_conteneur_devient_celle_de_l_app() {
        for a in ["0.0.0.0:8080", "127.0.0.1:3000", "[::]:9000", ":5000"] {
            assert_eq!(
                adresse_d_ecoute(a).as_deref(),
                Some("127.0.0.1:__PORT__"),
                "{a}"
            );
        }
    }

    #[test]
    fn ce_qui_n_est_pas_une_adresse_est_laisse_tel_quel() {
        // Un module Python, une image, un argument avec deux-points : rien de
        // tout cela n'est une adresse d'ecoute.
        for a in ["wsgi:app", "app.main:app", "node:20", "--timeout:15"] {
            assert_eq!(adresse_d_ecoute(a), None, "{a}");
        }
    }

    #[test]
    fn un_binaire_publie_est_localise_sans_interpretation() {
        // Cas de miniflux et gotify : l'amont publie un executable, renomme
        // d'apres l'app a l'installation.
        let mut f = facts_avec(None, Technology::Go);
        f.selection = Some(SourceSelection {
            prebuilt: vec![ArchAsset {
                arch: "amd64".into(),
                extract: false,
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(
            derive(&f, "miniflux").value().unwrap(),
            "__INSTALL_DIR__/miniflux"
        );
    }

    #[test]
    fn un_demarrage_node_passe_par_la_resource_du_manifest() {
        let f = facts_avec(
            Some(vec!["node".into(), "./dist/main.js".into()]),
            Technology::NodeJs,
        );
        assert_eq!(
            derive(&f, "app").value().unwrap(),
            "__NODEJS_DIR__/node __INSTALL_DIR__/dist/main.js"
        );
    }

    #[test]
    fn un_chemin_de_conteneur_est_replace_dans_le_repertoire_d_installation() {
        let f = facts_avec(
            Some(vec!["/usr/local/bin/buzz-relay".into()]),
            Technology::Rust,
        );
        assert_eq!(
            derive(&f, "buzz").value().unwrap(),
            "__INSTALL_DIR__/buzz-relay"
        );
    }

    #[test]
    fn un_script_d_entree_n_est_pas_devine() {
        // On ne sait pas ce que fait entrypoint.sh, et l'inventer serait
        // exactement ce que le projet refuse.
        let f = facts_avec(
            Some(vec!["/usr/local/memos/entrypoint.sh".into()]),
            Technology::Go,
        );
        let r = derive(&f, "memos");
        assert!(!r.is_resolved());
        assert!(r.reason().unwrap().unknown.contains("non transposable"));
    }

    #[test]
    fn l_absence_de_cmd_est_signalee_avec_ou_chercher() {
        let f = facts_avec(None, Technology::NodeJs);
        let r = derive(&f, "app");
        assert!(!r.is_resolved());
        assert!(r.reason().unwrap().look_in.iter().any(|s| s == "Procfile"));
    }

    #[test]
    fn un_executable_compile_sans_chemin_est_cherche_dans_l_installation() {
        let f = facts_avec(
            Some(vec!["gotify-app".into(), "serve".into()]),
            Technology::Go,
        );
        assert_eq!(
            derive(&f, "gotify").value().unwrap(),
            "__INSTALL_DIR__/gotify-app"
        );
    }

    #[test]
    fn php_fpm_n_est_pas_un_service_systemd() {
        // La brique phpfpm de YunoHost s'en charge.
        let f = facts_avec(Some(vec!["php-fpm".into()]), Technology::Php);
        assert!(!derive(&f, "app").is_resolved());
    }
}
