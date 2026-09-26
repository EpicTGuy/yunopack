//! Faits + faisabilite -> `AppSpec`.
//!
//! **Le seul etage du pipeline qui decide.** En amont, `analyze` collecte sans
//! interpreter ; en aval, `generate` rend des templates sans rien trancher.
//!
//! Tout ce qui ne se deduit pas devient un [`Known::Unresolved`] portant sa
//! raison et l'endroit ou chercher. C'est ce qui permet de se passer d'un
//! modele de langage sans jamais livrer un paquet devine : soit l'outil sait,
//! soit il pointe precisement ce qu'il ignore.

pub mod apt;
pub mod execstart;

use ynp_core::facts::{ConfigRole, Database, RepoFacts, Technology};
use ynp_core::known::{Candidate, Known};
use ynp_core::spec::*;

/// Construit la specification a partir des faits.
pub fn build(facts: &RepoFacts) -> Result<AppSpec, ynp_core::CoreError> {
    let app_id = ynp_core::app_id_from(Some(&facts.source.owner), &facts.source.repo)?;

    Ok(AppSpec {
        schema_version: SPEC_SCHEMA_VERSION,
        app: identite(facts, &app_id),
        upstream: upstream(facts),
        integration: integration(facts),
        install: questions(),
        resources: resources(facts, &app_id),
        runtime: runtime(facts, &app_id),
        features: features(facts),
        docs: docs(facts),
    })
}

fn identite(facts: &RepoFacts, app_id: &str) -> AppIdentity {
    AppIdentity {
        // Le nom affiche derive de l'identifiant, et non du depot : « miniflux »
        // plutot que « V2 ». Limite a 23 caracteres par le linter.
        name: tronquer(&titre(app_id), 23),
        id: app_id.to_string(),
        description_en: description(facts),
        description_fr: None,
        version: match facts.selection.as_ref().and_then(|s| s.version.clone()) {
            Some(v) => Known::resolved(v),
            // Sans repere de version, l'infrastructure YunoHost datera la
            // release du commit ; il faut alors trancher a la main.
            None => Known::unresolved(
                "ni release ni tag de version : la version devra etre datee du commit",
                &["releases de la forge", "CHANGELOG.md"],
            ),
        },
        maintainers: Vec::new(),
    }
}

/// La description du catalogue est limitee a 150 caracteres.
///
/// Repli deterministe : la description de la forge, faute de quoi le champ
/// reste a completer. On ne fabrique pas de prose.
fn description(facts: &RepoFacts) -> Known<String> {
    match facts
        .meta
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        Some(d) => Known::resolved(tronquer(d, 150)),
        None => Known::unresolved(
            "la forge ne donne aucune description",
            &["README.md", "site du projet"],
        ),
    }
}

fn upstream(facts: &RepoFacts) -> Upstream {
    Upstream {
        license: match &facts.meta.license_spdx {
            Some(l) => Known::resolved(l.clone()),
            None => Known::unresolved(
                "licence non identifiee par la forge",
                &["LICENSE", "COPYING", "README.md"],
            ),
        },
        website: facts.meta.homepage.clone(),
        code: Some(facts.source.url.clone()),
        demo: None,
        admindoc: None,
        userdoc: None,
    }
}

fn integration(facts: &RepoFacts) -> Integration {
    let prebuilt = facts
        .selection
        .as_ref()
        .map(|s| s.prebuilt.as_slice())
        .unwrap_or(&[]);

    Integration {
        // Des binaires publies par architecture bornent ce qu'on peut installer :
        // annoncer « all » masquerait l'app sur les plateformes non couvertes.
        architectures: if prebuilt.is_empty() {
            Architectures::All
        } else {
            Architectures::Only(prebuilt.iter().map(|a| a.arch.clone()).collect())
        },
        // L'integration a l'annuaire et au portail demande du travail cote
        // application : on ne l'affirme jamais sans preuve.
        ldap: Triple::No,
        sso: Triple::No,
        ram_build: if facts
            .selection
            .as_ref()
            .is_some_and(|s| s.evite_la_compilation())
        {
            // Rien a compiler : les paquets de reference annoncent 50M.
            "50M".into()
        } else {
            "500M".into()
        },
        ..Integration::default()
    }
}

fn questions() -> InstallQuestions {
    InstallQuestions {
        // On reste sur le cas courant. Les applications exigeant un domaine
        // dedie sont minoritaires et ne se reconnaissent a aucun signal fiable :
        // c'est a la relecture de l'appspec de trancher.
        url_scheme: UrlScheme::DomainAndPath,
        init_main_permission: "all_users".into(),
        extra: Vec::new(),
    }
}

fn resources(facts: &RepoFacts, app_id: &str) -> Resources {
    let sel = facts.selection.as_ref();
    let binaire_nu = sel.is_some_and(|s| s.prebuilt.iter().any(|a| !a.extract));

    Resources {
        sources: Sources {
            url: match sel {
                Some(s) => Known::resolved(s.url.clone()),
                None => Known::unresolved("source non determinee", &["releases de la forge"]),
            },
            sha256: match sel {
                Some(s) => Known::resolved(s.sha256.clone()),
                None => Known::unresolved("somme de controle non calculee", &[]),
            },
            autoupdate_strategy: sel.map(|s| s.strategy.clone()),
            per_arch: sel.map(|s| s.prebuilt.clone()).unwrap_or_default(),
            // Un binaire nu ne s'extrait pas : il est depose sous le nom de
            // l'application.
            extract: !binaire_nu,
            rename: binaire_nu.then(|| app_id.to_string()),
            // Une archive de release est plate, contrairement au tarball d'une
            // forge qui enveloppe tout dans `repo-sha/`. Le paquet officiel de
            // gotify declare ainsi `in_subdir = false` sur ses zips.
            in_subdir: !sel.is_some_and(|s| s.evite_la_compilation()),
        },
        system_user: true,
        install_dir: true,
        // Un volume declare signale des donnees a preserver a la desinstallation.
        data_dir: facts.build.as_ref().is_some_and(|b| !b.volumes.is_empty())
            || facts
                .compose
                .as_ref()
                .is_some_and(|c| c.services.iter().any(|s| s.is_app && !s.volumes.is_empty())),
        main_permission_url: Some("/".into()),
        ports: a_besoin_d_un_port(facts),
        apt_packages: apt::packages(facts),
        database: facts.services.database,
        nodejs_version: version_de_resource(facts, Technology::NodeJs),
        ruby_version: version_de_resource(facts, Technology::Ruby),
        go_version: version_de_resource(facts, Technology::Go),
        composer_version: None,
    }
}

/// Version a provisionner par le coeur, quand la technologie dispose d'une
/// resource. Rien pour Python : c'est tout l'objet de la regle PY001.
fn version_de_resource(facts: &RepoFacts, tech: Technology) -> Option<String> {
    if facts.stack.primary != tech {
        return None;
    }
    // Un binaire deja construit n'a pas besoin de son runtime a la compilation.
    if facts
        .selection
        .as_ref()
        .is_some_and(|s| s.evite_la_compilation())
        && matches!(tech, Technology::Go | Technology::Ruby)
    {
        return None;
    }
    tech.manifest_resource()?;
    facts.stack.runtime_version.clone()
}

fn runtime(facts: &RepoFacts, app_id: &str) -> Runtime {
    let mut env = indexmap::IndexMap::new();

    // Les variables dont le role est reconnu se cablent sur les valeurs
    // fournies par YunoHost ; les autres restent a la main de l'administrateur.
    for var in &facts.config.variables {
        if let Some(valeur) = valeur_yunohost(var.role) {
            env.insert(var.name.clone(), valeur.to_string());
        }
    }

    Runtime {
        technology: match facts.stack.primary {
            Technology::Unknown => Known::unresolved(
                "aucun Dockerfile exploitable ni fichier de projet reconnu",
                &["README.md", "la documentation du projet d'origine"],
            ),
            t => Known::resolved(t),
        },
        build_steps: if facts
            .selection
            .as_ref()
            .is_some_and(|s| s.evite_la_compilation())
        {
            // Rien a construire : l'amont publie un binaire.
            Vec::new()
        } else {
            facts
                .build
                .as_ref()
                .map(|b| b.build_steps.clone())
                .unwrap_or_default()
        },
        execstart: execstart::derive(facts, app_id),
        port_env_var: facts.config.get(ConfigRole::Port).map(|v| v.name.clone()),
        port_binding: liaison_port(facts),
        database_binding: liaison_base(facts),
        // Un fichier de configuration est necessaire des qu'il y a quelque
        // chose a transmettre a l'application, meme si l'amont n'en publie
        // aucun exemple. C'est ce manque qui a fait installer un miniflux
        // incapable de demarrer.
        config_file: (facts.config.example_file.is_some()
            || facts.services.database != Database::None
            || !env.is_empty())
        .then(|| ".env".to_string()),
        env,
    }
}

/// Ligne de configuration transmettant le port reserve par le coeur.
fn liaison_port(facts: &RepoFacts) -> Known<String> {
    if !a_besoin_d_un_port(facts) {
        return Known::resolved(String::new());
    }
    match facts.config.get(ConfigRole::Port) {
        Some(v) => Known::resolved(format!("{}=__PORT__", v.name)),
        None => Known::unresolved_avec(
            "on ignore sous quel nom l'application attend son port d'ecoute ; la forme varie \
             d'une application a l'autre (PORT=__PORT__, LISTEN_ADDR=127.0.0.1:__PORT__...)",
            &[
                "la documentation du projet d'origine",
                "sortie de --help du binaire",
            ],
            candidats_port(facts),
        ),
    }
}

/// Formes plausibles de la liaison du port, la mieux fondee en premier.
///
/// Les deux premieres sources sont des faits du depot ; les suivantes sont les
/// conventions les plus repandues, proposees en dernier et signalees comme
/// telles. Aucune ne s'applique sans decision.
fn candidats_port(facts: &RepoFacts) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut ajouter = |valeur: String, pourquoi: String| {
        if !out.iter().any(|c: &Candidate| c.value == valeur) {
            out.push(Candidate::new(valeur, pourquoi));
        }
    };

    // Une variable dont le nom evoque le port sans avoir ete classee comme
    // telle : le depot la nomme, c'est le meilleur indice disponible.
    for v in &facts.config.variables {
        let nom = v.name.to_ascii_uppercase();
        if nom.contains("PORT") || nom.contains("LISTEN") || nom.contains("BIND") {
            let source = if v.source.is_empty() {
                "nommee dans la configuration du projet".to_string()
            } else {
                format!("nommee dans {}", v.source)
            };
            // La valeur d'exemple dit la forme attendue : une variable qui vaut
            // `0.0.0.0:8080` veut une adresse, pas un numero. Constate sur
            // miniflux, dont le LISTEN_ADDR reduit au seul port ne marche pas.
            if let Some(d) = v.default.as_deref() {
                if est_une_adresse(d) {
                    ajouter(
                        format!("{}=127.0.0.1:__PORT__", v.name),
                        format!("{source}, avec la valeur « {d} » pour modele"),
                    );
                    continue;
                }
            }
            ajouter(format!("{}=__PORT__", v.name), source);
        }
    }

    // Le Dockerfile publie un EXPOSE : l'application ecoute donc bien sur un
    // port, meme si son nom reste a trouver.
    if let Some(expose) = facts.build.as_ref().and_then(|b| b.expose.first()) {
        ajouter(
            "PORT=__PORT__".to_string(),
            format!("EXPOSE {expose} dans le Dockerfile, sans nom de variable"),
        );
    }

    for (valeur, pourquoi) in [
        ("PORT=__PORT__", "forme la plus repandue"),
        ("HTTP_PORT=__PORT__", "variante courante"),
        (
            "LISTEN_ADDR=127.0.0.1:__PORT__",
            "applications Go qui prennent une adresse complete",
        ),
        (
            "SERVER_PORT=__PORT__",
            "variante courante des applications Java et Go",
        ),
    ] {
        ajouter(valeur.to_string(), pourquoi.to_string());
    }
    out
}

/// Ligne de configuration transmettant les identifiants de la base.
///
/// La chaine de connexion se compose de facon deterministe a partir des
/// reglages que le coeur fournit ; seul le *nom* sous lequel l'application
/// l'attend reste inconnu quand l'amont ne publie aucun exemple.
fn liaison_base(facts: &RepoFacts) -> Known<String> {
    let adresse = match facts.services.database {
        Database::PostgreSql => {
            "postgres://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__?sslmode=disable"
        }
        Database::MySql => "mysql://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__",
        // Rien a transmettre : pas de base, ou une base fichier.
        _ => return Known::resolved(String::new()),
    };

    if let Some(v) = facts.config.get(ConfigRole::DatabaseUrl) {
        return Known::resolved(format!("{}={adresse}", v.name));
    }

    // Certaines applications attendent les champs separement plutot qu'une URL.
    let champs: Vec<String> = [
        (ConfigRole::DatabaseHost, "127.0.0.1"),
        (ConfigRole::DatabaseName, "__DB_NAME__"),
        (ConfigRole::DatabaseUser, "__DB_USER__"),
        (ConfigRole::DatabasePassword, "__DB_PWD__"),
    ]
    .iter()
    .filter_map(|(role, valeur)| {
        facts
            .config
            .get(*role)
            .map(|v| format!("{}={valeur}", v.name))
    })
    .collect();

    if !champs.is_empty() {
        return Known::resolved(champs.join("\n"));
    }

    Known::unresolved_avec(
        format!(
            "une base est provisionnee mais on ignore sous quel nom l'application attend son \
             adresse ; la valeur a transmettre est « {adresse} »"
        ),
        &[
            "la documentation du projet d'origine",
            "le depot ne publie aucun exemple de configuration",
        ],
        candidats_base(facts, adresse),
    )
}

/// Vrai pour une valeur de la forme `hote:port`, y compris `:8080`.
fn est_une_adresse(v: &str) -> bool {
    match v.rsplit_once(':') {
        Some((_, port)) => !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Formes plausibles de la liaison de la base, la mieux fondee en premier.
fn candidats_base(facts: &RepoFacts, adresse: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut ajouter = |valeur: String, pourquoi: String| {
        if !out.iter().any(|c: &Candidate| c.value == valeur) {
            out.push(Candidate::new(valeur, pourquoi));
        }
    };

    for v in &facts.config.variables {
        let nom = v.name.to_ascii_uppercase();
        if nom.contains("DATABASE") || nom.contains("_DB") || nom.starts_with("DB") {
            let source = if v.source.is_empty() {
                "nommee dans la configuration du projet".to_string()
            } else {
                format!("nommee dans {}", v.source)
            };
            ajouter(format!("{}={adresse}", v.name), source);
        }
    }

    for (nom, pourquoi) in [
        ("DATABASE_URL", "forme la plus repandue"),
        ("DB_URL", "variante courante"),
        ("DATABASE_URI", "variante courante"),
    ] {
        ajouter(format!("{nom}={adresse}"), pourquoi.to_string());
    }

    // Certaines applications veulent les champs separement plutot qu'une URL.
    ajouter(
        "DB_HOST=127.0.0.1\nDB_NAME=__DB_NAME__\nDB_USER=__DB_USER__\nDB_PASSWORD=__DB_PWD__"
            .to_string(),
        "applications qui attendent les champs separement plutot qu'une URL".to_string(),
    );
    out
}

/// Valeur YunoHost correspondant a un role de configuration.
///
/// Les placeholders sont remplaces par les helpers au moment ou le fichier est
/// installe : c'est le mecanisme de templating natif, pas une invention.
fn valeur_yunohost(role: ConfigRole) -> Option<&'static str> {
    Some(match role {
        ConfigRole::BaseUrl => "https://__DOMAIN____PATH__",
        ConfigRole::DataPath => "__DATA_DIR__",
        ConfigRole::Secret => "__SECRET__",
        // Le port et la base passent par les liaisons dediees, qui savent les
        // composer et signalent leur absence.
        _ => return None,
    })
}

fn features(facts: &RepoFacts) -> Features {
    let php = facts.stack.primary == Technology::Php;
    // Un service systemd n'a de sens que si l'on sait quoi demarrer.
    let daemon = !php && facts.stack.primary != Technology::Static;

    Features {
        nginx: true,
        systemd: daemon,
        phpfpm: php,
        logrotate: daemon,
        // Les motifs de detection d'intrusion demandent de connaitre le format
        // des journaux de l'application : on ne les invente pas.
        fail2ban: false,
        cron: false,
        change_url: true,
        service_integration: daemon,
    }
}

fn docs(facts: &RepoFacts) -> Docs {
    Docs {
        description: facts.meta.description.clone(),
        pre_install: None,
        post_install: None,
        admin: None,
        license_text: facts.meta.license_text.clone(),
    }
}

/// Vrai si l'application a besoin d'un port reserve par le coeur.
///
/// Un `EXPOSE` ou un port dans le compose suffit a le dire. Mais un demon
/// servi par nginx en a besoin de toute facon : c'est par la que passe le
/// reverse-proxy. Constate sur gotify, dont le Dockerfile ne declare aucun
/// port alors que le paquet officiel en reserve un.
fn a_besoin_d_un_port(facts: &RepoFacts) -> bool {
    if expose_un_port(facts) {
        return true;
    }
    // Un demon a servir : ni du PHP (pris en charge par php-fpm), ni des
    // fichiers statiques (servis directement par nginx).
    !matches!(
        facts.stack.primary,
        Technology::Php | Technology::Static | Technology::Unknown
    )
}

fn expose_un_port(facts: &RepoFacts) -> bool {
    facts.build.as_ref().is_some_and(|b| !b.expose.is_empty())
        || facts
            .compose
            .as_ref()
            .is_some_and(|c| c.services.iter().any(|s| s.is_app && !s.ports.is_empty()))
        || facts
            .config
            .get(ynp_core::facts::ConfigRole::Port)
            .is_some()
}

/// Nom affichable tire du nom de depot : `uptime-kuma` -> `Uptime Kuma`.
fn titre(repo: &str) -> String {
    repo.trim_end_matches("_ynh")
        .split(['-', '_', '.'])
        .filter(|m| !m.is_empty())
        .map(|mot| {
            let mut c = mot.chars();
            match c.next() {
                Some(p) => p.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn tronquer(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // Couper sur un mot plutot qu'au milieu, et signaler la coupe : une phrase
    // qui s'arrete net laisse croire a un texte tronque par accident.
    let court: String = s.chars().take(max - 1).collect();
    let coupe = match court.rfind(' ') {
        Some(i) if i > max / 2 => &court[..i],
        _ => court.as_str(),
    };
    format!("{}…", coupe.trim_end_matches([',', ';', ':', ' ']))
}

#[cfg(test)]
mod propositions {
    use super::*;
    use ynp_core::facts::{BuildRecipe, ConfigFacts, ConfigVar};

    fn faits(variables: Vec<ConfigVar>, expose: Vec<u16>) -> RepoFacts {
        RepoFacts {
            config: ConfigFacts {
                example_file: Some(".env.example".into()),
                variables,
            },
            build: Some(BuildRecipe {
                expose,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn var(nom: &str, defaut: Option<&str>, source: &str) -> ConfigVar {
        ConfigVar {
            name: nom.into(),
            default: defaut.map(str::to_string),
            role: ConfigRole::Other,
            secret: false,
            source: source.into(),
        }
    }

    #[test]
    fn une_variable_du_depot_passe_avant_les_conventions() {
        let c = candidats_port(&faits(
            vec![var("APP_LISTEN_PORT", None, "Dockerfile")],
            vec![],
        ));
        assert_eq!(c[0].value, "APP_LISTEN_PORT=__PORT__");
        assert!(c[0].why.contains("Dockerfile"));
        // Les conventions restent proposees, mais apres.
        assert!(c.iter().any(|c| c.value == "PORT=__PORT__"));
    }

    #[test]
    fn une_valeur_d_exemple_en_hote_port_fait_proposer_une_adresse() {
        // miniflux attend `LISTEN_ADDR=127.0.0.1:8080`, pas un numero seul.
        let c = candidats_port(&faits(
            vec![var("LISTEN_ADDR", Some("0.0.0.0:8080"), "Dockerfile")],
            vec![],
        ));
        assert_eq!(c[0].value, "LISTEN_ADDR=127.0.0.1:__PORT__");
        assert!(c[0].why.contains("0.0.0.0:8080"));
    }

    #[test]
    fn un_expose_fonde_la_proposition_generique() {
        let c = candidats_port(&faits(vec![], vec![8080]));
        assert_eq!(c[0].value, "PORT=__PORT__");
        assert!(c[0].why.contains("EXPOSE 8080"));
    }

    #[test]
    fn aucune_proposition_n_est_repetee() {
        let c = candidats_port(&faits(vec![var("PORT", None, "x")], vec![3000]));
        let mut vus: Vec<&str> = c.iter().map(|c| c.value.as_str()).collect();
        let avant = vus.len();
        vus.sort_unstable();
        vus.dedup();
        assert_eq!(vus.len(), avant);
    }

    #[test]
    fn la_base_propose_la_variable_du_depot_avant_les_conventions() {
        let f = faits(vec![var("APP_DATABASE_DSN", None, "compose.yml")], vec![]);
        let c = candidats_base(&f, "postgres://x");
        assert_eq!(c[0].value, "APP_DATABASE_DSN=postgres://x");
        assert!(c.iter().any(|c| c.value == "DATABASE_URL=postgres://x"));
    }

    #[test]
    fn la_base_propose_aussi_la_forme_en_champs_separes() {
        let c = candidats_base(&faits(vec![], vec![]), "postgres://x");
        assert!(c.iter().any(|c| c.value.contains("DB_HOST=127.0.0.1")));
    }

    #[test]
    fn une_adresse_se_distingue_d_un_simple_numero() {
        assert!(est_une_adresse("0.0.0.0:8080"));
        assert!(est_une_adresse(":8080"));
        assert!(!est_une_adresse("8080"));
        assert!(!est_une_adresse("localhost"));
        assert!(!est_une_adresse("host:"));
    }
}

/// Complete les arbitrages avec les fichiers du depot ou chercher la reponse.
///
/// « Chercher dans la documentation » sans dire ou oblige a aller fouiller
/// soi-meme. Ces fichiers-la existent vraiment dans ce depot-ci, et leur
/// adresse mene directement a la page.
pub fn ajouter_les_pistes(
    arbitrages: &mut [ynp_core::spec::Arbitrage],
    faits: &RepoFacts,
    arbre: &ynp_core::tree::RepoTree,
) {
    use ynp_core::spec::Piste;

    let base = adresse_de_consultation(faits);
    let existe = |chemin: &str| arbre.paths().iter().any(|p| p == chemin);
    let piste = |chemin: &str| Piste {
        chemin: chemin.to_string(),
        url: base
            .as_ref()
            .map(|b| format!("{b}/{chemin}"))
            .unwrap_or_default(),
    };

    // Les fichiers qui repondent le plus souvent, par champ. Seuls ceux qui
    // existent reellement sont proposes : un lien mort vaut moins que rien.
    for a in arbitrages.iter_mut() {
        let candidats: &[&str] = match a.champ.as_str() {
            "runtime.port_binding" | "runtime.database_binding" => &[
                ".env.example",
                ".env.sample",
                "docker-compose.yml",
                "compose.yml",
                "docs/configuration.md",
                "README.md",
            ],
            "runtime.execstart" => &[
                "Dockerfile",
                "Procfile",
                "docs/installation.md",
                "docs/deployment.md",
                "README.md",
            ],
            "runtime.technology" => &[
                "package.json",
                "go.mod",
                "Cargo.toml",
                "pyproject.toml",
                "composer.json",
                "README.md",
            ],
            "upstream.license" => &["LICENSE", "LICENSE.md", "COPYING"],
            _ => &["README.md"],
        };
        a.pistes = candidats
            .iter()
            .filter(|c| existe(c))
            .map(|c| piste(c))
            .collect();

        // Le fichier d'exemple trouve par l'analyse passe devant : c'est celui
        // ou la reponse est le plus probable.
        if let Some(exemple) = &faits.config.example_file {
            if existe(exemple) && !a.pistes.iter().any(|p| &p.chemin == exemple) {
                a.pistes.insert(0, piste(exemple));
            }
        }
        // Le Dockerfile retenu, de meme, quand il n'est pas a la racine.
        if let Some(b) = &faits.build {
            if existe(&b.dockerfile_path) && !a.pistes.iter().any(|p| p.chemin == b.dockerfile_path)
            {
                a.pistes.push(piste(&b.dockerfile_path));
            }
        }
        a.pistes.truncate(5);
    }
}

/// Adresse ou lire un fichier du depot, a la reference retenue.
fn adresse_de_consultation(faits: &RepoFacts) -> Option<String> {
    let s = &faits.source;
    if s.url.is_empty() {
        return None;
    }
    let reference = s.commit.clone().or_else(|| s.default_branch.clone())?;
    let base = s.url.trim_end_matches('/');
    Some(match s.forge {
        ynp_core::facts::Forge::GitLab => format!("{base}/-/blob/{reference}"),
        _ => format!("{base}/blob/{reference}"),
    })
}

#[cfg(test)]
mod pistes {
    use super::*;
    use ynp_core::facts::Forge;
    use ynp_core::spec::Arbitrage;
    use ynp_core::tree::RepoTree;

    fn arbitrage(champ: &str) -> Arbitrage {
        Arbitrage {
            champ: champ.into(),
            raison: String::new(),
            ou_chercher: Vec::new(),
            candidats: Vec::new(),
            choix: Vec::new(),
            pistes: Vec::new(),
        }
    }

    fn faits(forge: Forge) -> RepoFacts {
        let mut f = RepoFacts::default();
        f.source.forge = forge;
        f.source.url = "https://github.com/a/b".into();
        f.source.commit = Some("v1.2".into());
        f
    }

    #[test]
    fn seuls_les_fichiers_presents_sont_proposes() {
        // Un lien mort vaut moins que rien.
        let arbre = RepoTree::from_pairs([("README.md", "x"), ("Dockerfile", "y")]);
        let mut a = vec![arbitrage("runtime.execstart")];
        ajouter_les_pistes(&mut a, &faits(Forge::GitHub), &arbre);

        let chemins: Vec<&str> = a[0].pistes.iter().map(|p| p.chemin.as_str()).collect();
        assert!(chemins.contains(&"Dockerfile"));
        assert!(chemins.contains(&"README.md"));
        assert!(!chemins.iter().any(|c| c.starts_with("docs/")));
    }

    #[test]
    fn l_adresse_mene_a_la_reference_retenue() {
        let arbre = RepoTree::from_pairs([("README.md", "x")]);
        let mut a = vec![arbitrage("upstream.license")];
        ajouter_les_pistes(&mut a, &faits(Forge::GitHub), &arbre);
        // Pas de LICENSE dans l'arbre, donc seul le repli reste.
        let mut b = vec![arbitrage("runtime.technology")];
        ajouter_les_pistes(&mut b, &faits(Forge::GitHub), &arbre);
        assert_eq!(
            b[0].pistes[0].url,
            "https://github.com/a/b/blob/v1.2/README.md"
        );
        assert!(a[0].pistes.is_empty() || a[0].pistes[0].chemin == "README.md");
    }

    #[test]
    fn gitlab_a_sa_propre_forme_d_adresse() {
        let arbre = RepoTree::from_pairs([("README.md", "x")]);
        let mut a = vec![arbitrage("runtime.technology")];
        ajouter_les_pistes(&mut a, &faits(Forge::GitLab), &arbre);
        assert!(a[0].pistes[0].url.contains("/-/blob/v1.2/"));
    }

    #[test]
    fn le_fichier_d_exemple_trouve_passe_devant() {
        let arbre = RepoTree::from_pairs([("README.md", "x"), ("app.env.example", "PORT=1")]);
        let mut f = faits(Forge::GitHub);
        f.config.example_file = Some("app.env.example".into());
        let mut a = vec![arbitrage("runtime.port_binding")];
        ajouter_les_pistes(&mut a, &f, &arbre);
        assert_eq!(a[0].pistes[0].chemin, "app.env.example");
    }

    #[test]
    fn sans_depot_connu_les_pistes_restent_des_chemins() {
        // Mieux vaut un nom de fichier sans lien qu'aucune indication.
        let arbre = RepoTree::from_pairs([("README.md", "x")]);
        let mut f = RepoFacts::default();
        f.source.url = String::new();
        let mut a = vec![arbitrage("runtime.technology")];
        ajouter_les_pistes(&mut a, &f, &arbre);
        assert_eq!(a[0].pistes[0].chemin, "README.md");
        assert!(a[0].pistes[0].url.is_empty());
    }
}
