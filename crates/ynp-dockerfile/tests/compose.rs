//! Le compose est lu pour en extraire l'intention, jamais pour etre execute.

use ynp_core::facts::Database;
use ynp_dockerfile::{find_compose, parse_compose, services_from_compose};

const LINKWARDEN: &str = r#"
services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_PASSWORD: secret
    volumes:
      - pgdata:/var/lib/postgresql/data
  linkwarden:
    build: .
    environment:
      - DATABASE_URL=postgresql://postgres:secret@postgres:5432/postgres
      - NEXTAUTH_URL=http://localhost:3000/api/v1/auth
    ports:
      - 3000:3000
    volumes:
      - ./data:/data/data
volumes:
  pgdata:
"#;

#[test]
fn le_service_applicatif_se_distingue_des_services_d_infrastructure() {
    let c = parse_compose("docker-compose.yml", LINKWARDEN, Some("linkwarden")).unwrap();

    let app: Vec<_> = c
        .services
        .iter()
        .filter(|s| s.is_app)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(
        app,
        vec!["linkwarden"],
        "seul le service construit depuis le depot est l'app"
    );

    let infra: Vec<_> = c
        .services
        .iter()
        .filter(|s| !s.is_app)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(infra, vec!["postgres"]);
}

#[test]
fn la_base_de_donnees_est_deduite_avec_sa_preuve() {
    let c = parse_compose("docker-compose.yml", LINKWARDEN, Some("linkwarden")).unwrap();
    let s = services_from_compose(&c);

    assert_eq!(s.database, Database::PostgreSql);
    assert!(s
        .database_evidence
        .as_deref()
        .unwrap()
        .contains("postgres:16-alpine"));
    assert!(!s.needs_redis);
    assert!(s.unsupported.is_empty());
}

#[test]
fn le_port_et_la_configuration_de_l_app_sont_extraits() {
    let c = parse_compose("docker-compose.yml", LINKWARDEN, Some("linkwarden")).unwrap();
    let app = c.services.iter().find(|s| s.is_app).unwrap();

    assert_eq!(app.ports, vec![3000]);
    assert_eq!(
        app.environment.get("DATABASE_URL").map(String::as_str),
        Some("postgresql://postgres:secret@postgres:5432/postgres")
    );
    assert_eq!(app.volumes, vec!["/data/data"]);
}

#[test]
fn les_deux_syntaxes_de_environment_sont_lues() {
    // Table d'un cote, liste de CLE=valeur de l'autre : les deux sont courantes.
    let c = parse_compose("c.yml", LINKWARDEN, Some("linkwarden")).unwrap();
    let pg = c.services.iter().find(|s| s.name == "postgres").unwrap();
    let app = c.services.iter().find(|s| s.is_app).unwrap();

    assert_eq!(
        pg.environment.get("POSTGRES_PASSWORD").map(String::as_str),
        Some("secret")
    );
    assert!(app.environment.contains_key("NEXTAUTH_URL"));
}

#[test]
fn un_service_sans_equivalent_yunohost_est_signale() {
    let yml = r#"
services:
  app:
    build: .
  search:
    image: docker.elastic.co/elasticsearch/elasticsearch:8.13.0
  cache:
    image: redis:7
"#;
    let s = services_from_compose(&parse_compose("c.yml", yml, None).unwrap());

    assert!(s.needs_redis, "redis a un helper, ce n'est pas un blocage");
    assert_eq!(s.unsupported.len(), 1);
    assert!(s.unsupported[0].contains("elasticsearch"));
}

#[test]
fn deux_bases_differentes_sont_signalees_car_une_seule_est_provisionnable() {
    let yml = r#"
services:
  app:
    build: .
  db:
    image: postgres:16
  legacy:
    image: mariadb:11
"#;
    let s = services_from_compose(&parse_compose("c.yml", yml, None).unwrap());

    assert_eq!(s.database, Database::PostgreSql);
    assert_eq!(s.unsupported.len(), 1, "la seconde base doit remonter");
    assert!(s.unsupported[0].contains("seconde base"));
}

#[test]
fn une_base_du_service_applicatif_lui_meme_n_est_pas_comptee() {
    // Une image applicative peut contenir « postgres » dans son nom sans etre
    // une base : seul un service d'infrastructure compte.
    let yml = "services:\n  postgres-exporter:\n    build: .\n";
    let s = services_from_compose(&parse_compose("c.yml", yml, None).unwrap());
    assert_eq!(s.database, Database::None);
}

#[test]
fn un_yaml_illisible_ne_produit_pas_de_faits_inventes() {
    assert!(parse_compose("c.yml", "ceci: n'est pas: du compose valide\n  - [", None).is_none());
}

#[test]
fn le_fichier_compose_est_trouve_selon_l_ordre_de_preference() {
    let tree = vec!["compose.yaml".to_string(), "docker-compose.yml".to_string()];
    assert_eq!(find_compose(&tree).as_deref(), Some("docker-compose.yml"));
    assert_eq!(find_compose(&["README.md".to_string()]), None);
}

#[test]
fn un_compose_qui_ne_decrit_que_les_dependances_n_a_pas_de_service_applicatif() {
    // Cas reel d'outline : le compose racine ne sert qu'a lancer postgres et
    // redis en developpement, l'application tourne a cote. Ne rien designer est
    // ici la bonne reponse.
    let yml = "services:\n  redis:\n    image: redis\n  postgres:\n    image: postgres\n";
    let c = parse_compose("docker-compose.yml", yml, Some("outline")).unwrap();
    assert!(c.services.iter().all(|s| !s.is_app));
}

#[test]
fn un_compose_publie_designe_l_app_par_son_image_faute_de_build() {
    // Cas reel de linkwarden, paperless-ngx, immich : les compose destines aux
    // utilisateurs finaux referencent une image publiee.
    let yml = r#"
services:
  db:
    image: postgres:16
  broker:
    image: redis:7
  webserver:
    image: ghcr.io/paperless-ngx/paperless-ngx:latest
    ports:
      - "8000:8000"
"#;
    let c = parse_compose("docker-compose.yml", yml, Some("paperless-ngx")).unwrap();
    let app: Vec<_> = c.services.iter().filter(|s| s.is_app).collect();
    assert_eq!(app.len(), 1);
    assert_eq!(app[0].name, "webserver");
    assert_eq!(app[0].ports, vec![8000]);
}

#[test]
fn les_outils_d_administration_ne_sont_pas_pris_pour_l_application() {
    // Cas reel de block/buzz : son compose ne lance que des dependances de
    // developpement. Adminer, Keycloak, Prometheus et le client minio etaient
    // designes comme le service applicatif.
    let yml = r#"
services:
  postgres:
    image: postgres:17-alpine
  redis:
    image: redis:7-alpine
  adminer:
    image: adminer:latest
  keycloak:
    image: quay.io/keycloak/keycloak:26.0
  minio:
    image: minio/minio:latest
  minio-init:
    image: minio/mc:latest
  prometheus:
    image: prom/prometheus:latest
"#;
    let c = parse_compose("docker-compose.yml", yml, Some("buzz")).unwrap();

    let app: Vec<&str> = c
        .services
        .iter()
        .filter(|s| s.is_app)
        .map(|s| s.name.as_str())
        .collect();
    assert!(app.is_empty(), "aucun service applicatif ici, or : {app:?}");

    // minio signale un besoin de stockage S3, que le paquet embarque ; son
    // client `minio/mc` n'est que de l'outillage.
    let s = services_from_compose(&c);
    assert_eq!(s.database, Database::PostgreSql);
    assert!(s.needs_redis);
    assert!(s.needs_s3);
    assert!(s.unsupported.is_empty(), "{:?}", s.unsupported);
}

#[test]
fn un_compose_de_test_n_est_pas_retenu_faute_de_mieux() {
    // Cas reel de gotify : son unique compose lance un serveur OIDC pour la
    // suite de tests. Le retenir laissait croire a une dependance du produit.
    use ynp_dockerfile::find_compose;

    let tree = vec![
        "test/oidc/dex/docker-compose.yml".to_string(),
        "README.md".to_string(),
    ];
    assert_eq!(find_compose(&tree), None);

    // En revanche un compose d'auto-hebergement, meme profond, doit sortir.
    let tree = vec![".docker/selfhost/compose.yml".to_string()];
    assert_eq!(
        find_compose(&tree).as_deref(),
        Some(".docker/selfhost/compose.yml")
    );
}
