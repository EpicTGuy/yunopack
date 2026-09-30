//! `docker-compose.yml` -> [`ComposeFacts`] et services detectes.
//!
//! Un compose est le signal le plus riche d'un depot : il donne d'un coup les
//! services, les ports, les volumes, la configuration et la base de donnees.
//! On le lit pour **en extraire l'intention**, jamais pour l'executer.
//!
//! La distinction decisive est celle entre le service applicatif — celui que le
//! depot construit (`build:`) — et les services d'infrastructure qu'il consomme
//! (`image: postgres`). Le premier devient l'application YunoHost ; les seconds
//! deviennent des resources du manifest, ou un motif de refus.

use indexmap::IndexMap;
use serde::Deserialize;
use ynp_core::facts::{ComposeFacts, ComposeService, Database, ServiceFacts};

/// Noms des fichiers compose reconnus, par ordre de preference.
pub const COMPOSE_NAMES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
    "docker/docker-compose.yml",
];

/// Retrouve le compose le plus pertinent.
///
/// Les emplacements canoniques d'abord, puis n'importe ou dans l'arborescence :
/// AFFiNE range le sien dans `.docker/selfhost/compose.yml`, et s'en tenir a la
/// racine revenait a ignorer ses dependances — PostgreSQL et Redis n'etaient
/// pas vus, et l'application paraissait n'en avoir aucune.
pub fn find_compose(tree: &[String]) -> Option<String> {
    if let Some(n) = COMPOSE_NAMES.iter().find(|n| tree.iter().any(|p| p == *n)) {
        return Some(n.to_string());
    }

    tree.iter()
        .filter(|p| {
            let f = p.rsplit('/').next().unwrap_or(p).to_lowercase();
            f.contains("compose") && (f.ends_with(".yml") || f.ends_with(".yaml"))
        })
        .max_by_key(|p| pertinence_compose(p))
        // Un compose de test ou de poste de developpement n'apprend rien sur le
        // deploiement : mieux vaut n'en retenir aucun. Constate sur gotify, dont
        // le seul compose sert a lancer un serveur OIDC pour la suite de tests.
        .filter(|p| pertinence_compose(p).0 >= 0)
        .cloned()
}

/// Un compose d'auto-hebergement decrit le deploiement reel ; celui d'un
/// conteneur de developpement decrit un poste de travail.
fn pertinence_compose(path: &str) -> (i32, i32) {
    let lower = path.to_lowercase();
    let mut score = 0;
    for (motif, poids) in [
        ("selfhost", 6),
        ("self-host", 6),
        ("production", 5),
        ("prod", 4),
        ("deploy", 3),
        ("devcontainer", -6),
        ("/dev", -4),
        ("test", -4),
        ("example", -2),
    ] {
        if lower.contains(motif) {
            score += poids;
        }
    }
    // A egalite, le moins profond ; l'ordre reste reproductible.
    (score, -(path.matches('/').count() as i32))
}

/// Analyse un fichier compose. Rend `None` si le YAML est illisible : un
/// compose invalide n'est pas une information, c'est une absence d'information.
///
/// `repo_hint` est le nom du depot, qui aide a departager le service applicatif
/// quand plusieurs candidats subsistent.
pub fn parse(path: &str, content: &str, repo_hint: Option<&str>) -> Option<ComposeFacts> {
    let raw: RawCompose = serde_yaml::from_str(content).ok()?;

    let mut services: Vec<ComposeService> = raw
        .services
        .into_iter()
        .map(|(name, svc)| ComposeService {
            is_app: svc.build.is_some(),
            image: svc.image,
            ports: svc.ports.iter().filter_map(container_port).collect(),
            environment: svc.environment.into_map(),
            volumes: svc
                .volumes
                .iter()
                .filter_map(|v| named_volume_target(v))
                .collect(),
            name,
        })
        .collect();

    mark_app_services(&mut services, repo_hint);
    Some(ComposeFacts {
        path: path.to_string(),
        services,
    })
}

/// Designe le ou les services qui constituent l'application.
///
/// Un `build:` est la preuve la plus forte : le service est construit depuis ce
/// depot. Mais les compose destines aux utilisateurs finaux referencent en
/// general une image publiee — c'est le cas des quatre compose reels testes.
/// On retombe alors sur les services qui ne sont ni une base, ni un cache, ni
/// un proxy : ce qui reste est l'application.
fn mark_app_services(services: &mut [ComposeService], repo_hint: Option<&str>) {
    if services.iter().any(|s| s.is_app) {
        return;
    }

    let candidates: Vec<usize> = services
        .iter()
        .enumerate()
        .filter(|(_, s)| match s.image.as_deref() {
            // Un service sans image est construit localement : c'est un candidat.
            None => true,
            Some(image) => classify_image(image).is_none(),
        })
        .map(|(i, _)| i)
        .collect();

    // Quand le nom du depot designe un candidat sans ambiguite, on s'y tient.
    if let Some(hint) = repo_hint.map(str::to_lowercase).filter(|h| !h.is_empty()) {
        let matching: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| {
                let s = &services[i];
                s.name.to_lowercase().contains(&hint)
                    || s.image
                        .as_deref()
                        .is_some_and(|im| im.to_lowercase().contains(&hint))
            })
            .collect();
        if !matching.is_empty() {
            for i in matching {
                services[i].is_app = true;
            }
            return;
        }
    }

    // Sinon on retient tous les candidats plutot que d'en choisir un au hasard :
    // une ambiguite affichee vaut mieux qu'une decision arbitraire.
    for i in candidates {
        services[i].is_app = true;
    }
}

/// Deduit les services d'infrastructure requis par l'application.
pub fn services_from_compose(facts: &ComposeFacts) -> ServiceFacts {
    let mut out = ServiceFacts::default();

    for svc in &facts.services {
        if svc.is_app {
            continue;
        }
        let Some(image) = svc.image.as_deref() else {
            continue;
        };
        match classify_image(image) {
            Some(Kind::Database(db)) => {
                // Un seul `[resources.database]` existe cote YunoHost. Si deux
                // bases differentes apparaissent, la seconde est un blocage.
                if out.database == Database::None {
                    out.database = db;
                    out.database_evidence = Some(format!("{} ({})", svc.name, image));
                } else if out.database != db {
                    out.unsupported
                        .push(format!("{} ({}) : seconde base", svc.name, image));
                }
            }
            Some(Kind::Redis) => out.needs_redis = true,
            Some(Kind::StockageObjet) => out.needs_s3 = true,
            Some(Kind::Unsupported) => {
                out.unsupported.push(format!("{} ({})", svc.name, image));
            }
            Some(Kind::Proxy) | Some(Kind::Tooling) | None => {}
        }
    }
    out
}

enum Kind {
    Database(Database),
    Redis,
    /// Stockage objet S3. Le paquet embarque un SeaweedFS a la place.
    StockageObjet,
    Unsupported,
    /// Reverse-proxy ou frontal : YunoHost fournit deja nginx, ces services
    /// disparaissent au packaging. Ils ne sont ni l'application, ni un blocage.
    Proxy,
    /// Outillage d'administration, d'observabilite ou de developpement.
    /// Accompagne l'application sans en faire partie.
    Tooling,
}

/// Reconnait un service d'infrastructure d'apres son image.
///
/// On compare sur le nom court de l'image, en ignorant le registre et le tag :
/// `docker.io/library/postgres:16-alpine` doit etre reconnu comme `postgres`.
fn classify_image(image: &str) -> Option<Kind> {
    let short = image.rsplit('/').next().unwrap_or(image);
    let name = short.split(':').next().unwrap_or(short).to_lowercase();
    // Certaines entrees designent l'organisation autant que l'image
    // (`minio/mc`) : comparer le seul nom court les manquerait, et `mc` seul
    // serait trop generique pour etre compare sans son prefixe.
    let complet = image.to_lowercase();

    let has = |needles: &[&str]| {
        needles
            .iter()
            .any(|n| name.contains(n) || complet.contains(n))
    };

    if has(&["postgres", "pgvector", "timescale", "pgautoupgrade"]) {
        Some(Kind::Database(Database::PostgreSql))
    } else if has(&["mariadb", "mysql", "percona"]) {
        Some(Kind::Database(Database::MySql))
    } else if has(&["mongo"]) {
        Some(Kind::Database(Database::MongoDb))
    } else if has(&["redis", "valkey", "keydb", "dragonfly"]) {
        Some(Kind::Redis)
    } else if has(&["nginx", "traefik", "caddy", "haproxy", "swag"]) {
        Some(Kind::Proxy)
    } else if has(&["minio/mc"]) {
        // Le client de minio, lance pour creer un compartiment : de
        // l'outillage, pas le stockage lui-meme.
        Some(Kind::Tooling)
    } else if has(&["minio", "seaweedfs", "dxflrs/garage", "garagehq"]) {
        Some(Kind::StockageObjet)
    } else if has(&[
        // Outils d'administration, d'observabilite et de developpement. Ils
        // accompagnent l'application sans en faire partie, et disparaissent au
        // packaging. Constate sur block/buzz, ou adminer, keycloak, prometheus
        // et minio-init etaient pris pour le service applicatif.
        "adminer",
        "pgadmin",
        "phpmyadmin",
        "mongo-express",
        "redisinsight",
        "prometheus",
        "grafana",
        "jaeger",
        "loki",
        "tempo",
        "otel",
        "cadvisor",
        "node-exporter",
        "statsd",
        "zipkin",
        "mailhog",
        "mailpit",
        "maildev",
        "smtp4dev",
        "keycloak",
        "authelia",
        "authentik",
        "dex",
        "oauth2-proxy",
        "minio/mc",
        "busybox",
        "alpine",
        "watchtower",
        "portainer",
        "dozzle",
    ]) {
        Some(Kind::Tooling)
    } else if has(&[
        "elasticsearch",
        "opensearch",
        "clickhouse",
        "cassandra",
        "typesense",
        "meilisearch",
        "solr",
        "neo4j",
        "influxdb",
        "couchdb",
        "etcd",
        "rabbitmq",
        "kafka",
        "zookeeper",
        "cockroach",
    ]) {
        // Pas d'equivalent provisionnable par le coeur de YunoHost.
        Some(Kind::Unsupported)
    } else {
        None
    }
}

/// Port interne d'un mapping `"8080:3000"` — c'est celui sur lequel
/// l'application ecoute, donc celui qui nous interesse pour le reverse-proxy.
fn container_port(spec: &PortSpec) -> Option<u16> {
    let s = match spec {
        PortSpec::Number(n) => return u16::try_from(*n).ok(),
        PortSpec::Text(s) => s.as_str(),
        PortSpec::Long { target, .. } => return u16::try_from(*target).ok(),
    };
    let without_proto = s.split('/').next().unwrap_or(s);
    // Formes possibles : "3000", "8080:3000", "127.0.0.1:8080:3000".
    without_proto.rsplit(':').next()?.trim().parse().ok()
}

/// Chemin monte dans le conteneur, qui indique ou l'application range ses
/// donnees — candidat pour `[resources.data_dir]`.
fn named_volume_target(v: &str) -> Option<String> {
    let parts: Vec<&str> = v.split(':').collect();
    match parts.len() {
        1 => Some(parts[0].to_string()),
        _ => Some(parts[1].to_string()),
    }
    .filter(|p| p.starts_with('/'))
}

// --- Representation brute du YAML ---

#[derive(Debug, Deserialize)]
struct RawCompose {
    #[serde(default)]
    services: IndexMap<String, RawService>,
}

#[derive(Debug, Deserialize)]
struct RawService {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    build: Option<serde_yaml::Value>,
    #[serde(default)]
    ports: Vec<PortSpec>,
    #[serde(default)]
    environment: Environment,
    #[serde(default)]
    volumes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PortSpec {
    Number(i64),
    Text(String),
    /// Forme longue : `{ target: 3000, published: 8080 }`.
    Long {
        target: i64,
    },
}

/// Compose accepte deux syntaxes pour `environment` : une table, ou une liste
/// de `CLE=valeur`. Les deux sont courantes, il faut lire les deux.
#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
enum Environment {
    Map(IndexMap<String, Option<String>>),
    List(Vec<String>),
    #[default]
    None,
}

impl Environment {
    fn into_map(self) -> IndexMap<String, String> {
        match self {
            Environment::Map(m) => m
                .into_iter()
                .map(|(k, v)| (k, v.unwrap_or_default()))
                .collect(),
            Environment::List(l) => l
                .iter()
                .map(|e| match e.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => (e.clone(), String::new()),
                })
                .collect(),
            Environment::None => IndexMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_port_retenu_est_celui_du_conteneur_pas_celui_de_l_hote() {
        assert_eq!(
            container_port(&PortSpec::Text("8080:3000".into())),
            Some(3000)
        );
        assert_eq!(
            container_port(&PortSpec::Text("127.0.0.1:8080:3000".into())),
            Some(3000)
        );
        assert_eq!(container_port(&PortSpec::Text("3000".into())), Some(3000));
        assert_eq!(
            container_port(&PortSpec::Text("8080:3000/tcp".into())),
            Some(3000)
        );
        assert_eq!(container_port(&PortSpec::Long { target: 9000 }), Some(9000));
    }

    #[test]
    fn le_registre_et_le_tag_n_empechent_pas_de_reconnaitre_une_image() {
        assert!(matches!(
            classify_image("docker.io/library/postgres:16-alpine"),
            Some(Kind::Database(Database::PostgreSql))
        ));
        assert!(matches!(
            classify_image("mariadb:11"),
            Some(Kind::Database(Database::MySql))
        ));
        assert!(matches!(
            classify_image("valkey/valkey:8"),
            Some(Kind::Redis)
        ));
        assert!(matches!(
            classify_image("elasticsearch:8.13"),
            Some(Kind::Unsupported)
        ));
        // nginx est un frontal, pas l'application : YunoHost fournit le sien.
        assert!(matches!(classify_image("nginx:alpine"), Some(Kind::Proxy)));
        // Une image quelconque n'est pas de l'infrastructure : c'est un
        // candidat pour etre l'application.
        assert!(classify_image("ghcr.io/linkwarden/linkwarden:v2").is_none());
    }
}
