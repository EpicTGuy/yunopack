//! Le parseur est teste sur des Dockerfiles de la forme qu'on rencontre
//! reellement dans les projets auto-heberges, pas sur des cas d'ecole.

use ynp_dockerfile::parse_dockerfile;

#[test]
fn un_dockerfile_node_minimal_donne_stack_port_et_commande() {
    let df = r#"
FROM node:20-bookworm-slim
WORKDIR /app
COPY . .
RUN npm ci --omit=dev
EXPOSE 3000
CMD ["node", "server.js"]
"#;
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.stages.len(), 1);
    assert_eq!(r.stages[0].image, "node");
    assert_eq!(r.stages[0].tag.as_deref(), Some("20-bookworm-slim"));
    assert_eq!(r.workdir.as_deref(), Some("/app"));
    assert_eq!(r.expose, vec![3000]);
    assert_eq!(r.start_command().as_deref(), Some("node server.js"));
    assert_eq!(r.build_steps, vec!["npm ci --omit=dev"]);
}

#[test]
fn les_dependances_apt_sont_extraites_litteralement() {
    let df = r#"
FROM debian:bookworm
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      libvips-dev \
      imagemagick \
      ffmpeg=7:5.1.1 \
 && rm -rf /var/lib/apt/lists/*
"#;
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.apt_packages, vec!["libvips-dev", "imagemagick", "ffmpeg"]);
    assert!(
        r.build_steps.is_empty(),
        "bruit d'infrastructure retenu : {:?}",
        r.build_steps
    );
}

#[test]
fn dans_un_multi_etage_le_runtime_est_la_derniere_etape() {
    let df = r#"
FROM node:20 AS builder
WORKDIR /build
RUN npm ci && npm run build

FROM nginx:1.27-alpine
COPY --from=builder /build/dist /usr/share/nginx/html
EXPOSE 80
"#;
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.stages.len(), 2);
    assert_eq!(r.runtime_stage().unwrap().image, "nginx");
    assert_eq!(r.stages[0].alias.as_deref(), Some("builder"));
    // Les etapes de build de toutes les phases comptent : YunoHost construit
    // sur la machine cible.
    assert_eq!(r.build_steps, vec!["npm ci", "npm run build"]);
    assert_eq!(r.expose, vec![80]);
}

#[test]
fn une_etape_qui_part_d_une_precedente_herite_de_son_environnement() {
    let df = r#"
FROM python:3.12 AS base
ENV PYTHONUNBUFFERED=1
WORKDIR /srv

FROM base
ENV APP_ENV=production
CMD ["gunicorn", "app:app"]
"#;
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.env.get("PYTHONUNBUFFERED").map(String::as_str), Some("1"));
    assert_eq!(r.env.get("APP_ENV").map(String::as_str), Some("production"));
    assert_eq!(r.workdir.as_deref(), Some("/srv"));
}

#[test]
fn les_arguments_de_build_sont_substitues_dans_l_image_de_base() {
    let df = "ARG NODE_VERSION=20.11\nFROM node:${NODE_VERSION}-slim\n";
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.stages[0].image, "node");
    assert_eq!(r.stages[0].tag.as_deref(), Some("20.11-slim"));
}

#[test]
fn un_drapeau_de_from_n_est_pas_pris_pour_l_image() {
    // Constate sur gotify, dont les deux etapes de construction passaient pour
    // une image nommee « --platform=${BUILDPLATFORM} ».
    let df = concat!(
        "FROM --platform=${BUILDPLATFORM} node:20 AS js-builder\n",
        "FROM --platform=$BUILDPLATFORM golang:1.22 AS builder\n",
        "FROM debian:sid-slim\n",
    );
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.stages[0].image, "node");
    assert_eq!(r.stages[0].tag.as_deref(), Some("20"));
    assert_eq!(r.stages[0].alias.as_deref(), Some("js-builder"));
    assert_eq!(r.stages[1].image, "golang");
    assert_eq!(r.stages[2].image, "debian");
}

#[test]
fn un_registre_prive_avec_port_n_est_pas_confondu_avec_un_tag() {
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM registry.example.com:5000/team/app:2.1\n",
    );
    assert_eq!(r.stages[0].image, "registry.example.com:5000/team/app");
    assert_eq!(r.stages[0].tag.as_deref(), Some("2.1"));
}

#[test]
fn entrypoint_et_cmd_se_combinent_comme_chez_docker() {
    let df = r#"
FROM golang:1.22
ENTRYPOINT ["/app/server"]
CMD ["--config", "/etc/app.yml"]
"#;
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(
        r.start_command().as_deref(),
        Some("/app/server --config /etc/app.yml")
    );
}

#[test]
fn la_forme_shell_de_cmd_est_conservee_telle_quelle() {
    // Prefixer de `/bin/sh -c` serait fidele a Docker mais inutile ici : c'est
    // la commande nue qui sert a deriver un ExecStart systemd.
    let r = parse_dockerfile("Dockerfile", "FROM x\nCMD bundle exec puma -p 3000\n");
    assert_eq!(
        r.start_command().as_deref(),
        Some("bundle exec puma -p 3000")
    );
}

#[test]
fn les_deux_formes_de_env_sont_reconnues() {
    let df = r#"
FROM x
ENV LANG C.UTF-8
ENV APP_HOME=/srv/app NODE_ENV=production
ENV GREETING="hello world"
"#;
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.env.get("LANG").map(String::as_str), Some("C.UTF-8"));
    assert_eq!(r.env.get("APP_HOME").map(String::as_str), Some("/srv/app"));
    assert_eq!(
        r.env.get("NODE_ENV").map(String::as_str),
        Some("production")
    );
    assert_eq!(
        r.env.get("GREETING").map(String::as_str),
        Some("hello world")
    );
}

#[test]
fn les_paquets_alpine_sont_collectes_a_part_pour_etre_traduits() {
    // Ils n'ont pas les memes noms qu'en Debian : la traduction se fait plus
    // tard via assets/knowledge/apk-to-deb.toml.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM alpine\nRUN apk add --no-cache vips-dev tini\n",
    );
    assert_eq!(r.apk_packages, vec!["vips-dev", "tini"]);
    assert!(r.apt_packages.is_empty());
}

#[test]
fn les_noms_de_paquets_issus_d_une_variable_sont_ecartes() {
    // On ne peut pas savoir statiquement ce que vaut $EXTRA_PACKAGES : mieux
    // vaut ne rien affirmer que d'inventer un nom de paquet.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM debian\nRUN apt-get install -y curl $EXTRA_PACKAGES\n",
    );
    assert_eq!(r.apt_packages, vec!["curl"]);
}

#[test]
fn un_paquet_liste_deux_fois_n_apparait_qu_une_fois() {
    let df = "FROM debian\nRUN apt-get install -y curl git\nRUN apt-get install -y curl make\n";
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.apt_packages, vec!["curl", "git", "make"]);
}

#[test]
fn expose_gere_le_protocole_et_les_ports_multiples() {
    let r = parse_dockerfile("Dockerfile", "FROM x\nEXPOSE 8080/tcp 9090 8080\n");
    assert_eq!(r.expose, vec![8080, 9090]);
}

#[test]
fn les_volumes_sont_lus_dans_les_deux_syntaxes() {
    let r = parse_dockerfile("Dockerfile", "FROM x\nVOLUME [\"/data\", \"/config\"]\n");
    assert_eq!(r.volumes, vec!["/data", "/config"]);

    let r = parse_dockerfile("Dockerfile", "FROM x\nVOLUME /data\n");
    assert_eq!(r.volumes, vec!["/data"]);
}

#[test]
fn un_dockerfile_php_composer_realiste_est_entierement_decode() {
    let df = r#"
# Application PHP typique du catalogue auto-heberge
FROM composer:2.7 AS vendor
WORKDIR /app
COPY composer.json composer.lock ./
RUN composer install --no-dev --no-scripts --prefer-dist

FROM php:8.3-fpm-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
        libzip-dev \
        libpng-dev \
    && docker-php-ext-install pdo_mysql zip gd \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /var/www/html
COPY --from=vendor /app/vendor ./vendor
ENV APP_ENV=production
EXPOSE 9000
CMD ["php-fpm"]
"#;
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.stages.len(), 2);
    assert_eq!(r.runtime_stage().unwrap().image, "php");
    assert_eq!(
        r.runtime_stage().unwrap().tag.as_deref(),
        Some("8.3-fpm-bookworm")
    );
    assert_eq!(r.apt_packages, vec!["libzip-dev", "libpng-dev"]);
    assert_eq!(
        r.build_steps,
        vec!["composer install --no-dev --no-scripts --prefer-dist",]
    );
    // Les extensions de l'image vont au manifeste, pas aux etapes.
    assert_eq!(r.extensions_php, vec!["pdo_mysql", "zip", "gd"]);
    assert_eq!(r.expose, vec![9000]);
    assert_eq!(r.env.get("APP_ENV").map(String::as_str), Some("production"));
    assert_eq!(r.start_command().as_deref(), Some("php-fpm"));
}

#[test]
fn un_dockerfile_sans_from_ne_fait_pas_paniquer_le_parseur() {
    let r = parse_dockerfile("Dockerfile", "# vide\nRUN echo hello\n");
    assert!(r.stages.is_empty());
    assert!(r.start_command().is_none());
}

#[test]
fn le_dockerfile_de_developpement_n_est_pas_choisi_avant_celui_de_production() {
    use ynp_dockerfile::find_dockerfile;

    let tree = vec!["Dockerfile.dev".to_string(), "Dockerfile".to_string()];
    assert_eq!(find_dockerfile(&tree).as_deref(), Some("Dockerfile"));

    let tree = vec![
        "docker/Dockerfile".to_string(),
        "test/Dockerfile".to_string(),
    ];
    assert_eq!(find_dockerfile(&tree).as_deref(), Some("docker/Dockerfile"));

    assert_eq!(find_dockerfile(&["README.md".to_string()]), None);
}

// --- Defauts constates sur des Dockerfiles reels (grist, paperless-ngx, linkwarden) ---

#[test]
fn une_etape_qui_part_d_un_alias_expose_l_image_de_base_reelle() {
    // Constate sur paperless-ngx : le runtime etait rapporte comme
    // « s6-overlay-base », qui est un alias local et non une image.
    let df = r#"
FROM python:3.12-slim-bookworm AS s6-overlay-base
RUN echo base

FROM s6-overlay-base AS main-app
EXPOSE 8000
CMD ["/init"]
"#;
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.runtime_stage().unwrap().image, "python");
    assert_eq!(
        r.runtime_stage().unwrap().tag.as_deref(),
        Some("3.12-slim-bookworm")
    );
}

#[test]
fn les_options_buildkit_de_run_ne_polluent_pas_l_etape_de_build() {
    // Constate sur linkwarden.
    let df =
        "FROM node:22\nRUN --mount=type=cache,sharing=locked,target=/root/.yarn yarn install\n";
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.build_steps, vec!["yarn install"]);
}

#[test]
fn les_fragments_de_blocs_shell_ne_sont_pas_pris_pour_des_etapes() {
    // Constate sur grist et paperless-ngx : le decoupage sur `;` casse un bloc
    // `case ... esac` en morceaux de syntaxe.
    let df = r#"
FROM debian
RUN case "$TAG" in dev) echo d ;; *) echo p ;; esac && npm run build
"#;
    let r = parse_dockerfile("Dockerfile", df);
    // Le bloc est garde entier : chaque etape est du shell valide, et
    // l'etape de construction y figure toujours.
    for etape in &r.build_steps {
        let ok = std::process::Command::new("bash")
            .args(["-n", "-c", etape])
            .status()
            .expect("bash")
            .success();
        assert!(ok, "etape syntaxiquement fausse : {etape}");
    }
    assert!(r.build_steps.iter().any(|s| s.contains("npm run build")));
}

#[test]
fn le_corps_d_un_bloc_conditionnel_ne_devient_pas_inconditionnel() {
    // Cas de Buzz : le certificat n'est copie que si EXTRA_CA_CERTS est
    // defini. Decoupe, le `cp` s'executait toujours, sur un fichier absent.
    let df = "FROM rust\nRUN if [ -n \"${EXTRA_CA_CERTS}\" ]; then \\\n                cp /tmp/extra-ca/src /usr/local/share/ca-certificates/x.crt \\\n                && update-ca-certificates; fi\n";
    let r = parse_dockerfile("Dockerfile", df);
    assert!(
        !r.build_steps.iter().any(|s| s.starts_with("cp ")),
        "{:?}",
        r.build_steps
    );
}

#[test]
fn le_cache_de_cargo_chef_n_est_pas_une_etape() {
    let df = "FROM rust\nRUN cargo install cargo-chef --locked\n\
              RUN cargo chef cook --release --recipe-path recipe.json\n\
              RUN cargo build --release\n";
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.build_steps, vec!["cargo build --release".to_string()]);
}

#[test]
fn un_runtime_scratch_est_rapporte_tel_quel() {
    // Vikunja : binaire Go statique. C'est un signal favorable pour YunoHost,
    // pas une anomalie — il ne faut donc pas le masquer.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM golang AS b\nFROM scratch\nCMD [\"/app/x\"]\n",
    );
    assert_eq!(r.runtime_stage().unwrap().image, "scratch");
}

#[test]
fn une_liste_de_paquets_rangee_dans_un_arg_est_recuperee() {
    // Motif de paperless-ngx : sans substitution, on perdrait toutes les
    // dependances de l'application.
    let df = r#"
FROM debian:trixie-slim
ARG RUNTIME_PACKAGES="\
  # General utils
  curl \
  # Timezones support
  tzdata \
  gosu"
RUN apt-get update && apt-get install --yes --quiet --no-install-recommends ${RUNTIME_PACKAGES}
"#;
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.apt_packages, vec!["curl", "tzdata", "gosu"]);
}

#[test]
fn une_variable_sans_valeur_connue_ne_produit_pas_de_faux_paquet() {
    let df = "FROM debian\nARG EXTRA\nRUN apt-get install -y curl ${EXTRA}\n";
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.apt_packages, vec!["curl"]);
}

#[test]
fn une_variable_dont_le_nom_en_prefixe_une_autre_est_substituee_correctement() {
    let df = "ARG APP=grist\nARG APP_VERSION=1.2\nFROM node:$APP_VERSION\n";
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.stages[0].tag.as_deref(), Some("1.2"));
}

#[test]
fn le_dockerfile_de_conditionnement_n_est_pas_pris_pour_celui_du_produit() {
    // Cas reel de miniflux, qui en compte quatre. Retenir celui de Debian
    // faisait passer devscripts et dh-make pour des dependances de l'app.
    use ynp_core::tree::RepoTree;
    use ynp_dockerfile::choose_dockerfile;

    let tree = RepoTree::from_pairs([
        (
            "packaging/debian/Dockerfile",
            "FROM golang:1\nRUN apt-get install -y devscripts dh-make debhelper\n\
             CMD [\"/src/packaging/debian/build.sh\"]\n",
        ),
        (
            "packaging/docker/alpine/Dockerfile",
            "FROM golang:1.26 AS build\nRUN make miniflux\n\
             FROM alpine:3.24\nEXPOSE 8080\nCMD [\"/usr/bin/miniflux\"]\n",
        ),
        (
            "packaging/rpm/Dockerfile",
            "FROM fedora\nRUN dnf install -y rpm-build\nCMD [\"/build.sh\"]\n",
        ),
    ]);

    assert_eq!(
        choose_dockerfile(&tree).as_deref(),
        Some("packaging/docker/alpine/Dockerfile")
    );
}

#[test]
fn le_dockerfile_racine_est_prefere_quand_il_existe() {
    use ynp_core::tree::RepoTree;
    use ynp_dockerfile::choose_dockerfile;

    let tree = RepoTree::from_pairs([
        (
            "Dockerfile",
            "FROM node:20\nEXPOSE 3000\nCMD [\"node\",\"s.js\"]\n",
        ),
        (
            "docker/Dockerfile",
            "FROM node:20\nEXPOSE 3000\nCMD [\"node\",\"s.js\"]\n",
        ),
    ]);
    assert_eq!(choose_dockerfile(&tree).as_deref(), Some("Dockerfile"));
}

#[test]
fn le_choix_est_reproductible_a_egalite_de_score() {
    use ynp_core::tree::RepoTree;
    use ynp_dockerfile::choose_dockerfile;

    let tree = RepoTree::from_pairs([
        ("b/Dockerfile", "FROM node:20\nEXPOSE 3000\n"),
        ("a/Dockerfile", "FROM node:20\nEXPOSE 3000\n"),
    ]);
    // Deux executions doivent rendre le meme resultat.
    assert_eq!(choose_dockerfile(&tree), choose_dockerfile(&tree));
    assert_eq!(choose_dockerfile(&tree).as_deref(), Some("b/Dockerfile"));
}

#[test]
fn une_etape_finale_herite_du_port_et_du_demarrage_de_son_parent() {
    // Cas reel de block/buzz : EXPOSE et ENTRYPOINT sont declares dans
    // `runtime-base`, et la derniere etape n'est qu'un `FROM runtime-base`.
    // Sans heritage, le Dockerfile principal paraissait n'exposer aucun port,
    // et un Dockerfile secondaire lui etait prefere.
    let df = r#"
FROM rust:1.83 AS builder
RUN cargo build --release

FROM debian:bookworm-slim AS runtime-base
WORKDIR /srv
EXPOSE 3000 8080
ENTRYPOINT ["/usr/local/bin/buzz-relay"]

FROM runtime-base AS runtime
"#;
    let r = parse_dockerfile("Dockerfile", df);

    assert_eq!(r.expose, vec![3000, 8080]);
    assert_eq!(
        r.start_command().as_deref(),
        Some("/usr/local/bin/buzz-relay")
    );
    assert_eq!(r.workdir.as_deref(), Some("/srv"));
    assert_eq!(r.runtime_stage().unwrap().image, "debian");
}

#[test]
fn une_etape_finale_qui_redefinit_l_emporte_sur_son_parent() {
    let df = r#"
FROM debian AS base
EXPOSE 8080
CMD ["/base"]

FROM base
EXPOSE 9000
CMD ["/final"]
"#;
    let r = parse_dockerfile("Dockerfile", df);
    assert_eq!(r.expose, vec![9000]);
    assert_eq!(r.start_command().as_deref(), Some("/final"));
}

#[test]
fn un_fichier_source_nomme_dockerfile_n_est_pas_un_dockerfile() {
    // Defaut trouve en faisant analyser yunopack par lui-meme : ses fichiers
    // `dockerfile.rs` etaient lus comme des Dockerfiles. Il en tirait les
    // dependances de ses propres fixtures et se croyait ecrit en Python,
    // parce qu'une fixture contient `FROM python:3.12`.
    use ynp_core::tree::RepoTree;
    use ynp_dockerfile::choose_dockerfile;

    let tree = RepoTree::from_pairs([
        (
            "crates/ynp-dockerfile/src/dockerfile.rs",
            "// FROM python:3.12\n",
        ),
        (
            "crates/ynp-dockerfile/tests/dockerfile.rs",
            "// FROM golang:1\n",
        ),
        ("Cargo.toml", "[workspace]\n"),
    ]);
    assert_eq!(choose_dockerfile(&tree), None, "aucun vrai Dockerfile ici");
}

#[test]
fn les_variantes_legitimes_restent_reconnues() {
    use ynp_dockerfile::dockerfile::est_un_dockerfile;

    for nom in [
        "Dockerfile",
        "dockerfile",
        "Dockerfile.prod",
        "Dockerfile.alpine",
        "grist.Dockerfile",
        "Containerfile",
    ] {
        assert!(est_un_dockerfile(nom), "{nom} devrait etre reconnu");
    }
    for nom in [
        "dockerfile.rs",
        "Dockerfile.md",
        "dockerfile.yml",
        "dockerfile_test.go",
        "Dockerfile.j2",
        "dockerfile.tera",
    ] {
        assert!(!est_un_dockerfile(nom), "{nom} ne devrait pas l'etre");
    }
}

#[test]
fn une_variante_de_developpement_n_est_pas_la_recette_de_production() {
    use ynp_core::tree::RepoTree;
    use ynp_dockerfile::choose_dockerfile;
    // Cas d'AnonAddy : son seul Dockerfile monte un poste de travail. Le
    // prendre pour la recette de production faisait echouer la traduction de
    // la commande de demarrage sur un paquet qui, sans lui, sortait complet.
    let tree = RepoTree::from_pairs([(
        "dev.Dockerfile",
        "FROM php:8.2-fpm\nEXPOSE 9000\nCMD [\"php-fpm\"]\n",
    )]);
    assert_eq!(choose_dockerfile(&tree), None);
    // Variante composee : cas de Modoboa.
    let tree = RepoTree::from_pairs([(
        "docker/Dockerfile.radicale.dev",
        "FROM python:3.12\nCMD [\"python3 -m radicale\"]\n",
    )]);
    assert_eq!(choose_dockerfile(&tree), None);
}

#[test]
fn une_variante_separee_par_un_tiret_ou_un_souligne_est_reconnue() {
    use ynp_core::tree::RepoTree;
    use ynp_dockerfile::choose_dockerfile;
    // Cas d'ibis : `Dockerfile_ci` recopie un binaire construit par la
    // chaine d'integration ; `Dockerfile` le construit.
    let tree = RepoTree::from_pairs([
        (
            "docker/Dockerfile_ci",
            "FROM debian:bookworm-slim\nCOPY ibis /usr/local/bin/ibis\nEXPOSE 3000\nCMD [\"ibis\"]\n",
        ),
        (
            "docker/Dockerfile",
            "FROM rust:1.80 AS build\nRUN cargo build --release\nFROM debian:bookworm-slim\nCOPY --from=build /target/release/ibis /usr/local/bin/ibis\nCMD [\"ibis\"]\n",
        ),
    ]);
    assert_eq!(
        choose_dockerfile(&tree).as_deref(),
        Some("docker/Dockerfile")
    );
    // Cas de Mailtrain : `Dockerfile-Develop` lance `tail -f /dev/null`.
    let tree = RepoTree::from_pairs([
        (
            "Dockerfile-Develop",
            "FROM node:14\nCMD tail -f /dev/null\n",
        ),
        (
            "Dockerfile",
            "FROM node:14\nRUN npm ci\nEXPOSE 3000\nCMD [\"node\", \"index.js\"]\n",
        ),
    ]);
    assert_eq!(choose_dockerfile(&tree).as_deref(), Some("Dockerfile"));
}

#[test]
fn une_variante_nommee_par_sa_base_reste_une_recette() {
    use ynp_core::tree::RepoTree;
    use ynp_dockerfile::choose_dockerfile;
    // Cas de Linkding : `default.Dockerfile` et `alpine.Dockerfile` decrivent
    // bien l'application, sous deux bases differentes.
    let tree = RepoTree::from_pairs([
        (
            "docker/default.Dockerfile",
            "FROM python:3.12\nEXPOSE 9090\nCMD [\"./bootstrap.sh\"]\n",
        ),
        (
            "docker/alpine.Dockerfile",
            "FROM python:3.12-alpine\nEXPOSE 9090\nCMD [\"./bootstrap.sh\"]\n",
        ),
    ]);
    assert!(choose_dockerfile(&tree).is_some());
}

#[test]
fn les_copies_depuis_le_depot_sont_relevees_par_leur_chemin_dans_l_image() {
    // Cas de Mealie : le point d'entree `/app/run.sh` est `docker/entry.sh`.
    let r = ynp_dockerfile::parse_dockerfile(
        "Dockerfile",
        "FROM python:3.12\n\
         COPY --chmod=755 ./docker/entry.sh /app/run.sh\n\
         COPY entrypoint.sh healthcheck.sh /usr/local/bin/\n\
         COPY --from=builder /app/dist /app/dist\n\
         ENTRYPOINT [\"/app/run.sh\"]\n",
    );
    assert_eq!(
        r.copies.get("/app/run.sh").map(String::as_str),
        Some("docker/entry.sh")
    );
    assert_eq!(
        r.copies
            .get("/usr/local/bin/entrypoint.sh")
            .map(String::as_str),
        Some("entrypoint.sh")
    );
    // Une copie entre etapes ne vient pas du depot.
    assert!(!r.copies.contains_key("/app/dist"));
}

#[test]
fn une_destination_de_copy_ecrite_avec_une_variable_est_developpee() {
    // Cas de Mealie : `COPY ./docker/entry.sh $MEALIE_HOME/run.sh`.
    let r = ynp_dockerfile::parse_dockerfile(
        "Dockerfile",
        "FROM python:3.12\nENV MEALIE_HOME=/app\nCOPY ./docker/entry.sh $MEALIE_HOME/run.sh\n",
    );
    assert_eq!(
        r.copies.get("/app/run.sh").map(String::as_str),
        Some("docker/entry.sh")
    );
}

#[test]
fn une_etape_qui_se_designe_elle_meme_ne_fait_pas_tourner_l_analyse() {
    // Cas de Passbolt : l'analyse tournait indefiniment en remontant la
    // chaine des etapes pour trouver un repertoire de travail.
    let r = ynp_dockerfile::parse_dockerfile(
        "Dockerfile",
        "FROM debian AS debian\nFROM debian AS final\nCOPY --from=debian /x /y\n",
    );
    assert!(r.copies_d_etape.is_empty());
}

#[test]
fn l_administration_de_l_image_n_est_pas_une_etape() {
    // Cas de Gollum : ses etapes modifiaient le groupe www-data et le
    // `.gitconfig` d'un compte de l'image.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM ruby:3.3\nRUN bundle install && bundle exec rake install\n\
         RUN groupmod -g 1000 www-data\n\
         RUN git config --file /home/www-data/.gitconfig --add safe.directory /wiki\n\
         RUN git config --global user.name x && usermod -aG staff app\n",
    );
    assert_eq!(
        r.build_steps,
        vec!["bundle install", "bundle exec rake install"]
    );
}

#[test]
fn les_paquets_d_une_ligne_a_bloc_vont_au_manifeste() {
    // Cas de passbolt : une ligne qui contient un `if` reste entiere, mais
    // son apt-get ne peut pas tourner sous l'utilisateur de l'application.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM php:8.2-fpm\nRUN apt-get update && apt-get -y install --no-install-recommends git unzip \
         && if [ -n \"$X\" ]; then echo x; fi && rm -rf /var/lib/apt/lists/*\n",
    );
    assert!(r.apt_packages.contains(&"git".to_string()));
    assert!(r.apt_packages.contains(&"unzip".to_string()));
    assert_eq!(r.build_steps.len(), 1);
    assert!(
        !r.build_steps[0].contains("apt-get"),
        "{}",
        r.build_steps[0]
    );
    assert!(r.build_steps[0].contains("if [ -n"));
}

#[test]
fn un_sous_shell_d_installation_va_au_manifeste() {
    // Cas de dawarich : `(apt-get install … 2>/dev/null || true)`.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM ruby:3.4\nRUN (apt-get install -y --no-install-recommends mesa-libgallium 2>/dev/null || true)\n",
    );
    assert_eq!(r.apt_packages, vec!["mesa-libgallium"]);
    assert!(
        r.build_steps.iter().all(|e| !e.contains("apt-get")),
        "{:?}",
        r.build_steps
    );
}

#[test]
fn un_test_shell_isole_n_est_pas_une_etape() {
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node:22\nRUN [ -f doc-lastmod.json ] && npm run build\n",
    );
    assert_eq!(r.build_steps, vec!["npm run build"]);
}

#[test]
fn chaque_etape_se_construit_dans_son_repertoire() {
    // Cas de booklore : le frontal depuis booklore-ui/, l'API depuis
    // booklore-api/, chacun sous son WORKDIR.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node:22 AS front\nWORKDIR /angular-app\nCOPY booklore-ui/package.json booklore-ui/package-lock.json ./\n\
         RUN npm ci\nCOPY booklore-ui/ ./\nRUN npm run build\n\
         FROM gradle:8-jdk21 AS back\nWORKDIR /springboot-app\nCOPY booklore-api/ /springboot-app/\n\
         RUN yq eval '.v = 1' -i /springboot-app/src/main/resources/application.yaml\nRUN gradle build\n\
         FROM eclipse-temurin:21\nCOPY src ./src\nRUN echo fini\n",
    );
    assert_eq!(
        r.build_steps,
        vec![
            "cd booklore-ui && npm ci",
            "cd booklore-ui && npm run build",
            "cd booklore-api && yq eval '.v = 1' -i src/main/resources/application.yaml",
            "cd booklore-api && gradle build",
        ]
    );
}

#[test]
fn un_workdir_vers_une_copie_change_le_repertoire_des_commandes() {
    // Cas de checkmate : tout est copie sous /app, puis chaque partie se
    // construit dans son repertoire.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node:22\nWORKDIR /app\nCOPY client ./client\nCOPY server ./server\n\
         WORKDIR /app/client\nRUN npm ci\nWORKDIR /app/server\nRUN npm run build\n",
    );
    assert_eq!(
        r.build_steps,
        vec!["cd client && npm ci", "cd server && npm run build"]
    );
    assert_eq!(r.workdir.as_deref(), Some("/app/server"));
}

#[test]
fn une_sortie_de_construction_devient_le_repertoire_de_l_application() {
    // Cas de cobalt : `pnpm deploy … /prod/api`, copie dans /app.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node:24 AS base\nFROM base AS build\nWORKDIR /app\nCOPY . /app\n\
         RUN pnpm install --prod --frozen-lockfile\n\
         RUN pnpm deploy --filter=@imput/cobalt-api --prod /prod/api\n\
         FROM base AS api\nWORKDIR /app\nCOPY --from=build /prod/api /app\nCMD [\"node\", \"src/cobalt\"]\n",
    );
    assert!(r.build_steps.contains(
        &"pnpm deploy --filter=@imput/cobalt-api --prod $PWD/.sortie/prod/api".to_string()
    ));
    assert!(r
        .build_steps
        .contains(&"mkdir -p $PWD/.sortie/prod/api".to_string()));
    assert_eq!(r.contexte_final.as_deref(), Some(".sortie/prod/api"));
}

#[test]
fn la_racine_d_une_etape_n_est_pas_une_sortie_de_construction() {
    // Cas d'azuracast : `COPY --from=x / /` remplacait la chaine vide dans
    // toutes les etapes.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM debian AS base\nWORKDIR /app\nCOPY . /app\nRUN bash /app/setup.sh\n\
         FROM scratch AS final\nCOPY --from=base / /\nWORKDIR /app\nCMD [\"php\", \"run.php\"]\n",
    );
    assert!(r.build_steps.iter().all(|e| !e.contains(".sortie")));
    assert_eq!(r.contexte_final, None);
}

#[test]
fn les_outils_des_images_php_donnent_leurs_extensions() {
    // Cas de congressus.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM php:8.2-apache\nRUN docker-php-ext-install pdo pdo_mysql && docker-php-source extract\n\
         RUN git clone https://github.com/php-memcached-dev/php-memcached.git /usr/src/php/ext/memcached/\n\
         RUN docker-php-ext-configure memcached && docker-php-ext-install -j4 zip memcached\n\
         RUN pecl install memcache-8.2 && docker-php-ext-enable memcache\nRUN composer install\n",
    );
    assert_eq!(r.build_steps, vec!["composer install"]);
    assert_eq!(
        r.extensions_php,
        vec!["pdo", "pdo_mysql", "zip", "memcached", "memcache"]
    );
}

#[test]
fn npm_link_n_est_pas_une_etape() {
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node:22\nRUN npm ci && npm run build && npm link\n",
    );
    assert_eq!(r.build_steps, vec!["npm ci", "npm run build"]);
}

#[test]
fn un_gestionnaire_de_versions_de_node_n_est_pas_une_etape() {
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM python:3.11\nRUN npm install -g n && n 18 && npm ci\n",
    );
    assert_eq!(r.build_steps, vec!["npm install -g n", "npm ci"]);
}

#[test]
fn un_outil_global_de_composer_n_est_pas_une_etape() {
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM php:8.2\nRUN composer global require hirak/prestissimo && composer install --no-dev\n",
    );
    assert_eq!(r.build_steps, vec!["composer install --no-dev"]);
}

#[test]
fn le_depot_copie_au_dessus_du_workdir_donne_le_contexte() {
    // Cas de broadcast-box.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node AS web-build\nWORKDIR /broadcast-box/web\nCOPY . /broadcast-box\n\
         RUN npm install && npm run build\nFROM golang:alpine AS go-build\nWORKDIR /broadcast-box\n\
         COPY . /broadcast-box\nRUN go build\n",
    );
    assert_eq!(
        r.build_steps,
        vec![
            "cd web && npm install",
            "cd web && npm run build",
            "go build"
        ]
    );
}

#[test]
fn les_fichiers_copies_d_une_etape_vers_le_repertoire_final_sont_copies() {
    // Cas de freetar : la roue construite par poetry est installee ensuite.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM python:3.12 AS builder\nRUN pip install poetry\nCOPY . /app\nWORKDIR /app\n\
         RUN poetry build --format=wheel\n\
         FROM python:3.12\nCOPY --from=builder /app/dist/*.whl .\nRUN pip install *.whl && rm *.whl\n",
    );
    let i = |e: &str| r.build_steps.iter().position(|x| x.contains(e)).unwrap();
    assert!(
        i("poetry build") < i("cp dist/*.whl .") && i("cp dist/*.whl .") < i("pip install *.whl")
    );
}

#[test]
fn un_workdir_dans_une_copie_en_miroir_donne_le_contexte() {
    // Cas de your_spotify : copies fichier par fichier, puis WORKDIR.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node:22 AS builder\nWORKDIR /app\nCOPY package.json package.json\n\
         COPY apps/server/package.json apps/server/package.json\nRUN pnpm install\n\
         COPY apps/server/src apps/server/src\nWORKDIR /app/apps/server\nRUN pnpm build\n",
    );
    assert_eq!(
        r.build_steps,
        vec!["pnpm install", "cd apps/server && pnpm build"]
    );
}

#[test]
fn une_etape_qui_lit_un_arg_sans_valeur_est_ecartee() {
    // Cas de booklore : la version de l'amont inscrite par yq.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM gradle:9 AS build\nARG APP_VERSION\nARG TARGETARCH\n\
         RUN yq eval '.app.version = strenv(APP_VERSION)' -i app.yaml\n\
         RUN curl -o tool https://example.org/tool-${TARGETARCH}\nRUN ./gradlew build\n",
    );
    assert!(r.build_steps.iter().all(|e| !e.contains("yq")));
    assert!(r.build_steps.iter().any(|e| e.contains("TARGETARCH")));
    assert!(r.build_steps.iter().any(|e| e.contains("gradlew")));
}

#[test]
fn le_produit_d_une_etape_rejoint_celle_qui_l_embarque() {
    // Cas de booklore : le front Angular dans les ressources Spring, puis le jar.
    let r = parse_dockerfile(
        "Dockerfile",
        "FROM node:24 AS front\nWORKDIR /angular-app\nCOPY booklore-ui/ .\nRUN npm ci && npm run build\n\
         FROM gradle:9 AS api\nWORKDIR /springboot-app\nCOPY booklore-api/ .\n\
         COPY --from=front /angular-app/dist/booklore/browser /springboot-app/src/main/resources/static\n\
         RUN ./gradlew build\n\
         FROM eclipse-temurin:25-jre\nWORKDIR /app\nCOPY --from=api /springboot-app/build/libs/api.jar /app/app.jar\n\
         CMD [\"java\", \"-jar\", \"app.jar\"]\n",
    );
    let copie = "mkdir -p booklore-api/src/main/resources && rm -rf booklore-api/src/main/resources/static \
                 && cp -a booklore-ui/dist/booklore/browser booklore-api/src/main/resources/static";
    let i = |e: &str| r.build_steps.iter().position(|x| x == e).unwrap();
    assert!(i("cd booklore-ui && npm run build") < i(copie));
    assert!(i(copie) < i("cd booklore-api && ./gradlew build"));
    assert!(r
        .copies_d_etape
        .iter()
        .any(|c| c.depuis == "booklore-api/build/libs/api.jar"));
}
