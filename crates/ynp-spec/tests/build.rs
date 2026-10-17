//! La specification est le seul document relu par un humain : elle doit etre
//! juste, et avouer ce qu'elle ignore.

use ynp_core::facts::*;
use ynp_core::spec::{Architectures, UrlScheme};

/// Une application Go dont l'amont publie des binaires : le cas le plus
/// confortable, celui de gotify et miniflux.
fn depot_go_avec_binaires() -> RepoFacts {
    RepoFacts {
        source: SourceRef {
            forge: Forge::GitHub,
            owner: "miniflux".into(),
            repo: "v2".into(),
            url: "https://github.com/miniflux/v2".into(),
            default_branch: Some("main".into()),
            commit: None,
        },
        meta: RepoMeta {
            description: Some("Minimalist and opinionated feed reader".into()),
            license_spdx: Some("Apache-2.0".into()),
            homepage: Some("https://miniflux.app".into()),
            ..Default::default()
        },
        stack: StackFacts {
            primary: Technology::Go,
            runtime_version: Some("1.26".into()),
            ..Default::default()
        },
        services: ServiceFacts {
            database: Database::PostgreSql,
            ..Default::default()
        },
        build: Some(BuildRecipe {
            dockerfile_path: "Dockerfile".into(),
            expose: vec![8080],
            apt_packages: vec!["ca-certificates".into(), "curl".into()],
            ..Default::default()
        }),
        selection: Some(SourceSelection {
            reference: "2.3.3".into(),
            strategy: "latest_github_release".into(),
            version_regex: None,
            version: Some("2.3.3".into()),
            kind: "release".into(),
            url: "https://github.com/miniflux/v2/archive/2.3.3.tar.gz".into(),
            sha256: "ab".repeat(32),
            prebuilt: vec![ArchAsset {
                arch: "amd64".into(),
                name: "miniflux-linux-amd64".into(),
                url: "https://x/miniflux-linux-amd64".into(),
                sha256: "cd".repeat(32),
                empreinte_publiee: None,
                taille: 0,
                sous_repertoire: None,
                executables: Vec::new(),
                pattern: "^miniflux-linux-amd64$".into(),
                extract: false,
            }],
        }),
        ..Default::default()
    }
}

/// Le meme depot, mais dont l'amont publie un .env.example : tout est alors
/// deductible.
fn depot_complet() -> RepoFacts {
    let mut f = depot_go_avec_binaires();
    f.config = ConfigFacts {
        example_file: Some(".env.example".into()),
        moteurs_cites: Default::default(),
        fichier_d_exemple: None,
        variables: vec![
            ConfigVar {
                name: "PORT".into(),
                role: ConfigRole::Port,
                default: Some("8080".into()),
                secret: false,
                source: ".env.example".into(),
                facultative: false,
                format_secret: None,
            },
            ConfigVar {
                name: "DATABASE_URL".into(),
                role: ConfigRole::DatabaseUrl,
                default: None,
                secret: false,
                source: ".env.example".into(),
                facultative: false,
                format_secret: None,
            },
            // L'adresse publique de miniflux, qui recoit celle du domaine.
            ConfigVar {
                name: "BASE_URL".into(),
                role: ConfigRole::BaseUrl,
                default: None,
                secret: false,
                source: ".env.example".into(),
                facultative: false,
                format_secret: None,
            },
        ],
    };
    f
}

#[test]
fn une_application_bien_documentee_produit_une_specification_complete() {
    let spec = ynp_spec::build(&depot_complet()).unwrap();

    assert!(
        spec.is_complete(),
        "champs manquants : {:?}",
        spec.unresolved()
    );
    // Le depot s'appelle miniflux/v2 : « v2 » ne designe pas l'application.
    assert_eq!(spec.app.id, "miniflux");
    assert_eq!(spec.app.name, "Miniflux");
    assert_eq!(spec.upstream.license.value().unwrap(), "Apache-2.0");
    assert_eq!(spec.app.version.value().unwrap(), "2.3.3");
    assert_eq!(spec.resources.database, Database::PostgreSql);
    // Un service sans variable de prefixe se sert a la racine d'un domaine :
    // c'est ce qui fonctionne a coup sur.
    assert_eq!(spec.install.url_scheme, UrlScheme::FullDomain);
}

#[test]
fn un_binaire_publie_borne_les_architectures_annoncees() {
    // Annoncer « all » masquerait l'app sur les plateformes non couvertes.
    let spec = ynp_spec::build(&depot_go_avec_binaires()).unwrap();
    assert_eq!(
        spec.integration.architectures,
        Architectures::Only(vec!["amd64".into()])
    );
}

#[test]
fn un_binaire_publie_dispense_de_construire_et_de_reserver_la_memoire() {
    let spec = ynp_spec::build(&depot_complet()).unwrap();

    assert!(spec.runtime.build_steps.is_empty(), "rien a construire");
    assert_eq!(spec.integration.ram_build, "50M");
    assert!(
        !spec.resources.sources.in_subdir,
        "un binaire nu n'a pas de sous-repertoire"
    );
    assert_eq!(
        spec.runtime.execstart.value().unwrap(),
        "__INSTALL_DIR__/miniflux"
    );
}

#[test]
fn le_socle_fourni_par_yunohost_n_est_pas_redeclare_en_dependance() {
    let spec = ynp_spec::build(&depot_go_avec_binaires()).unwrap();
    // `ca-certificates` vient du Dockerfile mais fait partie du socle ;
    // `postgresql` est ajoute parce qu'une base est provisionnee et qu'il faut
    // bien un serveur a joindre.
    assert_eq!(spec.resources.apt_packages, vec!["curl", "postgresql"]);
}

#[test]
fn une_application_opaque_avoue_ce_qu_elle_ignore() {
    // Le contrat du projet : ni licence, ni description, ni commande de
    // demarrage ne doivent etre inventees.
    let mut facts = depot_go_avec_binaires();
    facts.meta.license_spdx = None;
    facts.meta.description = None;
    facts.stack = StackFacts::default();
    facts.build = None;
    facts.selection = None;

    let spec = ynp_spec::build(&facts).unwrap();
    let manquants: Vec<String> = spec.unresolved().into_iter().map(|(c, _)| c).collect();

    assert!(manquants.contains(&"upstream.license".to_string()));
    assert!(manquants.contains(&"app.description_en".to_string()));
    assert!(manquants.contains(&"runtime.technology".to_string()));
    assert!(manquants.contains(&"resources.sources.url".to_string()));

    // Chaque manque dit ou chercher.
    for (_, marqueur) in spec.unresolved() {
        assert!(marqueur.starts_with("FIXME(yunopack)"), "{marqueur}");
    }
}

#[test]
fn la_configuration_reconnue_est_cablee_sur_les_valeurs_yunohost() {
    let mut facts = depot_complet();
    facts.config = ConfigFacts {
        example_file: Some(".env.example".into()),
        moteurs_cites: Default::default(),
        fichier_d_exemple: None,
        variables: vec![
            ConfigVar {
                name: "PORT".into(),
                role: ConfigRole::Port,
                default: Some("8080".into()),
                secret: false,
                source: ".env.example".into(),
                facultative: false,
                format_secret: None,
            },
            ConfigVar {
                name: "DATABASE_URL".into(),
                role: ConfigRole::DatabaseUrl,
                default: None,
                secret: false,
                source: ".env.example".into(),
                facultative: false,
                format_secret: None,
            },
            ConfigVar {
                name: "BASE_URL".into(),
                role: ConfigRole::BaseUrl,
                default: None,
                secret: false,
                source: ".env.example".into(),
                facultative: false,
                format_secret: None,
            },
        ],
    };
    let spec = ynp_spec::build(&facts).unwrap();

    assert_eq!(
        spec.runtime.env.get("BASE_URL").map(String::as_str),
        Some("https://__DOMAIN____PATH__")
    );
    // Le port et la base ne passent pas par `env` : ils ont des liaisons
    // dediees, qui savent les composer et signalent leur absence.
    assert_eq!(spec.runtime.port_binding.value().unwrap(), "PORT=__PORT__");
    assert_eq!(
        spec.runtime.database_binding.value().unwrap(),
        "DATABASE_URL=postgres://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__?sslmode=disable"
    );
    assert!(spec.is_complete());
}

#[test]
fn une_application_php_passe_par_la_brique_fpm_plutot_qu_un_service() {
    let mut facts = depot_go_avec_binaires();
    facts.stack = StackFacts {
        primary: Technology::Php,
        ..Default::default()
    };
    facts.selection.as_mut().unwrap().prebuilt.clear();

    let spec = ynp_spec::build(&facts).unwrap();
    assert!(spec.features.phpfpm);
    assert!(!spec.features.systemd);
    // Pas de service, donc pas de commande de demarrage a demander : chaque
    // application PHP sans Dockerfile posait pourtant la question.
    assert!(
        !spec
            .arbitrages()
            .iter()
            .any(|a| a.champ == "runtime.execstart"),
        "une application PHP ne doit pas faire demander sa commande de demarrage"
    );
}

#[test]
fn la_specification_fait_un_aller_retour_toml_sans_perte() {
    // C'est le fichier qu'un agent edite puis rend au pipeline.
    let spec = ynp_spec::build(&depot_go_avec_binaires()).unwrap();
    let texte = toml::to_string_pretty(&spec).unwrap();
    let relu: ynp_core::AppSpec = toml::from_str(&texte).unwrap();
    assert_eq!(spec, relu);
}

#[test]
fn une_archive_de_release_est_extraite_mais_sans_sous_repertoire() {
    // Cas de gotify : des zips plats, que le paquet officiel declare avec
    // `in_subdir = false`. Le tarball d'une forge, lui, enveloppe tout dans
    // `repo-sha/`.
    let mut facts = depot_go_avec_binaires();
    let asset = &mut facts.selection.as_mut().unwrap().prebuilt[0];
    asset.name = "gotify-linux-amd64.zip".into();
    asset.extract = true;

    let s = ynp_spec::build(&facts).unwrap().resources.sources;
    assert!(s.utilise_des_binaires());
    assert!(s.extract, "un zip s'extrait");
    assert_eq!(s.rename, None, "rien a renommer dans une archive");
    assert!(!s.in_subdir, "une archive de release est plate");
}

#[test]
fn un_binaire_nu_est_depose_sous_le_nom_de_l_application() {
    // Cas de miniflux : `miniflux-linux-amd64` devient `miniflux`.
    let s = ynp_spec::build(&depot_go_avec_binaires())
        .unwrap()
        .resources
        .sources;

    assert!(!s.extract, "un binaire nu ne s'extrait pas");
    assert_eq!(s.rename.as_deref(), Some("miniflux"));
    assert!(!s.in_subdir);
}

#[test]
fn sans_binaire_publie_le_tarball_de_la_forge_a_son_sous_repertoire() {
    let mut facts = depot_go_avec_binaires();
    facts.selection.as_mut().unwrap().prebuilt.clear();

    let s = ynp_spec::build(&facts).unwrap().resources.sources;
    assert!(!s.utilise_des_binaires());
    assert!(
        s.in_subdir,
        "le tarball d'une forge enveloppe tout dans repo-sha/"
    );
    assert!(s.extract);
}

#[test]
fn chaque_architecture_porte_son_url_et_sa_somme() {
    let s = ynp_spec::build(&depot_go_avec_binaires())
        .unwrap()
        .resources
        .sources;
    assert_eq!(s.per_arch.len(), 1);
    assert_eq!(s.per_arch[0].arch, "amd64");
    assert_eq!(s.per_arch[0].sha256.len(), 64);
    assert!(s.per_arch[0].pattern.starts_with('^'));
}

#[test]
fn un_demon_servi_par_nginx_obtient_un_port_meme_sans_expose() {
    // Cas reel de gotify : son Dockerfile ne declare aucun port, mais le
    // reverse-proxy doit bien savoir ou joindre l'application.
    let mut facts = depot_complet();
    facts.build.as_mut().unwrap().expose.clear();
    facts
        .config
        .variables
        .retain(|v| v.role != ConfigRole::Port);
    facts.compose = None;

    let spec = ynp_spec::build(&facts).unwrap();
    assert!(
        spec.resources.ports,
        "un demon derriere nginx a besoin d'un port"
    );
}

#[test]
fn une_application_php_ne_reserve_pas_de_port() {
    // php-fpm s'en charge : reserver un port serait du gaspillage.
    let mut facts = depot_complet();
    facts.build.as_mut().unwrap().expose.clear();
    facts
        .config
        .variables
        .retain(|v| v.role != ConfigRole::Port);
    facts.stack = StackFacts {
        primary: Technology::Php,
        ..Default::default()
    };

    assert!(!ynp_spec::build(&facts).unwrap().resources.ports);
}

#[test]
fn un_site_statique_non_plus() {
    let mut facts = depot_complet();
    facts.build.as_mut().unwrap().expose.clear();
    facts
        .config
        .variables
        .retain(|v| v.role != ConfigRole::Port);
    facts.stack = StackFacts {
        primary: Technology::Static,
        ..Default::default()
    };

    assert!(!ynp_spec::build(&facts).unwrap().resources.ports);
}

#[test]
fn un_port_passe_en_argument_ne_fait_rien_demander() {
    // Cas de Taiga : `gunicorn … --bind 127.0.0.1:__PORT__`. La commande
    // recoit le port en argument ; la specification demandait pourtant sous
    // quel nom le transmettre, et le generateur ignorait ensuite la reponse.
    let mut facts = depot_go_avec_binaires();
    facts.stack = StackFacts {
        primary: Technology::Python,
        commande_declaree: Some(ynp_core::facts::CommandeDeclaree {
            commande: "gunicorn app:app --bind 0.0.0.0:8000".into(),
            ou: "Procfile".into(),
        }),
        ..Default::default()
    };
    facts.build = None;
    facts.selection.as_mut().unwrap().prebuilt.clear();
    facts.config.variables.clear();

    let spec = ynp_spec::build(&facts).unwrap();
    assert!(
        spec.runtime.execstart.value().unwrap().contains("__PORT__"),
        "{:?}",
        spec.runtime.execstart
    );
    assert!(spec.resources.ports, "le port reste reserve");
    assert!(
        !spec
            .arbitrages()
            .iter()
            .any(|a| a.champ == "runtime.port_binding"),
        "le port est deja lie par la commande"
    );
}
