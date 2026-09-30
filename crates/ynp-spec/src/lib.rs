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
        // Le README en donne souvent une : on la propose, sans la retenir.
        // Un README s'ouvre aussi bien sur un bandeau que sur une phrase.
        None => Known::unresolved_avec(
            "la forge ne donne aucune description",
            &["README.md", "site du projet"],
            facts
                .meta
                .description_readme
                .as_deref()
                .map(|d| {
                    vec![Candidate::new(
                        tronquer(d, 150),
                        "premiere phrase du README",
                    )]
                })
                .unwrap_or_default(),
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
            autoupdate_version_regex: sel.and_then(|s| s.version_regex.clone()),
            per_arch: sel.map(|s| s.prebuilt.clone()).unwrap_or_default(),
            // Un binaire nu ne s'extrait pas : il est depose sous le nom de
            // l'application.
            extract: !binaire_nu,
            rename: binaire_nu.then(|| app_id.to_string()),
            // Le tarball d'une forge enveloppe tout dans `repo-sha/`. Une archive
            // de release, elle, peut etre plate (les zips de gotify) ou non
            // (`flaresolverr/flaresolverr`) : on se fie a ce qu'on en a lu, et
            // a l'ancienne hypothese — plate — seulement faute de mieux.
            in_subdir: match sel.filter(|s| s.evite_la_compilation()) {
                None => true,
                Some(s) => {
                    let lus: Vec<bool> = s
                        .prebuilt
                        .iter()
                        .filter_map(|a| a.sous_repertoire)
                        .collect();
                    !lus.is_empty() && lus.iter().all(|&r| r)
                }
            },
        },
        system_user: true,
        install_dir: true,
        // Detecte depuis toujours, jamais provisionne : Buzz n'a trouve un
        // Redis que parce qu'une autre application de l'hote de test en
        // faisait tourner un.
        redis: facts.services.needs_redis,
        s3: exige_un_stockage_s3(facts),
        // Un volume declare signale des donnees a preserver a la desinstallation.
        // Le stockage S3 embarque range aussi ses objets dans ce repertoire.
        data_dir: exige_un_stockage_s3(facts)
            || facts.build.as_ref().is_some_and(|b| !b.volumes.is_empty())
            || facts
                .compose
                .as_ref()
                .is_some_and(|c| c.services.iter().any(|s| s.is_app && !s.volumes.is_empty())),
        main_permission_url: Some("/".into()),
        ports: a_besoin_d_un_port(facts),
        port_fixe: port_fixe_du_code(facts),
        apt_packages: apt::packages(facts),
        database: facts.services.database,
        nodejs_version: version_de_resource(facts, Technology::NodeJs),
        ruby_version: version_de_resource(facts, Technology::Ruby),
        go_version: version_de_resource(facts, Technology::Go),
        composer_version: None,
    }
}

/// `corepack enable` ecrit ses raccourcis dans le repertoire de Node, qui
/// appartient a root : execute comme utilisateur de l'application, il
/// echoue. Appeler directement `corepack pnpm …` produit le meme effet sans
/// rien ecrire hors de chez l'application, ou son cache est pose.
fn sans_activation_de_corepack(etapes: Vec<String>) -> Vec<String> {
    let active = |e: &String| e.trim().starts_with("corepack enable");
    if !etapes.iter().any(active) {
        return etapes;
    }
    etapes
        .into_iter()
        .filter(|e| !active(e))
        .map(|e| {
            let mut c = e.clone();
            for outil in ["pnpm", "yarn"] {
                if c.starts_with(&format!("{outil} ")) {
                    c = format!("corepack {c}");
                }
                c = c.replace(&format!("&& {outil} "), &format!("&& corepack {outil} "));
            }
            if c.contains("corepack ") {
                format!("export COREPACK_HOME=\"$PWD/.corepack\"; {c}")
            } else {
                c
            }
        })
        .collect()
}

/// Les copies que l'image fait de ses etapes de construction vers son etape
/// finale, refaites dans le repertoire d'installation.
///
/// Sans elles, les binaires de Buzz restaient dans `target/release/`, alors
/// que le service lancait `__INSTALL_DIR__/buzz-relay` — la ou l'image les
/// range, par un `COPY --from`. Une destination hors du repertoire
/// d'installation n'est pas reproduite.
fn copies_apres_construction(facts: &RepoFacts) -> Vec<String> {
    let Some(b) = &facts.build else {
        return Vec::new();
    };
    b.copies_d_etape
        .iter()
        .filter_map(|c| {
            let vers = execstart::chemin(&c.vers);
            let vers = vers.strip_prefix("__INSTALL_DIR__/")?.to_string();
            if vers.is_empty() || vers == c.depuis {
                return None;
            }
            let parent = vers.rsplit_once('/').map(|(p, _)| p);
            // La destination est videe d'abord : a une mise a jour, elle existe
            // deja, et `cp -a dist web` creerait `web/dist`.
            Some(match parent {
                Some(p) => format!("rm -rf {vers} && mkdir -p {p} && cp -a {} {vers}", c.depuis),
                None => format!("rm -rf {vers} && cp -a {} {vers}", c.depuis),
            })
        })
        .collect()
}

/// La chaine Rust a installer, s'il y a du Rust a compiler.
///
/// Rien quand l'amont publie un binaire. Sinon la version de l'image
/// `rust:1.88` de la construction si elle en donne une, et `stable` a defaut :
/// un `rust-toolchain.toml` dans le depot l'emporte de toute facon, `rustup`
/// le lit a la premiere commande `cargo`.
fn chaine_rust(facts: &RepoFacts) -> Option<String> {
    if facts
        .selection
        .as_ref()
        .is_some_and(|s| s.evite_la_compilation())
    {
        return None;
    }
    let etapes = facts
        .build
        .as_ref()
        .map(|b| b.stages.as_slice())
        .unwrap_or_default();
    let etape_rust = etapes
        .iter()
        .find(|e| e.image.rsplit('/').next() == Some("rust"));
    let compile = facts
        .build
        .as_ref()
        .is_some_and(|b| b.build_steps.iter().any(|e| e.contains("cargo ")));
    if facts.stack.primary != Technology::Rust && !(etape_rust.is_some() && compile) {
        return None;
    }
    let version = etape_rust
        .and_then(|e| e.tag.as_deref())
        .map(|t| {
            t.chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect::<String>()
        })
        .map(|v| v.trim_end_matches('.').to_string())
        .filter(|v| v.contains('.'));
    Some(version.unwrap_or_else(|| "stable".to_string()))
}

/// Version d'un runtime qu'une etape de construction du Dockerfile utilise,
/// hors de l'etape finale. `FROM node:22-bookworm-slim AS web-builder` donne
/// Node 22 ; une image sans version exploitable ne donne rien.
fn version_d_une_etape_de_construction(facts: &RepoFacts, tech: Technology) -> Option<String> {
    let image = match tech {
        Technology::NodeJs => "node",
        Technology::Ruby => "ruby",
        Technology::Go => "golang",
        _ => return None,
    };
    let build = facts.build.as_ref()?;
    let (_, construction) = build.stages.split_last()?;
    construction
        .iter()
        .filter(|e| e.image.rsplit('/').next() == Some(image))
        .find_map(|e| {
            let tag = e.tag.as_deref()?;
            let majeure: String = tag
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            let majeure = majeure.trim_end_matches('.');
            (!majeure.is_empty()).then(|| majeure.to_string())
        })
}

/// Version a provisionner par le coeur, quand la technologie dispose d'une
/// resource. Rien pour Python : c'est tout l'objet de la regle PY001.
fn version_de_resource(facts: &RepoFacts, tech: Technology) -> Option<String> {
    if facts.stack.primary != tech {
        // Une etape de construction peut exiger un autre runtime que celui de
        // l'application : Buzz est ecrit en Rust mais construit son interface
        // dans une etape `FROM node:22`. Sans Node, `pnpm` n'existe pas.
        return version_d_une_etape_de_construction(facts, tech);
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
        // Un secret n'est genere que pour une variable que le projet fixe.
        // Buzz mentionne en commentaire une cle privee Nostr de 32 octets en
        // hexadecimal, facultative : y ecrire une chaine aleatoire l'aurait
        // rendue invalide. Il lit aussi dans son code la cle d'un autre
        // programme et celle d'un service de recherche externe.
        if var.role == ConfigRole::Secret && var.facultative {
            continue;
        }
        if let Some(valeur) = valeur_yunohost(var.role) {
            env.insert(var.name.clone(), valeur.to_string());
        }
    }

    // Les variables que le projet nomme pour joindre Redis, cablees sur la
    // base que le script d'installation lui reserve.
    if facts.services.needs_redis {
        for var in &facts.config.variables {
            let nom = var.name.to_ascii_uppercase();
            let valeur = match nom.as_str() {
                n if n == "REDIS_URL"
                    || n == "REDIS_URI"
                    || n == "REDIS_DSN"
                    || n.ends_with("_REDIS_URL") =>
                {
                    "redis://127.0.0.1:6379/__REDIS_DB__"
                }
                "REDIS_HOST" => "127.0.0.1",
                "REDIS_PORT" => "6379",
                "REDIS_DB" | "REDIS_DATABASE" => "__REDIS_DB__",
                _ => continue,
            };
            env.entry(var.name.clone())
                .or_insert_with(|| valeur.to_string());
        }
    }

    // Les variables que le projet nomme pour joindre son stockage S3, cablees
    // sur celui que le paquet embarque. Elles l'emportent sur un role plus
    // general : `AWS_SECRET_ACCESS_KEY` ressemble a un secret de
    // l'application, et une valeur tiree au hasard n'ouvrirait rien.
    if exige_un_stockage_s3(facts) {
        for var in &facts.config.variables {
            if let Some(valeur) = valeur_s3(&var.name) {
                env.insert(var.name.clone(), valeur.to_string());
            }
        }
    }

    // Une variable de l'image qui pointe vers ce qu'une etape de construction
    // y a depose suit ce depot : `BUZZ_WEB_DIR=/srv/buzz/web`, rempli par
    // `COPY --from=web-builder`, designe chez YunoHost la copie refaite dans
    // le repertoire d'installation.
    if let Some(b) = &facts.build {
        for (nom, valeur) in &b.env {
            let designe = b
                .copies_d_etape
                .iter()
                .any(|c| valeur == &c.vers || valeur.starts_with(&format!("{}/", c.vers)));
            if designe && !env.contains_key(nom) {
                env.insert(nom.clone(), execstart::chemin(valeur));
            }
        }
    }

    // PHP est servi par php-fpm et un site statique par nginx : aucun
    // service a demarrer, donc aucune commande a demander. Chaque
    // application PHP sans Dockerfile posait pourtant la question.
    let execstart = match facts.stack.primary {
        Technology::Php | Technology::Static => Known::resolved(String::new()),
        _ => avec_l_option_de_port(facts, execstart::derive(facts, app_id)),
    };

    // Les migrations que le script d'entree lancait avant son `exec` se
    // traduisent comme la commande. Une seule qui ne se traduit pas, et la
    // commande redevient une question : demarrer sur une base jamais migree
    // serait un paquet qui a l'air fini sans l'etre.
    let mut migrations = Vec::new();
    let mut intraduisible = None;
    for m in &facts.stack.migrations_du_point_d_entree {
        match execstart::traduire_avec(m, facts) {
            Some(t) => migrations.push(t),
            None => intraduisible = Some(m.clone()),
        }
    }
    let execstart = match (intraduisible, execstart.value()) {
        (Some(m), Some(_)) => Known::unresolved_avec(
            format!(
                "le script d'entree lance aussi la migration « {m} », non transposable \
                 automatiquement"
            ),
            &[
                "le script d'entree de l'image",
                "la documentation du projet d'origine",
            ],
            Vec::new(),
        ),
        _ => execstart,
    };

    Runtime {
        technology: match facts.stack.primary {
            // Rien a la racine. Si le depot porte des fichiers de projet plus
            // bas, on les propose en disant ou ils sont : a lui de trancher.
            Technology::Unknown => {
                let candidats: Vec<Candidate> = facts
                    .stack
                    .pistes
                    .iter()
                    .map(|p| Candidate::new(p.technologie.to_string(), format!("d'apres {}", p.ou)))
                    .collect();
                let raison = if let Some(langage) = &facts.stack.langage_hors_perimetre {
                    format!(
                        "le projet est ecrit en {langage}, que YunoHost ne sait pas installer \
                         par lui-meme ; aucun des langages proposes n'est celui de \
                         l'application"
                    )
                } else if candidats.is_empty() {
                    "aucun Dockerfile exploitable ni fichier de projet reconnu".to_string()
                } else {
                    format!(
                        "la racine du depot ne dit pas dans quel langage l'application est \
                         ecrite ; {} sont attestees plus bas, sans qu'aucune s'impose",
                        candidats.len()
                    )
                };
                Known::unresolved_avec(
                    raison,
                    &["README.md", "la documentation du projet d'origine"],
                    candidats,
                )
            }
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
            let mut etapes = facts
                .build
                .as_ref()
                .map(|b| b.build_steps.clone())
                .unwrap_or_default();
            etapes.extend(copies_apres_construction(facts));
            sans_activation_de_corepack(etapes)
        },
        rust_toolchain: chaine_rust(facts),
        port_env_var: facts.config.get(ConfigRole::Port).map(|v| v.name.clone()),
        port_binding: liaison_port(facts, execstart.value().map(String::as_str)),
        execstart,
        migrations,
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

/// Le port ne se transmet par aucune variable : le projet n'en lit ni n'en
/// nomme. C'est la que le code lui-meme peut repondre.
fn sans_variable_de_port(facts: &RepoFacts) -> bool {
    !matches!(facts.stack.primary, Technology::Php | Technology::Static)
        && a_besoin_d_un_port(facts)
        && facts.config.get(ConfigRole::Port).is_none()
        && variable_du_port_expose(facts).is_none()
}

/// Ajoute `--port __PORT__` a la commande quand le code definit cette option
/// et que rien d'autre ne transmet le port.
///
/// Seule `--port` est sans ambiguite : c'est un numero. `--listen`, `--bind`
/// ou `--addr` attendent selon les projets une adresse, un hote seul ou un
/// port — le `--bind` d'une application click attend `0.0.0.0`. Ceux-la ne
/// se devinent pas.
fn avec_l_option_de_port(facts: &RepoFacts, execstart: Known<String>) -> Known<String> {
    let Some(commande) = execstart.value() else {
        return execstart;
    };
    let option = facts
        .ecoute
        .options
        .iter()
        .map(|(o, _)| o.as_str())
        .find(|o| matches!(*o, "--port" | "--http-port"));
    match option {
        Some(o)
            if sans_variable_de_port(facts)
                && !commande.is_empty()
                && !commande.contains("__PORT__")
                && !commande.contains(o) =>
        {
            Known::resolved(format!("{commande} {o} __PORT__"))
        }
        _ => execstart,
    }
}

/// Le port ecrit en dur dans le code, quand c'est la seule facon dont
/// l'application le choisit : aucune variable, aucune option, un seul port
/// litteral — celui que l'image expose, si elle en expose un. RSSPub ecoute
/// ainsi sur 3000 sans aucun reglage ; le paquet doit reserver ce port-la.
pub(crate) fn port_fixe_du_code(facts: &RepoFacts) -> Option<u16> {
    if !sans_variable_de_port(facts) || !facts.ecoute.options.is_empty() {
        return None;
    }
    let mut ports: Vec<u16> = facts.ecoute.ports_en_dur.iter().map(|(p, _)| *p).collect();
    ports.sort_unstable();
    ports.dedup();
    let [p] = ports[..] else {
        return None;
    };
    let exposes = ports_exposes(facts);
    (exposes.is_empty() || exposes.contains(&p)).then_some(p)
}

/// Ligne de configuration transmettant le port reserve par le coeur.
///
/// Rien a transmettre dans deux cas qui, jusqu'ici, posaient la question :
///
/// - la commande de demarrage porte deja le port. `gunicorn --bind
///   127.0.0.1:__PORT__` le recoit en argument, pas par une variable : c'est
///   la forme ordinaire des applications Python, et Taiga voyait sa commande
///   resolue puis le port demande quand meme ;
/// - PHP et les sites statiques sont servis par php-fpm et nginx. L'`EXPOSE`
///   de leur conteneur est celui du serveur web qu'il embarque, pas celui de
///   l'application.
fn liaison_port(facts: &RepoFacts, execstart: Option<&str>) -> Known<String> {
    if matches!(facts.stack.primary, Technology::Php | Technology::Static)
        || execstart.is_some_and(|c| c.contains("__PORT__"))
        || !a_besoin_d_un_port(facts)
        || port_fixe_du_code(facts).is_some()
    {
        return Known::resolved(String::new());
    }
    // Une variable dont la valeur d'exemple porte le port expose est celle
    // du port de l'application, quel que soit son nom. Buzz : EXPOSE 3000,
    // et `BUZZ_BIND_ADDR=0.0.0.0:3000` dans son .env.example.
    if facts.config.get(ConfigRole::Port).is_none() {
        if let Some(v) = variable_du_port_expose(facts) {
            return Known::resolved(if v.default.as_deref().is_some_and(est_une_adresse) {
                format!("{}=127.0.0.1:__PORT__", v.name)
            } else {
                format!("{}=__PORT__", v.name)
            });
        }
    }
    match facts.config.get(ConfigRole::Port) {
        // Une variable qui attend une adresse — `TIMETAGGER_BIND=0.0.0.0:80` —
        // recoit une adresse : lui passer un numero seul l'empecherait de
        // demarrer. La boucle locale, parce que nginx est devant.
        Some(v) if v.default.as_deref().is_some_and(est_une_adresse) => {
            Known::resolved(format!("{}=127.0.0.1:__PORT__", v.name))
        }
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

/// Les ports que l'image declare ecouter.
pub(crate) fn ports_exposes(facts: &RepoFacts) -> Vec<u16> {
    let mut out: Vec<u16> = facts
        .build
        .as_ref()
        .map(|b| b.expose.clone())
        .unwrap_or_default();
    if let Some(c) = &facts.compose {
        for s in c.services.iter().filter(|s| s.is_app) {
            out.extend(&s.ports);
        }
    }
    out
}

/// La seule variable dont la valeur d'exemple est un port expose, ou une
/// adresse sur un port expose. Deux candidates, et l'on ne tranche pas.
fn variable_du_port_expose(facts: &RepoFacts) -> Option<&ynp_core::facts::ConfigVar> {
    let exposes = ports_exposes(facts);
    if exposes.is_empty() {
        return None;
    }
    let port_de = |d: &str| -> Option<u16> {
        if d.contains("://") {
            return None;
        }
        d.rsplit_once(':').map_or(d, |(_, p)| p).trim().parse().ok()
    };
    let mut trouvees = facts.config.variables.iter().filter(|v| {
        !est_le_port_d_un_autre_service(&v.name)
            && v.default
                .as_deref()
                .and_then(port_de)
                .is_some_and(|p| exposes.contains(&p))
    });
    let premiere = trouvees.next()?;
    trouvees.next().is_none().then_some(premiere)
}

/// Les mots d'un nom de variable qui designent un autre service que
/// l'application : `PGPORT`, `DB_PORT`, `OTEL_EXPORTER_OTLP_ENDPOINT`,
/// `METRICS_PORT`, et un nom d'en-tete HTTP.
fn est_le_port_d_un_autre_service(nom: &str) -> bool {
    const AUTRES: &[&str] = &[
        "PG",
        "PGPORT",
        "DB",
        "DATABASE",
        "POSTGRES",
        "POSTGRESQL",
        "MYSQL",
        "MARIADB",
        "REDIS",
        "VALKEY",
        "MEMCACHED",
        "MONGO",
        "MONGODB",
        "SMTP",
        "MAIL",
        "IMAP",
        "POP3",
        "LDAP",
        "OTEL",
        "OTLP",
        "METRICS",
        "PROMETHEUS",
        "HEALTH",
        "MQTT",
        "AMQP",
        "RABBITMQ",
        "S3",
        "MINIO",
        "ELASTIC",
        "ELASTICSEARCH",
        "HEADER",
        "DEBUG",
        "INSPECT",
        "ADMIN",
    ];
    nom.to_ascii_uppercase()
        .split('_')
        .any(|m| AUTRES.contains(&m))
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
        // Par mots, pas par sous-chaine : `PORT` se trouve aussi dans
        // `EXPORTER`, `REPORT` ou `SUPPORT`. Et le port d'un autre service
        // n'est pas une proposition : Buzz se voyait proposer PGPORT en tete.
        let nom = v.name.to_ascii_uppercase();
        let mots: Vec<&str> = nom.split('_').collect();
        let evoque = mots
            .iter()
            .any(|m| matches!(*m, "PORT" | "LISTEN" | "BIND" | "ADDR" | "ADDRESS"));
        if evoque && !est_le_port_d_un_autre_service(&nom) {
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
    // Une URL n'est pas une adresse d'ecoute : `http://localhost:4317` est
    // un point d'acces a joindre, et `TEEDY_URL=http://localhost:8080`
    // l'adresse publique de l'application.
    if v.contains("://") {
        return false;
    }
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

/// L'application ne fonctionne pas sans stockage S3.
///
/// Un stockage objet dans le compose de deploiement suffit a le dire. Dans un
/// compose de developpement, il peut ne servir qu'a essayer une option :
/// chibisafe range ses fichiers sur le disque, et lance minio dans son
/// `docker-compose.dev.yml` pour tester son stockage S3 facultatif. Il faut
/// alors que la configuration nomme ou joindre le stockage — Buzz exige
/// `BUZZ_S3_ENDPOINT` et refuse de demarrer sans.
fn exige_un_stockage_s3(facts: &RepoFacts) -> bool {
    if !facts.services.needs_s3 {
        return false;
    }
    let de_deploiement = facts.compose.as_ref().is_some_and(|c| {
        let nom = c
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&c.path)
            .to_ascii_lowercase();
        let marque = nom
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|m| matches!(m, "dev" | "development" | "test" | "local" | "ci" | "e2e"));
        !marque && c.services.iter().any(|s| s.is_app)
    });
    de_deploiement
        || facts.config.variables.iter().any(|v| {
            matches!(
                valeur_s3(&v.name),
                Some("http://127.0.0.1:__PORT_OBJETS__") | Some("__APP__")
            )
        })
}

/// Valeur d'une variable de stockage S3, d'apres son nom.
///
/// Le compartiment porte le nom de l'application ; la region est celle que
/// les clients S3 supposent par defaut, SeaweedFS n'en tenant pas compte.
/// Un prefixe propre au projet (`BUZZ_S3_BUCKET`) ne change rien au role.
fn valeur_s3(nom: &str) -> Option<&'static str> {
    let n = nom.to_ascii_uppercase();
    let est = |noms: &[&str], suffixes: &[&str]| {
        noms.contains(&n.as_str()) || suffixes.iter().any(|s| n.ends_with(s))
    };
    Some(
        if est(
            &[
                "S3_ENDPOINT",
                "S3_ENDPOINT_URL",
                "S3_URL",
                "AWS_ENDPOINT_URL",
                "AWS_ENDPOINT_URL_S3",
                "AWS_S3_ENDPOINT",
            ],
            &["_S3_ENDPOINT", "_S3_ENDPOINT_URL"],
        ) {
            "http://127.0.0.1:__PORT_OBJETS__"
        } else if est(
            &[
                "S3_ACCESS_KEY",
                "S3_ACCESS_KEY_ID",
                "S3_KEY",
                "AWS_ACCESS_KEY_ID",
                "AWS_ACCESS_KEY",
                "MINIO_ACCESS_KEY",
            ],
            &["_S3_ACCESS_KEY", "_S3_ACCESS_KEY_ID"],
        ) {
            "__S3_ACCESS_KEY__"
        } else if est(
            &[
                "S3_SECRET_KEY",
                "S3_SECRET_ACCESS_KEY",
                "S3_SECRET",
                "AWS_SECRET_ACCESS_KEY",
                "AWS_SECRET_KEY",
                "MINIO_SECRET_KEY",
            ],
            &["_S3_SECRET_KEY", "_S3_SECRET_ACCESS_KEY"],
        ) {
            "__S3_SECRET_KEY__"
        } else if est(
            &[
                "S3_BUCKET",
                "S3_BUCKET_NAME",
                "AWS_S3_BUCKET",
                "AWS_BUCKET",
                "AWS_BUCKET_NAME",
                "MINIO_BUCKET",
            ],
            &["_S3_BUCKET", "_S3_BUCKET_NAME"],
        ) {
            "__APP__"
        } else if est(
            &["S3_REGION", "AWS_REGION", "AWS_DEFAULT_REGION"],
            &["_S3_REGION"],
        ) {
            "us-east-1"
        } else if est(
            &[
                "S3_FORCE_PATH_STYLE",
                "S3_PATH_STYLE",
                "AWS_S3_FORCE_PATH_STYLE",
                "AWS_USE_PATH_STYLE_ENDPOINT",
            ],
            &["_S3_FORCE_PATH_STYLE", "_S3_PATH_STYLE"],
        ) {
            "true"
        } else if est(&["S3_USE_SSL", "MINIO_USE_SSL"], &["_S3_USE_SSL"]) {
            "false"
        } else {
            return None;
        },
    )
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
            facultative: false,
        }
    }

    fn buzz() -> RepoFacts {
        // Extrait reel : .env.example et EXPOSE 3000 8080 9102.
        faits(
            vec![
                var("PGPORT", Some("5432"), ".env.example"),
                var("BUZZ_BIND_ADDR", Some("0.0.0.0:3000"), ".env.example"),
                var(
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                    Some("http://localhost:4317"),
                    ".env.example",
                ),
                var("BUZZ_MEDIA_UPLOAD_PORT_HEADER", None, "code"),
            ],
            vec![3000, 8080, 9102],
        )
    }

    #[test]
    fn la_variable_qui_porte_le_port_expose_est_celle_de_l_application() {
        assert_eq!(
            liaison_port(&buzz(), None).value().map(String::as_str),
            Some("BUZZ_BIND_ADDR=127.0.0.1:__PORT__")
        );
    }

    #[test]
    fn une_url_n_est_ni_une_adresse_d_ecoute_ni_un_port() {
        // Cas de Teedy : `TEEDY_URL=http://localhost:8080`, EXPOSE 8080.
        let f = faits(
            vec![var("TEEDY_URL", Some("http://localhost:8080"), ".env")],
            vec![8080],
        );
        assert!(liaison_port(&f, None).value().is_none());
        assert!(!est_une_adresse("http://localhost:4317"));
        assert!(est_une_adresse("0.0.0.0:3000"));
    }

    #[test]
    fn corepack_s_appelle_directement_plutot_que_de_s_activer() {
        let e = sans_activation_de_corepack(vec![
            "corepack enable".into(),
            "pnpm install --frozen-lockfile".into(),
            "pnpm -C web build && pnpm -C admin build".into(),
        ]);
        assert_eq!(e.len(), 2);
        assert!(
            e[0].ends_with("corepack pnpm install --frozen-lockfile"),
            "{}",
            e[0]
        );
        assert!(e[1].contains("&& corepack pnpm -C admin build"), "{}", e[1]);
        assert!(e[0].starts_with("export COREPACK_HOME="));
    }

    #[test]
    fn les_variables_s3_se_cablent_sur_le_stockage_embarque() {
        let mut secret = var("AWS_SECRET_ACCESS_KEY", Some("changeme"), ".env.example");
        secret.role = ConfigRole::Secret;
        let vars = vec![
            var("BUZZ_S3_ENDPOINT", None, ".env.example"),
            var("S3_ACCESS_KEY_ID", None, ".env.example"),
            secret,
            var("S3_BUCKET", Some("uploads"), ".env.example"),
            var("S3_FORCE_PATH_STYLE", None, ".env.example"),
            var("S3_BATCH_SIZE", Some("50"), ".env.example"),
        ];
        let mut f = faits(vars.clone(), vec![]);
        f.services.needs_s3 = true;
        f.compose = Some(ynp_core::facts::ComposeFacts {
            path: "docker-compose.dev.yml".into(),
            services: vec![],
        });
        let r = runtime(&f, "x");
        let v = |n: &str| r.env.get(n).map(String::as_str);
        assert_eq!(
            v("BUZZ_S3_ENDPOINT"),
            Some("http://127.0.0.1:__PORT_OBJETS__")
        );
        assert_eq!(v("S3_ACCESS_KEY_ID"), Some("__S3_ACCESS_KEY__"));
        // Une cle S3 n'est pas un secret a tirer au hasard.
        assert_eq!(v("AWS_SECRET_ACCESS_KEY"), Some("__S3_SECRET_KEY__"));
        assert_eq!(v("S3_BUCKET"), Some("__APP__"));
        assert_eq!(v("S3_FORCE_PATH_STYLE"), Some("true"));
        assert_eq!(v("S3_BATCH_SIZE"), None);

        // Sans stockage embarque, rien n'est cable : les variables S3 d'un
        // projet sont souvent un stockage facultatif.
        let r = runtime(&faits(vars, vec![]), "x");
        assert_eq!(r.env.get("S3_BUCKET"), None);
    }

    #[test]
    fn les_migrations_du_script_passent_avant_le_demarrage() {
        // Cas de your_spotify : `node index.js --migrate`, puis `exec node
        // index.js`.
        let mut f = faits(vec![], vec![8080]);
        f.stack.primary = Technology::NodeJs;
        if let Some(b) = f.build.as_mut() {
            b.entrypoint = Some(vec!["sh".into(), "/app/run.sh".into()]);
        }
        f.stack.exec_du_point_d_entree = Some("node /app/build/index.js".into());
        f.stack.migrations_du_point_d_entree = vec!["node /app/build/index.js --migrate".into()];
        let r = runtime(&f, "x");
        assert_eq!(r.migrations.len(), 1, "{:?}", r.migrations);
        assert!(
            r.migrations[0].contains("index.js --migrate"),
            "{:?}",
            r.migrations
        );
        assert!(
            r.migrations[0].starts_with("__NODEJS_DIR__/node"),
            "{:?}",
            r.migrations
        );
        assert!(r.execstart.value().is_some());
    }

    #[test]
    fn un_port_ecrit_en_dur_se_reserve_tel_quel() {
        // Cas de RSSPub : `SocketAddr::from(([0, 0, 0, 0], 3000))`, EXPOSE 3000.
        let mut f = faits(vec![], vec![3000]);
        f.stack.primary = Technology::Rust;
        f.ecoute.ports_en_dur = vec![(3000, "src/main.rs".into())];
        assert_eq!(port_fixe_du_code(&f), Some(3000));
        assert_eq!(
            liaison_port(&f, Some("__INSTALL_DIR__/rsspub"))
                .value()
                .map(String::as_str),
            Some("")
        );

        // Un port en dur qui n'est pas celui de l'image : autre chose ecoute.
        f.ecoute.ports_en_dur = vec![(9090, "src/metrics.rs".into())];
        assert_eq!(port_fixe_du_code(&f), None);

        // Une variable de port lue par le code l'emporte.
        f.ecoute.ports_en_dur = vec![(3000, "src/main.rs".into())];
        let mut port = var("PORT", Some("3000"), "src/main.rs");
        port.role = ConfigRole::Port;
        f.config.variables = vec![port];
        assert_eq!(port_fixe_du_code(&f), None);
    }

    #[test]
    fn l_option_port_du_code_s_ajoute_a_la_commande() {
        let mut f = faits(vec![], vec![]);
        f.stack.primary = Technology::Go;
        f.ecoute.options = vec![("--port".into(), "main.go".into())];
        let e = avec_l_option_de_port(&f, Known::resolved("__INSTALL_DIR__/app serve".into()));
        assert_eq!(
            e.value().map(String::as_str),
            Some("__INSTALL_DIR__/app serve --port __PORT__")
        );

        // `--listen` ne dit pas s'il attend un port, un hote ou une adresse.
        f.ecoute.options = vec![("--listen".into(), "main.go".into())];
        let e = avec_l_option_de_port(&f, Known::resolved("__INSTALL_DIR__/app".into()));
        assert_eq!(e.value().map(String::as_str), Some("__INSTALL_DIR__/app"));
    }

    #[test]
    fn un_stockage_objet_de_dev_sans_variable_s3_n_est_pas_exige() {
        // Cas de chibisafe : minio dans docker-compose.dev.yml, aucune
        // variable pour le joindre.
        let app = ynp_core::facts::ComposeService {
            name: "chibisafe".into(),
            is_app: true,
            ..Default::default()
        };
        let mut f = faits(vec![], vec![]);
        f.services.needs_s3 = true;
        f.compose = Some(ynp_core::facts::ComposeFacts {
            path: "docker-compose.dev.yml".into(),
            services: vec![app.clone()],
        });
        assert!(!exige_un_stockage_s3(&f));

        // Le meme compose, de deploiement cette fois, suffit.
        f.compose = Some(ynp_core::facts::ComposeFacts {
            path: "docker-compose.yml".into(),
            services: vec![app],
        });
        assert!(exige_un_stockage_s3(&f));
    }

    #[test]
    fn un_secret_seulement_mentionne_n_est_pas_genere() {
        // Cas de Buzz : une cle Nostr en commentaire dans l'exemple.
        let mut facultative = var("BUZZ_RELAY_PRIVATE_KEY", None, ".env.example");
        facultative.role = ConfigRole::Secret;
        facultative.facultative = true;
        let mut fixee = var("SECRET_KEY", Some("changeme"), ".env.example");
        fixee.role = ConfigRole::Secret;
        let r = runtime(&faits(vec![facultative, fixee], vec![]), "x");
        assert!(!r.env.contains_key("BUZZ_RELAY_PRIVATE_KEY"));
        assert_eq!(
            r.env.get("SECRET_KEY").map(String::as_str),
            Some("__SECRET__")
        );
    }

    #[test]
    fn deux_variables_sur_des_ports_exposes_ne_se_departagent_pas() {
        let mut f = buzz();
        f.config
            .variables
            .push(var("WEB_ADDR", Some("0.0.0.0:8080"), ".env.example"));
        assert!(liaison_port(&f, None).value().is_none());
    }

    #[test]
    fn un_mot_ne_se_confond_pas_avec_une_sous_chaine_ni_un_autre_service() {
        let c = candidats_port(&buzz());
        for absent in ["PGPORT", "OTEL_EXPORTER", "PORT_HEADER"] {
            assert!(
                !c.iter().any(|x| x.value.contains(absent)),
                "{absent} : {c:?}"
            );
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
