//! Services d'infrastructure dont l'application a besoin.
//!
//! Le compose est la source la plus sure, mais toutes les applications n'en
//! ont pas. On recoupe donc avec l'adresse de base declaree en configuration et
//! avec les pilotes presents dans les dependances : trois indices independants
//! qui se confirment ou se completent.

use ynp_core::facts::{ComposeFacts, ConfigFacts, ConfigRole, Database, ServiceFacts, StackFacts};
use ynp_core::tree::RepoTree;

pub fn detect(
    tree: &RepoTree,
    compose: Option<&ComposeFacts>,
    config: &ConfigFacts,
    stack: &StackFacts,
    has_dockerfile: bool,
) -> ServiceFacts {
    let mut facts = compose
        .map(ynp_dockerfile::services_from_compose)
        .unwrap_or_default();

    // A defaut de compose, l'adresse de connexion dit quelle base est attendue.
    if facts.database == Database::None {
        if let Some((db, evidence)) = from_connection_string(config) {
            facts.database = db;
            facts.database_evidence = Some(evidence);
        }
    }

    // Dernier recours : le pilote present dans les dependances.
    if facts.database == Database::None {
        if let Some((db, evidence)) = from_drivers(tree) {
            facts.database = db;
            facts.database_evidence = Some(evidence);
        }
    }

    facts.requires_container_runtime = requires_containers(tree, compose, stack, has_dockerfile);
    facts
}

/// Base deduite du schema d'une URL de connexion.
fn from_connection_string(config: &ConfigFacts) -> Option<(Database, String)> {
    let var = config.get(ConfigRole::DatabaseUrl)?;
    let value = var.default.as_deref()?;
    let scheme = value.split("://").next()?.to_lowercase();

    let db = match scheme.as_str() {
        s if s.starts_with("postgres") => Database::PostgreSql,
        "mysql" | "mysql2" | "mariadb" => Database::MySql,
        "sqlite" | "sqlite3" | "file" => Database::Sqlite,
        s if s.starts_with("mongodb") => Database::MongoDb,
        _ => return None,
    };
    Some((db, format!("{} = {scheme}://…", var.name)))
}

/// Base deduite d'un pilote present dans les dependances du projet.
///
/// Indice plus faible que les precedents : un pilote peut n'etre qu'une option.
/// C'est pourquoi il n'intervient qu'en dernier.
fn from_drivers(tree: &RepoTree) -> Option<(Database, String)> {
    const DRIVERS: &[(&str, Database)] = &[
        ("pg", Database::PostgreSql),
        ("postgres", Database::PostgreSql),
        ("psycopg", Database::PostgreSql),
        ("asyncpg", Database::PostgreSql),
        ("pdo_pgsql", Database::PostgreSql),
        ("mysql2", Database::MySql),
        ("mysqlclient", Database::MySql),
        ("pymysql", Database::MySql),
        ("pdo_mysql", Database::MySql),
        ("mongoose", Database::MongoDb),
        ("pymongo", Database::MongoDb),
        ("better-sqlite3", Database::Sqlite),
        ("sqlite3", Database::Sqlite),
    ];

    // Un monorepo range ses dependances dans des sous-paquets : AFFiNE declare
    // @prisma/client et ioredis dans packages/backend/server/package.json, que
    // la lecture du seul package.json racine manquait entierement.
    let mut fichiers: Vec<String> = [
        "package.json",
        "composer.json",
        "requirements.txt",
        "pyproject.toml",
        "Gemfile",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    fichiers.extend(
        tree.find_by_name(|f| f == "package.json" || f == "requirements.txt")
            .into_iter()
            .filter(|p| p.contains('/')),
    );

    for file in &fichiers {
        let file = file.as_str();
        let Some(content) = tree.text(file) else {
            continue;
        };
        let lower = content.to_lowercase();
        for (driver, db) in DRIVERS {
            // Les guillemets encadrent le nom dans un JSON comme dans un TOML,
            // ce qui evite qu'un `pg` isole corresponde a « image » ou « page ».
            if lower.contains(&format!("\"{driver}\"")) || lower.contains(&format!("'{driver}'")) {
                return Some((*db, format!("{file} : pilote {driver}")));
            }
        }
    }
    from_go_modules(tree)
}

/// Pilotes Go, reconnus a leur chemin de module.
///
/// `go.mod` n'entoure pas ses dependances de guillemets : la detection
/// generique passait a cote. Constate sur miniflux, dont la base PostgreSQL
/// n'etait pas vue faute de lire `github.com/lib/pq`.
fn from_go_modules(tree: &RepoTree) -> Option<(Database, String)> {
    const GO_DRIVERS: &[(&str, Database)] = &[
        ("github.com/lib/pq", Database::PostgreSql),
        ("github.com/jackc/pgx", Database::PostgreSql),
        ("github.com/go-sql-driver/mysql", Database::MySql),
        ("go.mongodb.org/mongo-driver", Database::MongoDb),
        ("github.com/mattn/go-sqlite3", Database::Sqlite),
        ("modernc.org/sqlite", Database::Sqlite),
    ];

    let content = tree.text("go.mod")?;
    for (module, db) in GO_DRIVERS {
        if content.contains(module) {
            return Some((*db, format!("go.mod : {module}")));
        }
    }
    None
}

/// Vrai si l'application n'est distribuee que sous forme d'image.
///
/// Critere volontairement etroit : la presence d'un Dockerfile n'est pas un
/// probleme — c'est notre meilleure source d'information. Le blocage porte sur
/// les depots qui ne fournissent aucun chemin d'installation natif : ni
/// Dockerfile a transposer, ni fichiers de projet a construire.
fn requires_containers(
    tree: &RepoTree,
    compose: Option<&ComposeFacts>,
    stack: &StackFacts,
    has_dockerfile: bool,
) -> bool {
    if has_dockerfile || stack.primary != ynp_core::facts::Technology::Unknown {
        return false;
    }
    let Some(c) = compose else {
        // Pas de compose non plus : ce n'est pas un probleme de conteneur,
        // c'est une absence d'information, traitee ailleurs.
        return false;
    };
    let app_services: Vec<_> = c.services.iter().filter(|s| s.is_app).collect();
    let only_images = !app_services.is_empty() && app_services.iter().all(|s| s.image.is_some());

    only_images && !has_helm_chart(tree)
}

fn has_helm_chart(tree: &RepoTree) -> bool {
    tree.paths().iter().any(|p| p.ends_with("Chart.yaml"))
}

/// Vrai si le depot ne propose qu'un deploiement Kubernetes.
pub fn is_kubernetes_only(tree: &RepoTree, has_dockerfile: bool) -> bool {
    has_helm_chart(tree) && !has_dockerfile
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::{ConfigVar, Technology};

    fn config_with(name: &str, role: ConfigRole, value: &str) -> ConfigFacts {
        ConfigFacts {
            example_file: Some(".env.example".into()),
            moteurs_cites: Default::default(),
            variables: vec![ConfigVar {
                name: name.into(),
                role,
                default: Some(value.into()),
                secret: false,
                source: ".env.example".into(),
                facultative: false,
            }],
        }
    }

    fn node_stack() -> StackFacts {
        StackFacts {
            primary: Technology::NodeJs,
            ..Default::default()
        }
    }

    #[test]
    fn le_compose_prime_sur_les_autres_indices() {
        let yml = "services:\n  app:\n    build: .\n  db:\n    image: postgres:16\n";
        let c = ynp_dockerfile::parse_compose("c.yml", yml, None).unwrap();
        // Indice contradictoire en configuration : le compose doit l'emporter.
        let cfg = config_with("DATABASE_URL", ConfigRole::DatabaseUrl, "mysql://x/y");

        let f = detect(&RepoTree::new(), Some(&c), &cfg, &node_stack(), true);
        assert_eq!(f.database, Database::PostgreSql);
    }

    #[test]
    fn l_adresse_de_connexion_designe_la_base_en_l_absence_de_compose() {
        let cfg = config_with(
            "DATABASE_URL",
            ConfigRole::DatabaseUrl,
            "postgresql://u:p@h/db",
        );
        let f = detect(&RepoTree::new(), None, &cfg, &node_stack(), true);

        assert_eq!(f.database, Database::PostgreSql);
        assert!(f
            .database_evidence
            .as_deref()
            .unwrap()
            .contains("DATABASE_URL"));
    }

    #[test]
    fn sqlite_est_reconnu_et_ne_demande_aucun_provisionnement() {
        let cfg = config_with(
            "DATABASE_URL",
            ConfigRole::DatabaseUrl,
            "sqlite:///data/app.db",
        );
        let f = detect(&RepoTree::new(), None, &cfg, &node_stack(), true);

        assert_eq!(f.database, Database::Sqlite);
        assert_eq!(
            f.database.manifest_type(),
            None,
            "rien a provisionner cote coeur"
        );
    }

    #[test]
    fn le_pilote_des_dependances_sert_de_dernier_recours() {
        let tree = RepoTree::from_pairs([(
            "package.json",
            r#"{"dependencies":{"express":"^4","pg":"^8.11"}}"#,
        )]);
        let f = detect(&tree, None, &ConfigFacts::default(), &node_stack(), true);

        assert_eq!(f.database, Database::PostgreSql);
        assert!(f
            .database_evidence
            .as_deref()
            .unwrap()
            .contains("pilote pg"));
    }

    #[test]
    fn un_depot_sans_indice_ne_declare_aucune_base() {
        let tree = RepoTree::from_pairs([("package.json", r#"{"dependencies":{"express":"^4"}}"#)]);
        let f = detect(&tree, None, &ConfigFacts::default(), &node_stack(), true);
        assert_eq!(f.database, Database::None);
    }

    #[test]
    fn un_depot_avec_dockerfile_n_est_jamais_bloque_pour_cause_de_conteneur() {
        // La presence d'un Dockerfile est un atout, pas un probleme.
        let yml = "services:\n  app:\n    image: ghcr.io/x/y\n";
        let c = ynp_dockerfile::parse_compose("c.yml", yml, None).unwrap();
        let f = detect(
            &RepoTree::new(),
            Some(&c),
            &ConfigFacts::default(),
            &node_stack(),
            true,
        );
        assert!(!f.requires_container_runtime);
    }

    #[test]
    fn un_depot_qui_ne_distribue_qu_une_image_est_signale() {
        let yml = "services:\n  app:\n    image: ghcr.io/x/y\n    ports: [\"80:80\"]\n";
        let c = ynp_dockerfile::parse_compose("c.yml", yml, None).unwrap();
        let empty = StackFacts::default();

        let f = detect(
            &RepoTree::new(),
            Some(&c),
            &ConfigFacts::default(),
            &empty,
            false,
        );
        assert!(f.requires_container_runtime);
    }

    #[test]
    fn un_depot_uniquement_helm_est_reconnu_comme_tel() {
        let tree = RepoTree::from_pairs([("charts/app/Chart.yaml", "name: app\n")]);
        assert!(is_kubernetes_only(&tree, false));
        assert!(
            !is_kubernetes_only(&tree, true),
            "un Dockerfile ouvre un chemin natif"
        );
    }
}

#[cfg(test)]
mod pilotes_go {
    use super::*;
    use ynp_core::facts::Technology;

    #[test]
    fn un_pilote_declare_dans_go_mod_est_reconnu() {
        // Cas reel de miniflux : go.mod n'entoure pas ses dependances de
        // guillemets, la detection generique passait a cote.
        let tree = RepoTree::from_pairs([(
            "go.mod",
            "module miniflux.app/v2\n\ngo 1.26.0\n\nrequire (\n\tgithub.com/lib/pq v1.10.9\n)\n",
        )]);
        let stack = StackFacts {
            primary: Technology::Go,
            ..Default::default()
        };
        let f = detect(&tree, None, &ConfigFacts::default(), &stack, true);

        assert_eq!(f.database, Database::PostgreSql);
        assert!(f
            .database_evidence
            .as_deref()
            .unwrap()
            .contains("github.com/lib/pq"));
    }

    #[test]
    fn un_go_mod_sans_pilote_ne_declare_aucune_base() {
        let tree = RepoTree::from_pairs([("go.mod", "module x\n\ngo 1.22\n")]);
        let stack = StackFacts {
            primary: Technology::Go,
            ..Default::default()
        };
        assert_eq!(
            detect(&tree, None, &ConfigFacts::default(), &stack, true).database,
            Database::None
        );
    }
}
