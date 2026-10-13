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
mod entree;
pub mod execstart;
mod java;
mod php;
mod statique;

use ynp_core::facts::{ConfigRole, Database, RepoFacts, Technology};
use ynp_core::known::{Candidate, Known};
use ynp_core::spec::*;

/// Construit la specification a partir des faits.
pub fn build(facts: &RepoFacts) -> Result<AppSpec, ynp_core::CoreError> {
    let app_id = ynp_core::app_id_from(Some(&facts.source.owner), &facts.source.repo)?;

    let mut runtime = runtime(facts, &app_id);
    let mut resources = resources(facts, &app_id);
    outils_des_etapes(facts, &mut runtime, &mut resources);
    if let Some(d) = &facts.stack.sous_repertoire {
        dans_le_sous_repertoire(&mut runtime, d);
    }
    // Un reglage qui designe le repertoire de donnees l'exige : sans lui,
    // YunoHost s'arrete sur « Variable $data_dir wasn't initialized »
    // (endurain).
    if !resources.data_dir && designe_les_donnees(&runtime) {
        resources.data_dir = true;
    }

    Ok(AppSpec {
        schema_version: SPEC_SCHEMA_VERSION,
        app: identite(facts, &app_id),
        upstream: upstream(facts),
        integration: integration(facts),
        install: questions(facts),
        resources,
        runtime,
        features: features(facts),
        docs: docs(facts),
    })
}

/// Le serveur d'un monorepo vit dans un sous-repertoire de l'installation
/// (`backend`) : les etapes s'y lancent, les chemins de l'installation que
/// la commande designe y sont ramenes, et le service y travaille. Le venv,
/// cree a la racine, reste ou il est.
fn dans_le_sous_repertoire(r: &mut Runtime, d: &str) {
    let ramener = |c: &str| {
        c.replace("__INSTALL_DIR__/venv/", "\u{0}")
            .replace("__INSTALL_DIR__/", &format!("__INSTALL_DIR__/{d}/"))
            .replace('\u{0}', "__INSTALL_DIR__/venv/")
    };
    r.build_steps = r
        .build_steps
        .iter()
        .map(|e| format!("cd {d} && {e}"))
        .collect();
    if let Some(c) = r.execstart.value() {
        r.execstart = Known::resolved(ramener(c));
    }
    r.migrations = r.migrations.iter().map(|m| ramener(m)).collect();
    for p in &mut r.processus_annexes {
        p.execstart = ramener(&p.execstart);
    }
    r.racine_web = r.racine_web.as_ref().map(|w| format!("{d}/{w}"));
    r.repertoire_de_travail = Some(match &r.repertoire_de_travail {
        Some(w) => format!("{d}/{w}"),
        None => d.to_string(),
    });
}

/// Vrai si la configuration, la commande ou les migrations designent le
/// repertoire de donnees.
fn designe_les_donnees(r: &Runtime) -> bool {
    let d = |v: &str| v.contains("__DATA_DIR__");
    r.env.values().any(|v| d(v))
        || r.execstart.value().is_some_and(|v| d(v))
        || r.port_binding.value().is_some_and(|v| d(v))
        || r.database_binding.value().is_some_and(|v| d(v))
        || r.migrations.iter().any(|v| d(v))
        || r.processus_annexes.iter().any(|p| d(&p.execstart))
        || r.cron.iter().any(|v| d(v))
}

/// Vrai si une etape lance un outil de Node.
fn appelle_node(etapes: &[String]) -> bool {
    appelle(
        etapes,
        &["npm", "npx", "pnpm", "yarn", "corepack", "node", "bun"],
    )
}

/// Vrai si une etape lance l'un de ces outils.
fn appelle(etapes: &[String], outils: &[&str]) -> bool {
    etapes.iter().any(|e| {
        e.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
            .any(|m| outils.contains(&m.rsplit('/').next().unwrap_or(m)))
    })
}

/// Ce que les etapes de construction appellent, le paquet doit l'installer,
/// quel que soit le langage de l'application. klaxon (Ruby) construisait ses
/// assets par yarn sans Node ; danbooru lancait bundle sans Ruby ;
/// backstage (Node) installait sa documentation par pip, que Debian refuse
/// hors d'un environnement virtuel.
fn outils_des_etapes(facts: &RepoFacts, runtime: &mut Runtime, resources: &mut Resources) {
    let etapes = runtime.build_steps.clone();
    if resources.nodejs_version.is_none() && appelle_node(&etapes) {
        resources.nodejs_version = Some(
            facts
                .stack
                .version_node
                .clone()
                .unwrap_or_else(|| "20".into()),
        );
    }
    if resources.ruby_version.is_none() && appelle(&etapes, &["bundle", "gem", "rake", "rails"]) {
        resources.ruby_version = Some(
            (facts.stack.primary == Technology::Ruby)
                .then(|| facts.stack.runtime_version.clone())
                .flatten()
                .unwrap_or_else(|| "3.3".into()),
        );
    }
    if !runtime.venv_python && appelle(&etapes, &["pip", "pip3"]) {
        runtime.venv_python = true;
    }
    let mut apt = |p: &str| {
        if !resources.apt_packages.iter().any(|a| a == p) {
            resources.apt_packages.push(p.to_string());
        }
    };
    if runtime.venv_python {
        apt("python3-venv");
        apt("python3-dev");
    }
    if appelle(&etapes, &["composer"]) {
        apt("composer");
        apt("unzip");
    }
    // La ressource Ruby compile Ruby sur le serveur, et exige que le
    // manifeste declare de quoi le faire (docuseal : « libjemalloc-dev is
    // required to install Ruby »). Sans libssl-dev, Ruby n'aurait pas
    // OpenSSL.
    if resources.ruby_version.is_some() {
        for p in [
            "gcc",
            "make",
            "libjemalloc-dev",
            "libffi-dev",
            "libyaml-dev",
            "zlib1g-dev",
            "libssl-dev",
        ] {
            apt(p);
        }
    }
    for outil in ["git", "curl", "wget", "unzip", "make"] {
        if appelle(&etapes, &[outil]) {
            apt(outil);
        }
    }
    resources.apt_packages.sort_unstable();
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
        ram_build: memoire_de_construction(facts).into(),
        ..Integration::default()
    }
}

/// La memoire qu'il faut pour construire, que YunoHost verifie avant
/// d'installer. cap annoncait 500M, et la construction Next.js de son
/// interface depassait 3 Go.
fn memoire_de_construction(facts: &RepoFacts) -> &'static str {
    if facts
        .selection
        .as_ref()
        .is_some_and(|s| s.evite_la_compilation())
    {
        // Rien a compiler : les paquets de reference annoncent 50M.
        return "50M";
    }
    let etapes = facts
        .build
        .as_ref()
        .map(|b| b.build_steps.join("\n"))
        .unwrap_or_default();
    let deps = &facts.stack.dependances_npm;
    let a = |d: &str| deps.iter().any(|x| x == d);
    let lourd_en_node = [
        "next",
        "nuxt",
        "@angular/core",
        "turbo",
        "@remix-run/node",
        "gatsby",
    ]
    .iter()
    .any(|d| a(d))
        || ["next build", "turbo run", "ng build"]
            .iter()
            .any(|c| etapes.contains(c));
    let compile = matches!(facts.stack.primary, Technology::Rust)
        || ["cargo build", "gradle", "gradlew", "mvn", "mvnw", "dotnet"]
            .iter()
            .any(|c| etapes.contains(c));
    if lourd_en_node || compile {
        "2G"
    } else if facts.stack.primary == Technology::NodeJs
        || facts.stack.scripts_npm.contains_key("build")
        || facts.stack.primary == Technology::Go
    {
        "1G"
    } else {
        "500M"
    }
}

fn questions(facts: &RepoFacts) -> InstallQuestions {
    InstallQuestions {
        // Le cas courant, sauf un site construit : il reference ses fichiers
        // depuis la racine, et ne fonctionnerait pas sous un chemin.
        url_scheme: if statique::site(facts).is_some() || sans_reglage_de_chemin(facts) {
            UrlScheme::FullDomain
        } else {
            UrlScheme::DomainAndPath
        },
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
        // Une base SQLite aussi : dans le repertoire d'installation, la mise
        // a jour, qui le remplace, l'emporterait.
        data_dir: exige_un_stockage_s3(facts)
            || fichier_sqlite(facts).is_some()
            || facts.build.as_ref().is_some_and(|b| !b.volumes.is_empty())
            || facts
                .compose
                .as_ref()
                .is_some_and(|c| c.services.iter().any(|s| s.is_app && !s.volumes.is_empty())),
        main_permission_url: Some("/".into()),
        ports: a_besoin_d_un_port(facts),
        port_fixe: port_fixe_du_code(facts),
        apt_packages: apt::packages(facts),
        apt_extras: java::depot_temurin(facts).into_iter().collect(),
        database: facts.services.database,
        nodejs_version: version_de_resource(facts, Technology::NodeJs).or_else(|| {
            // Le frontal d'une application PHP se construit avec Node.
            (!php::etapes_du_frontal(facts).is_empty()).then(|| "20".to_string())
        }),
        ruby_version: version_de_resource(facts, Technology::Ruby),
        go_version: version_de_resource(facts, Technology::Go),
        composer_version: None,
    }
}

/// Retire le clone du depot lui-meme : wrp construit son image par
/// `git clone https://github.com/tenox7/wrp.git`, puis travaille dans ce
/// clone. Sous YunoHost, les sources sont deja en place, a la racine de
/// l'installation ; le clone les dupliquerait dans un sous-repertoire.
fn sans_clone_du_depot(etapes: Vec<String>, facts: &RepoFacts) -> Vec<String> {
    let depot = facts.source.repo.to_lowercase();
    let hote = facts
        .source
        .url
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or("")
        .to_lowercase();
    etapes
        .into_iter()
        .filter(|e| {
            let b = e.trim().to_lowercase();
            let clone = b.starts_with("git clone")
                && !hote.is_empty()
                && b.contains(&hote)
                && b.split_whitespace()
                    .any(|m| m.trim_end_matches(".git").rsplit('/').next() == Some(depot.as_str()));
            !clone
        })
        .collect()
}

/// Les chemins absolus du repertoire de travail de l'image, rendus relatifs :
/// les etapes tournent dans le repertoire d'installation, qui en tient lieu.
/// webhook.site creait `/var/www/html/database/database.sqlite`, hors de
/// toute installation.
fn relatives_au_repertoire_de_travail(etapes: Vec<String>, facts: &RepoFacts) -> Vec<String> {
    let Some(b) = facts.build.as_ref() else {
        return etapes;
    };
    // Le WORKDIR, et la ou l'image copie la racine du depot (`COPY . /buku`
    // chez bukuserver, qui installe ensuite `/buku[server]`).
    let mut racines: Vec<String> = b
        .copies
        .iter()
        .filter(|(vers, depuis)| vers.starts_with('/') && matches!(depuis.as_str(), "." | "./"))
        .map(|(vers, _)| vers.trim_end_matches('/').to_string())
        .collect();
    racines.extend(
        b.workdir
            .as_deref()
            .map(|w| w.trim_end_matches('/').to_string()),
    );
    racines.retain(|r| r.len() > 1);
    if racines.is_empty() {
        return etapes;
    }
    etapes
        .into_iter()
        .map(|e| {
            e.split(' ')
                .map(|m| {
                    for r in &racines {
                        if let Some(reste) = m.strip_prefix(&format!("{r}/")) {
                            return format!("./{reste}");
                        }
                        if m == r {
                            return ".".to_string();
                        }
                        // Des extras pip : `/buku[server]`.
                        if let Some(reste) =
                            m.strip_prefix(r.as_str()).filter(|x| x.starts_with('['))
                        {
                            return format!(".{reste}");
                        }
                    }
                    m.to_string()
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Un outil que l'image pose dans `/usr/bin` ou `/usr/local/bin` — uv chez
/// simplelogin, rclone chez duplicati, caddy chez dreeve — va a la racine de
/// l'installation, la ou la traduction des commandes le cherche deja ;
/// l'utilisateur de l'application ne peut pas ecrire dans le systeme. Le
/// PATH des etapes et du service inclut alors l'installation
/// (`runtime.outils_locaux`).
fn outils_dans_l_installation(etapes: Vec<String>) -> Vec<String> {
    const SYSTEME: &[&str] = &[
        "env", "sh", "bash", "python", "python3", "perl", "git", "curl", "wget", "tar", "unzip",
        "node", "npm", "php", "ruby", "java", "make", "gcc", "true", "false",
    ];
    etapes
        .into_iter()
        .map(|e| {
            let ecrit = ecrit_dans_un_repertoire_de_binaires(&e);
            if !ecrit {
                return e;
            }
            e.split(' ')
                .map(|m| {
                    let (tete, chemin) = match m.find("/usr/") {
                        Some(i) => (&m[..i], &m[i..]),
                        None => return m.to_string(),
                    };
                    for d in ["/usr/local/bin", "/usr/bin"] {
                        if chemin == d || chemin == format!("{d}/") {
                            return format!("{tete}$PWD");
                        }
                        if let Some(nom) = chemin.strip_prefix(&format!("{d}/")) {
                            let base = nom.split(['/', '"', '\'']).next().unwrap_or(nom);
                            if d == "/usr/bin" && SYSTEME.contains(&base) {
                                return m.to_string();
                            }
                            return format!("{tete}$PWD/{nom}");
                        }
                    }
                    m.to_string()
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Un secret propre a l'application, de forme libre : il se tire au hasard
/// sans rien casser. Pas une cle d'un service tiers (`API_KEY`), ni une cle
/// au format impose (`PRIVATE_KEY` hexadecimale, `ENCRYPTION_KEY` de
/// longueur fixe).
fn secret_interne(nom: &str) -> bool {
    let n = nom.to_ascii_uppercase();
    // Le secret canonique des cadres web (Django, Flask, Rails, Symfony) :
    // babybuddy s'arretait sur « The SECRET_KEY setting must not be empty ».
    if matches!(
        n.as_str(),
        "SECRET_KEY"
            | "SECRET_KEY_BASE"
            | "APP_SECRET"
            | "SECRET"
            | "FLASK_SECRET_KEY"
            | "DJANGO_SECRET_KEY"
    ) {
        return true;
    }
    // `AUTH_SECRET` d'Auth.js (NEXTAUTH_SECRET, BETTER_AUTH_SECRET), que
    // benevoles exige des la construction ; pas un secret OAuth tiers.
    let auth = n.ends_with("AUTH_SECRET") && !n.contains("OAUTH");
    (auth
        || [
            "JWT", "SESSION", "COOKIE", "SIGNING", "HMAC", "SALT", "CSRF",
        ]
        .iter()
        .any(|m| n.contains(m)))
        && !["PRIVATE_KEY", "ENCRYPTION", "PUBLIC"]
            .iter()
            .any(|m| n.contains(m))
}

/// Vrai pour un service web qui ne peut apprendre sous quel chemin il est
/// servi : aucune variable d'adresse publique ni de prefixe. nginx lui
/// transmet l'URL complete (`/butler/`), et il repond 404 a tout ce qui
/// n'est pas a la racine. PHP et les fichiers statiques, servis par nginx
/// lui-meme, suivent le chemin sans rien demander.
fn sans_reglage_de_chemin(facts: &RepoFacts) -> bool {
    if matches!(
        facts.stack.primary,
        Technology::Php | Technology::Static | Technology::Unknown
    ) {
        return false;
    }
    // Une adresse publique ne suffit pas : anonymousoverflow, qui recoit
    // APP_URL, repondait 404 sous /anonymousoverflow. Servi a la racine d'un
    // domaine, un service fonctionne toujours ; sous un chemin, seulement
    // s'il lit un prefixe.
    variable_de_chemin(facts).is_none()
}

/// La variable par laquelle l'application apprend sous quel prefixe elle est
/// servie (`BASE_PATH`, `URL_PREFIX`...) : le paquet y met le chemin
/// d'installation. Une adresse publique (`BASE_URL`) ne suffit pas : elle
/// sert souvent aux liens, pas au routage — calrs repondait 404 sous /calrs.
fn variable_de_chemin(facts: &RepoFacts) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "BASE_PATH",
        "BASEPATH",
        "ROOT_PATH",
        "SUBPATH",
        "SUB_PATH",
        "URL_PREFIX",
        "PATH_PREFIX",
        "BASE_URI",
        "CONTEXT_PATH",
        "WEB_ROOT",
        "URL_BASE",
    ];
    facts
        .config
        .variables
        .iter()
        .find(|v| {
            let n = v.name.to_ascii_uppercase();
            PREFIXES.iter().any(|p| n.ends_with(p)) && !d_un_service_tiers(&n)
        })
        .map(|v| v.name.clone())
}

/// Une variable qui designe un service tiers, et non l'application :
/// `CALRS_SMS_BASE_URL`, `OIDC_ISSUER_URL`, `S3_PUBLIC_URL`.
fn d_un_service_tiers(nom: &str) -> bool {
    let n = nom.to_ascii_uppercase();
    // Auth.js et Better Auth tournent dans l'application : leur adresse et
    // leur secret sont les siens (benevoles).
    if !n.contains("OAUTH") && ["AUTH_URL", "AUTH_SECRET"].iter().any(|s| n.ends_with(s)) {
        return false;
    }
    [
        "SMS", "SMTP", "MAIL", "API", "OIDC", "OAUTH", "AUTH", "S3", "WEBHOOK", "SENTRY", "STRIPE",
        "LDAP", "PROXY", "REDIS", "DATABASE", "DB_", "CDN", "STORAGE", "MINIO", "AWS", "TURN",
        "STUN",
    ]
    .iter()
    .any(|m| n.contains(m))
}

/// Le premier script de l'image qu'une etape de construction lance par son
/// chemin absolu, hors des repertoires du systeme et de l'application.
fn script_d_image(facts: &RepoFacts) -> Option<String> {
    let etapes = facts.build.as_ref()?.build_steps.clone();
    let du_systeme = |m: &str| {
        [
            "/usr/", "/bin/", "/sbin/", "/etc/", "/tmp/", "/dev/", "/proc/",
        ]
        .iter()
        .any(|p| m.starts_with(p))
    };
    etapes.into_iter().find_map(|e| {
        // Un script lance, pas un repertoire (`/build`, sortie de cmake chez
        // drawpile) ni un argument (`chmod +x /docker-entrypoint.d/start.sh`
        // chez endurain) : en tete d'une commande, ou apres un interprete.
        e.split([';', '&', '|', '(', ')']).find_map(|commande| {
            let mots: Vec<&str> = commande.split_whitespace().collect();
            let candidat = match mots.as_slice() {
                ["bash" | "sh" | "source" | ".", script, ..] => Some(*script),
                [script, ..] if script.ends_with(".sh") => Some(*script),
                _ => None,
            }?;
            (candidat.starts_with('/') && !du_systeme(candidat)).then(|| candidat.to_string())
        })
    })
}

/// Vrai pour l'option d'installation globale de npm, seule ou groupee
/// avec d'autres options courtes (`-g`, `-gf`, `--global`) : ferdium
/// installe pnpm par `npm i -gf`.
fn option_globale(m: &str) -> bool {
    m == "--global" || (m.starts_with('-') && !m.starts_with("--") && m.contains('g'))
}

/// `gradle` et `mvn` ne sont pas installes, et le paquet Debian de Gradle
/// est trop ancien : un projet livre presque toujours son propre lanceur
/// (`gradlew`, `mvnw`), a la version voulue. booklore construisait son API
/// par `gradle build`, introuvable.
fn lanceurs_du_projet(etapes: Vec<String>, facts: &RepoFacts) -> Vec<String> {
    etapes
        .into_iter()
        .map(|e| {
            // Le repertoire de l'etape : `cd booklore-api && gradle build`.
            let dossier = e
                .strip_prefix("cd ")
                .and_then(|r| r.split_once(" && "))
                .map(|(d, _)| d.trim().to_string());
            let a = |f: &str| {
                let chemin = match &dossier {
                    Some(d) => format!("{d}/{f}"),
                    None => f.to_string(),
                };
                facts.tree.contains(&chemin)
            };
            let mut mots: Vec<String> = Vec::new();
            let mut debut = true;
            for m in e.split(' ') {
                let remplace = match m {
                    "gradle" if debut && a("gradlew") => Some("./gradlew"),
                    "mvn" if debut && a("mvnw") => Some("./mvnw"),
                    _ => None,
                };
                mots.push(
                    remplace
                        .map(str::to_string)
                        .unwrap_or_else(|| m.to_string()),
                );
                debut = matches!(m, "&&" | "||" | ";");
            }
            mots.join(" ")
        })
        .collect()
}

/// `npm install -g turbo` ecrit dans le repertoire de Node, refuse a
/// l'utilisateur de l'application (documenso). L'outil s'installe dans un
/// prefixe de l'installation, que les etapes suivantes ont dans leur PATH ;
/// il reste appelable par son nom.
fn npm_global_dans_l_installation(etapes: Vec<String>) -> Vec<String> {
    let global = |e: &str| {
        let mots: Vec<&str> = e.split_whitespace().collect();
        mots.windows(2)
            .any(|w| w[0] == "npm" && matches!(w[1], "install" | "i" | "add"))
            && mots.iter().any(|m| option_globale(m))
            && !mots.iter().any(|m| m.starts_with("--prefix"))
    };
    if !etapes.iter().any(|e| global(e)) {
        return etapes;
    }
    etapes
        .into_iter()
        .map(|e| {
            let e = if global(&e) {
                let mots: Vec<String> = e
                    .split(' ')
                    .map(|m| {
                        if option_globale(m) {
                            format!("{m} --prefix \"$PWD/.npm-global\"")
                        } else {
                            m.to_string()
                        }
                    })
                    .collect();
                mots.join(" ")
            } else {
                e
            };
            format!("export PATH=\"$PWD/.npm-global/bin:$PATH\"; {e}")
        })
        .collect()
}

/// yarn et pnpm ne sont pas installes : corepack, livre avec Node, les
/// fournit a la demande. cronmaster lancait `yarn build`, introuvable.
/// `corepack enable` ecrirait dans le repertoire de Node, qui appartient a
/// root : il devient `true`. Les blocs `if … then … fi` sont parcourus aussi.
fn yarn_et_pnpm_par_corepack(etapes: Vec<String>) -> Vec<String> {
    const SEPARATEURS: &[&str] = &["&&", "||", ";", "|", "then", "else", "do", "(", "{"];
    etapes
        .into_iter()
        .map(|e| {
            let mut sortie: Vec<String> = Vec::new();
            let mut debut = true;
            let mut change = false;
            let mots: Vec<&str> = e.split(' ').collect();
            let mut i = 0;
            while i < mots.len() {
                let m = mots[i];
                if debut && matches!(m, "yarn" | "pnpm") {
                    sortie.push(format!("corepack {m}"));
                    change = true;
                } else if debut && m == "corepack" && mots.get(i + 1) == Some(&"enable") {
                    // `corepack enable [pnpm|yarn]` : rien a activer.
                    sortie.push("true".into());
                    i += 2;
                    while i < mots.len() && matches!(mots[i], "pnpm" | "yarn" | "npm") {
                        i += 1;
                    }
                    change = true;
                    debut = false;
                    continue;
                } else {
                    sortie.push(m.to_string());
                }
                debut = SEPARATEURS.contains(&m.trim_end_matches(';'))
                    || m.ends_with(';')
                    || (debut && m.contains('=') && !m.starts_with('-'));
                i += 1;
            }
            let ligne = sortie.join(" ");
            // Les scripts du projet appellent eux-memes `pnpm` (emdash :
            // `pnpm run --filter … build` dans `pnpm build`) : les raccourcis
            // de corepack, poses dans l'installation, sont dans le PATH.
            let raccourcis = "export COREPACK_HOME=\"$PWD/.corepack\" PATH=\"$PWD/.corepack/bin:$PATH\"; \
                 mkdir -p .corepack/bin && corepack enable --install-directory .corepack/bin pnpm yarn;";
            if (change || ligne.contains("corepack ")) && !ligne.contains("--install-directory") {
                let ligne = ligne
                    .trim_start_matches("export COREPACK_HOME=\"$PWD/.corepack\"; ")
                    .to_string();
                format!("{raccourcis} {ligne}")
            } else {
                ligne
            }
        })
        .collect()
}

/// Bun, que YunoHost ne fournit pas, est publie sur npm avec ses binaires :
/// `npx --yes bun` le lance sans rien installer dans le systeme. cap
/// construisait par `bun install`, introuvable.
fn bun_par_npx(etapes: Vec<String>) -> Vec<String> {
    etapes
        .into_iter()
        .map(|e| {
            let mut sortie = Vec::new();
            let mut debut_de_commande = true;
            for m in e.split(' ') {
                if debut_de_commande && m == "bun" {
                    sortie.push("npx --yes bun".to_string());
                } else {
                    sortie.push(m.to_string());
                }
                // Un mot qui termine une commande : le suivant en commence une.
                debut_de_commande =
                    matches!(m, "&&" | "||" | ";" | "|") || (m.contains('=') && debut_de_commande);
            }
            sortie.join(" ")
        })
        .collect()
}

/// Une installation npm qui echoue sur un conflit de dependances paires
/// (ERESOLVE) est retentee comme npm 6 la faisait. Les projets ecrits avant
/// npm 7 en sont pleins : astral ne s'installait pas.
fn avec_les_pairs_tolerees(etapes: Vec<String>) -> Vec<String> {
    etapes
        .into_iter()
        .map(|e| {
            let t = e.trim();
            let npm = t == "npm ci"
                || t == "npm install"
                || (t.starts_with("npm ci ") || t.starts_with("npm install "))
                    && !t.contains("&&")
                    && !t.contains("||")
                    && !t.contains(" -g")
                    && !t.contains("--global")
                    && !t.contains("legacy-peer-deps");
            if npm {
                format!("{t} || {t} --legacy-peer-deps")
            } else {
                e
            }
        })
        .collect()
}

/// Vrai si l'etape ecrit dans `/usr/bin` ou `/usr/local/bin`.
fn ecrit_dans_un_repertoire_de_binaires(etape: &str) -> bool {
    const ECRITURE: &[&str] = &[
        "cp", "mv", "install", "ln", "tar", "unzip", "curl", "wget", "chmod",
    ];
    let mots: Vec<&str> = etape
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
        .filter(|m| !m.is_empty())
        .collect();
    let mut ecrit = false;
    for (i, m) in mots.iter().enumerate() {
        if ECRITURE.contains(&m.rsplit('/').next().unwrap_or(m)) {
            ecrit = true;
        }
        let cible = m
            .trim_matches(|c| c == '"' || c == '\'')
            .trim_start_matches('>');
        let bin = cible.starts_with("/usr/local/bin") || cible.starts_with("/usr/bin");
        if bin
            && (ecrit
                || mots
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.starts_with('>')))
        {
            return true;
        }
    }
    false
}

/// `corepack enable` ecrit ses raccourcis dans le repertoire de Node, qui
/// appartient a root : execute comme utilisateur de l'application, il
/// echoue. Appeler directement `corepack pnpm …` produit le meme effet sans
/// rien ecrire hors de chez l'application, ou son cache est pose.
fn sans_activation_de_corepack(etapes: Vec<String>) -> Vec<String> {
    // `npm install -g pnpm` revient au meme, et echoue de meme : your_spotify
    // installait ainsi pnpm dans le repertoire de Node, refuse a
    // l'utilisateur de l'application.
    let active = |e: &String| {
        let e = e.trim();
        if e.starts_with("corepack enable") {
            return true;
        }
        let mots: Vec<&str> = e.split_whitespace().collect();
        matches!(mots.first(), Some(&"npm"))
            && mots.iter().any(|m| matches!(*m, "install" | "i" | "add"))
            && mots.iter().any(|m| option_globale(m))
            && mots
                .iter()
                .filter(|m| !m.starts_with('-') && !matches!(**m, "npm" | "install" | "i" | "add"))
                .all(|m| {
                    // `"pnpm@$(node -p …)"` chez ferdium : le nom avant la version.
                    let m = m.trim_matches('"');
                    let nom = m.split('@').next().unwrap_or(m);
                    matches!(nom, "pnpm" | "yarn" | "corepack")
                })
    };
    if !etapes.iter().any(active) {
        return etapes;
    }
    etapes
        .into_iter()
        .filter(|e| !active(e))
        .map(|e| {
            // Les affectations de tete (`CI=true pnpm install`) restent devant.
            let mots: Vec<&str> = e.split_whitespace().collect();
            let n = mots
                .iter()
                .take_while(|m| m.contains('=') && !m.starts_with('-'))
                .count();
            let (tete, reste) = (mots[..n].join(" "), mots[n..].join(" "));
            let mut c = reste;
            for outil in ["pnpm", "yarn"] {
                if c.starts_with(&format!("{outil} ")) {
                    c = format!("corepack {c}");
                }
                c = c.replace(&format!("&& {outil} "), &format!("&& corepack {outil} "));
            }
            let c = if tete.is_empty() {
                c
            } else {
                format!("{tete} {c}")
            };
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
        //
        // Sauf un secret interne, de forme libre, que le code exige souvent
        // sans le dire facultatif : AnonymousOverflow s'arrete au demarrage
        // sans JWT_SIGNING_SECRET.
        if var.role == ConfigRole::Secret && var.facultative && !secret_interne(&var.name) {
            continue;
        }
        if matches!(var.role, ConfigRole::BaseUrl | ConfigRole::Domain)
            && d_un_service_tiers(&var.name)
        {
            continue;
        }
        // Un nom d'adresse dont la valeur par defaut est un hote ou ecouter :
        // `HOSTNAME=0.0.0.0` du serveur autonome de Next.js. L'adresse
        // publique y ferait echouer l'ecoute.
        // Un nom d'hote, pas une URL : APP_URL=localhost chez ferdium-server
        // est l'adresse publique a remplir.
        if var.role == ConfigRole::BaseUrl
            && !var.name.to_ascii_uppercase().contains("URL")
            && matches!(
                var.default.as_deref(),
                Some("0.0.0.0" | "::" | "[::]" | "127.0.0.1" | "localhost")
            )
        {
            env.insert(var.name.clone(), "127.0.0.1".to_string());
            continue;
        }
        // L'identifiant d'un service tiers (OIDC_CLIENT_SECRET, SMTP_PASSWORD)
        // ne s'invente pas : une valeur au hasard activerait une connexion qui
        // ne peut pas marcher.
        if var.role == ConfigRole::Secret
            && d_un_service_tiers(&var.name)
            && !secret_interne(&var.name)
        {
            continue;
        }
        // Un secret au format que le projet prescrit : `__SECRET_BASE64_32__`,
        // que les scripts tirent par la meme commande openssl.
        if var.role == ConfigRole::Secret {
            if let Some((f, n)) = var.format_secret.as_deref().and_then(|f| f.split_once(':')) {
                env.insert(
                    var.name.clone(),
                    format!("__SECRET_{}_{n}__", f.to_ascii_uppercase()),
                );
                continue;
            }
        }
        // Un chemin de donnees qui designe un fichier (`/data/app.db`) : le
        // fichier, dans le repertoire de donnees.
        if var.role == ConfigRole::DataPath {
            if let Some(fichier) = var
                .default
                .as_deref()
                .and_then(|d| d.rsplit('/').next())
                .filter(|f| {
                    [".db", ".sqlite", ".sqlite3", ".json"]
                        .iter()
                        .any(|e| f.ends_with(e))
                })
            {
                env.insert(var.name.clone(), format!("__DATA_DIR__/{fichier}"));
                continue;
            }
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
                // Le Redis de YunoHost n'a pas de mot de passe : en envoyer
                // un fait echouer la connexion (« AUTH called without any
                // password configured »).
                "REDIS_PASSWORD" => {
                    env.insert(var.name.clone(), String::new());
                    continue;
                }
                _ => continue,
            };
            env.entry(var.name.clone())
                .or_insert_with(|| valeur.to_string());
        }
    }

    // Le prefixe sous lequel l'application est servie, quand elle sait le
    // lire. Pas `__PATH__` : il vaut `/` a la racine d'un domaine, et
    // l'application qui ecrit `${BASE_PATH}/api` servirait `//api`. Les
    // scripts posent `prefixe` au chemin sans sa barre finale, vide a la
    // racine.
    if let Some(v) = variable_de_chemin(facts) {
        env.insert(v, "__PREFIXE__".to_string());
    }

    // Le module de reglages que Django attend de l'environnement.
    if let Some(m) = &facts.stack.reglages_django {
        env.entry("DJANGO_SETTINGS_MODULE".to_string())
            .or_insert_with(|| m.clone());
    }

    // Rails en production : l'environnement, la cle des sessions, les
    // fichiers statiques et le journal, que l'image fixait par ses ENV.
    if facts.stack.cadre_web.as_deref() == Some("rails") {
        for (nom, valeur) in [
            ("RAILS_ENV", "production"),
            ("SECRET_KEY_BASE", "__SECRET__"),
            ("RAILS_SERVE_STATIC_FILES", "true"),
            ("RAILS_LOG_TO_STDOUT", "true"),
        ] {
            env.entry(nom.to_string())
                .or_insert_with(|| valeur.to_string());
        }
    }

    // Ce que le cadre PHP exige et qu'aucun exemple ne remplit : la cle de
    // chiffrement de Laravel, l'environnement de production.
    for (nom, valeur) in php::reglages(facts) {
        env.insert(nom.to_string(), valeur.to_string());
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

    // Les affectations qui precedent la commande de l'image vont dans la
    // configuration : cap lance `HOSTNAME="0.0.0.0" node apps/web/server.js`.
    // Une ecoute sur toutes les interfaces se ramene sur la boucle locale,
    // nginx etant devant.
    let commandes = facts
        .build
        .as_ref()
        .and_then(|b| b.start_command())
        // Depliee : le script d'un paquet d'espace de travail porte ses
        // propres affectations (`NODE_ENV=production`, chez chibisafe).
        .map(|c| {
            if ynp_core::noms::commande_d_espace_de_travail(&c).is_some() {
                execstart::developper(&c, facts)
            } else {
                c
            }
        })
        .into_iter()
        .chain(facts.stack.exec_du_point_d_entree.clone())
        // Le script npm qui lance l'application, sans image :
        // `cross-env NODE_ENV=production node ./index`.
        .chain(
            facts
                .build
                .is_none()
                .then(|| entree::script_de_production(facts))
                .flatten()
                .map(|s| execstart::developper(&format!("npm run {s}"), facts)),
        );
    for c in commandes {
        for (nom, valeur) in execstart::affectations_de_tete(&c) {
            let valeur = if matches!(valeur.as_str(), "0.0.0.0" | "::" | "[::]") {
                "127.0.0.1".to_string()
            } else {
                valeur
            };
            env.entry(nom).or_insert(valeur);
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
        _ if statique::site(facts).is_some() => Known::resolved(String::new()),
        // Sauf la file de taches d'une application Laravel, que rien d'autre
        // ne fait tourner.
        Technology::Php => Known::resolved(php::file_de_taches(facts).unwrap_or_default()),
        Technology::Static => Known::resolved(String::new()),
        _ => avec_l_option_de_port(facts, execstart::derive(facts, app_id)),
    };

    // Les migrations que le script d'entree lancait avant son `exec` se
    // traduisent comme la commande. Une seule qui ne se traduit pas, et la
    // commande redevient une question : demarrer sur une base jamais migree
    // serait un paquet qui a l'air fini sans l'etre.
    let sans_commande_d_image = facts
        .build
        .as_ref()
        .is_some_and(|b| b.start_command().is_none());
    let mut migrations: Vec<String> = entree::deduite(facts)
        .map(|e| e.migrations)
        .or_else(|| {
            sans_commande_d_image
                .then(|| entree::django(facts).map(|(_, m)| m))
                .flatten()
        })
        .unwrap_or_default();
    for m in php::migrations(facts) {
        if !migrations.contains(&m) {
            migrations.push(m);
        }
    }
    let mut intraduisible = None;
    for m in &facts.stack.migrations_du_point_d_entree {
        match execstart::traduire_avec(m, facts) {
            Some(t) => migrations.push(t),
            None => intraduisible = Some(m.clone()),
        }
    }
    // Ce qui precede le serveur dans le script d'un paquet d'espace de
    // travail : `yarn migrate && node dist/main.js` chez chibisafe. Le
    // service demarre dans le dossier du paquet ; ses migrations aussi.
    if let (Some(p), Some((_, script))) = (
        facts.stack.paquet_du_processus.as_ref(),
        facts
            .build
            .as_ref()
            .and_then(|b| b.start_command())
            .and_then(|c| ynp_core::noms::commande_d_espace_de_travail(&c)),
    ) {
        if let Some(corps) = p.scripts.get(&script) {
            let parts: Vec<&str> = corps.split("&&").map(str::trim).collect();
            // Un script appele (`yarn migrate`) se deplie lui-meme, partie par
            // partie : `prisma migrate deploy && prisma generate`.
            let mut a_traduire: Vec<String> = Vec::new();
            for part in parts.iter().take(parts.len().saturating_sub(1)) {
                match execstart::script_npm_appele(part).and_then(|s| p.scripts.get(s)) {
                    Some(sous) => a_traduire.extend(sous.split("&&").map(|x| x.trim().to_string())),
                    None => a_traduire.push(part.to_string()),
                }
            }
            for commande in a_traduire {
                if let Some(t) = execstart::traduire_avec(&commande, facts) {
                    if !migrations.contains(&t) {
                        migrations.push(t);
                    }
                }
            }
        }
    }
    let (processus_annexes, annexe_intraduisible) = processus_annexes(facts, &mut migrations);
    let execstart = match (annexe_intraduisible, execstart.value()) {
        (Some((nom, c)), Some(_)) => Known::unresolved_avec(
            format!(
                "le Procfile declare aussi le processus « {nom} » (`{c}`), non transposable \
                 automatiquement : sans lui, l'application serait incomplete"
            ),
            &["Procfile", "la documentation du projet d'origine"],
            Vec::new(),
        ),
        _ => execstart,
    };
    // Une etape qui lance un script de l'image par son chemin absolu
    // (`bash /bd_build/prepare.sh` chez AzuraCast) prepare un systeme entier :
    // rien ne dit ce qu'il fait, et il ne se transpose pas.
    let execstart = match (script_d_image(facts), execstart.value()) {
        (Some(script), Some(_)) => Known::unresolved_avec(
            format!(
                "une etape de construction lance « {script} », un script de preparation de \
                 l'image, non transposable automatiquement"
            ),
            &[
                "le Dockerfile",
                "la documentation d'installation sans Docker du projet",
            ],
            Vec::new(),
        ),
        _ => execstart,
    };
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

    let etapes = if facts
        .selection
        .as_ref()
        .is_some_and(|s| s.evite_la_compilation())
    {
        // Rien a construire : l'amont publie un binaire. Qu'il soit
        // executable, quel que soit le mode que l'archive lui a laisse :
        // dmarc-report-viewer le publie en -rw-r--r--.
        // (Un binaire nu, lui, l'est par le script d'installation.)
        let archive = facts
            .selection
            .as_ref()
            .is_some_and(|s| s.prebuilt.iter().any(|a| a.extract));
        execstart
            .value()
            .filter(|_| archive)
            .and_then(|c| c.split_whitespace().next())
            .and_then(|b| b.strip_prefix("__INSTALL_DIR__/"))
            .map(|b| vec![format!("chmod +x {b}")])
            .unwrap_or_default()
    } else {
        let mut etapes = statique::site(facts)
            .map(|s| s.etapes)
            .or_else(|| facts.build.as_ref().map(|b| b.build_steps.clone()))
            .or_else(|| entree::deduite(facts).map(|e| e.etapes))
            .unwrap_or_default();
        if facts.stack.primary == Technology::Php {
            etapes = etapes
                .iter()
                .map(|e| php::avec_le_php_du_manifeste(e))
                .collect();
            if !etapes.iter().any(|e| e.contains("composer.phar")) {
                if let Some(c) = php::etape_composer(facts) {
                    etapes.insert(0, c);
                }
            }
            if !etapes.iter().any(|e| e.contains("npm ")) {
                etapes.extend(php::etapes_du_frontal(facts));
            }
            // Le lien public des fichiers deposes (`public/storage`), que
            // Laravel ne cree pas de lui-meme.
            if php::cadre(facts) == Some(php::Cadre::Laravel)
                && !etapes.iter().any(|e| e.contains("storage:link"))
            {
                etapes.push("php$PHP_VERSION artisan storage:link || true".into());
            }
        }
        etapes.extend(copies_apres_construction(facts));
        // La convention Django lance gunicorn : un Dockerfile sans
        // commande ne l'installe pas forcement.
        let sans_cmd = facts
            .build
            .as_ref()
            .is_some_and(|b| b.start_command().is_none());
        if sans_cmd
            && entree::django(facts).is_some()
            && !etapes.iter().any(|e| e.contains("gunicorn"))
        {
            etapes.push("pip install gunicorn".into());
        } else if sans_cmd && entree::django(facts).is_none() {
            if let Some((_, serveur)) = entree::application_python(facts) {
                if !etapes.iter().any(|e| e.contains(serveur)) {
                    etapes.push(format!("pip install {serveur}"));
                }
            }
        }
        sans_activation_de_corepack(relatives_au_repertoire_de_travail(
            sans_clone_du_depot(etapes, facts),
            facts,
        ))
    };
    // @t3-oss/env valide l'environnement des la construction, et l'arrete
    // faute des valeurs de production (fountain-ink : « Invalid environment
    // variables »). Sa convention : SKIP_ENV_VALIDATION pour construire.
    let etapes: Vec<String> = if facts
        .stack
        .dependances_npm
        .iter()
        .any(|d| d.starts_with("@t3-oss/env"))
    {
        etapes
            .into_iter()
            .map(|e| format!("export SKIP_ENV_VALIDATION=1; {e}"))
            .collect()
    } else {
        etapes
    };
    let outils_locaux = etapes
        .iter()
        .any(|e| ecrit_dans_un_repertoire_de_binaires(e));
    let etapes = lanceurs_du_projet(etapes, facts);
    let build_steps = npm_global_dans_l_installation(yarn_et_pnpm_par_corepack(bun_par_npx(
        avec_les_pairs_tolerees(outils_dans_l_installation(etapes)),
    )));

    let port_binding = liaison_port(facts, execstart.value().map(String::as_str));
    let database_binding = liaison_base(facts);
    let a_une_liaison = port_binding.value().is_some_and(|v| !v.is_empty())
        || database_binding.value().is_some_and(|v| !v.is_empty());

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
            // Un site construit par Node est servi tel quel.
            _ if statique::site(facts).is_some() => Known::resolved(Technology::Static),
            t => Known::resolved(t),
        },
        build_steps,
        outils_locaux,
        rust_toolchain: chaine_rust(facts),
        // Toute application Python, et toute commande qui appelle le venv.
        venv_python: facts.stack.primary == Technology::Python
            || execstart
                .value()
                .is_some_and(|c| c.contains("__INSTALL_DIR__/venv/")),
        port_env_var: facts.config.get(ConfigRole::Port).map(|v| v.name.clone()),
        port_binding,
        execstart,
        migrations,
        processus_annexes,
        racine_web: php::racine_web(facts).or_else(|| statique::site(facts).map(|s| s.racine)),
        a_garder: php::a_garder(facts),
        fichier_sqlite: fichier_sqlite(facts),
        cron: php::cron(facts),
        repertoire_de_travail: repertoire_de_travail(facts),
        database_binding,
        // Un fichier de configuration est necessaire des qu'il y a quelque
        // chose a transmettre a l'application, meme si l'amont n'en publie
        // aucun exemple. C'est ce manque qui a fait installer un miniflux
        // incapable de demarrer.
        // Une liaison a ecrire suffit : le port de farside, lu dans son
        // code, ne figurait dans aucun fichier faute d'exemple, et
        // l'application ecoutait sur son port par defaut.
        config_file: (facts.config.example_file.is_some()
            || facts.services.database != Database::None
            || !env.is_empty()
            || a_une_liaison)
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
    // Un serveur d'application Python lance sans adresse : gunicorn ecoute
    // alors sur 127.0.0.1:8000, ou sur $PORT et toutes les interfaces sous un
    // Procfile ; uvicorn sur 127.0.0.1:8000. On lui donne le port reserve,
    // sur la boucle locale. babybuddy lance `gunicorn babybuddy.wsgi` ainsi.
    let premier = commande.split_whitespace().next().unwrap_or("");
    let serveur = premier.rsplit('/').next().unwrap_or(premier);
    if !commande.contains("__PORT__") {
        let a = |o: &str| {
            commande
                .split_whitespace()
                .any(|m| m == o || m.starts_with(&format!("{o}=")))
        };
        if serveur == "gunicorn" && !a("-b") && !a("--bind") {
            return Known::resolved(format!("{commande} --bind 127.0.0.1:__PORT__"));
        }
        if serveur == "uvicorn" && !a("--port") && !a("--uds") && !a("--fd") {
            let hote = if a("--host") { "" } else { " --host 127.0.0.1" };
            return Known::resolved(format!("{commande}{hote} --port __PORT__"));
        }
    }
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
        _ => {
            // Une option dont le code donne une adresse par defaut
            // (`--bind`, `:8923` chez anubis) recoit l'adresse reservee.
            let adresse = facts
                .ecoute
                .options_adresse
                .iter()
                .map(|(o, _)| o.as_str())
                .find(|o| {
                    matches!(
                        *o,
                        "--bind"
                            | "--listen"
                            | "--addr"
                            | "--address"
                            | "--listen-addr"
                            | "--http-addr"
                            | "--http"
                    )
                });
            match adresse {
                Some(o)
                    if sans_variable_de_port(facts)
                        && !commande.contains("__PORT__")
                        && !commande.contains(o) =>
                {
                    Known::resolved(format!("{commande} {o}=127.0.0.1:__PORT__"))
                }
                _ => execstart,
            }
        }
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
    let mut exposes = ports_exposes(facts);
    exposes.sort_unstable();
    exposes.dedup();
    if let [p] = ports[..] {
        return (exposes.is_empty() || exposes.contains(&p)).then_some(p);
    }
    if !ports.is_empty() {
        return None;
    }
    // En dernier recours, le port unique que l'image publie : c'est celui
    // sur lequel l'application ecoute par defaut. Si elle lisait en fait une
    // variable que l'outil n'a pas vue, sa valeur par defaut est ce port-la ;
    // s'il est deja pris, YunoHost le dit a l'installation. bytestash publie
    // 5000, gathio 3000, filestash 8334.
    //
    // Sauf un port privilegie ou une image qui embarque son serveur web :
    // le port 80 d'active-pieces ou de fireshare est celui de leur nginx,
    // place devant l'application. Et un cadre qui nomme deja son port
    // (Spring Boot) se lie par ce nom.
    let serveur_web = facts.build.as_ref().is_some_and(|b| {
        b.apt_packages.iter().chain(b.apk_packages.iter()).any(|p| {
            matches!(
                p.as_str(),
                "nginx" | "caddy" | "apache2" | "httpd" | "lighttpd"
            )
        })
    });
    match exposes[..] {
        [p] if p >= 1024 && !serveur_web && facts.stack.cadre_web.is_none() => Some(p),
        _ => None,
    }
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
        // Spring Boot fixe ces noms pour toutes ses applications.
        None if facts.stack.cadre_web.as_deref() == Some("spring-boot") => {
            Known::resolved("SERVER_PORT=__PORT__\nSERVER_ADDRESS=127.0.0.1".into())
        }
        None if facts.stack.cadre_web.as_deref() == Some("rails") => {
            Known::resolved("PORT=__PORT__".into())
        }
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

/// Le sous-repertoire du depot ou l'image travaille, s'il y en a un.
///
/// documenso travaille dans `/app/apps/remix`, gladys dans `/src/server` :
/// leurs commandes (`node build/server/main.js`, `node index.js`) et leurs
/// migrations (`../../packages/prisma/...`) se lisent depuis la. Le service
/// travaillait a la racine de l'installation, ou ces chemins ne designent
/// rien. Le repertoire n'est retenu que s'il existe dans le depot.
fn repertoire_de_travail(facts: &RepoFacts) -> Option<String> {
    if let Some(c) = facts.build.as_ref().and_then(|b| b.contexte_final.clone()) {
        return Some(c);
    }
    // Le paquet de l'espace de travail que l'image lance demarre chez lui.
    if let Some(p) = &facts.stack.paquet_du_processus {
        return Some(p.dossier.clone());
    }
    const RACINES: &[&str] = &[
        "/app/",
        "/usr/src/app/",
        "/srv/app/",
        "/srv/",
        "/opt/app/",
        "/code/",
        "/home/app/",
        "/var/www/html/",
        "/var/www/",
        "/workspace/",
        "/src/",
    ];
    let w = format!(
        "{}/",
        facts
            .build
            .as_ref()?
            .workdir
            .as_deref()?
            .trim_end_matches('/')
    );
    let racine = RACINES
        .iter()
        .find(|r| w.starts_with(**r) && w.len() > r.len())?;
    let sous = w[racine.len()..].trim_end_matches('/').to_string();
    // Le WORKDIR est le sous-repertoire de l'application lui-meme : celui-ci
    // s'applique deja a tout (analytodon, apps/frontend).
    if facts.stack.sous_repertoire.as_deref() == Some(sous.as_str()) {
        return None;
    }
    facts
        .tree
        .iter()
        .any(|p| p.starts_with(&format!("{sous}/")))
        .then_some(sous)
}

/// L'adresse de la base, sous le schema que l'application attend.
///
/// SQLAlchemy refuse `postgres://` depuis sa version 1.4 : Airflow veut
/// `postgresql+psycopg2://`. Le schema de la valeur d'exemple du projet fait
/// foi ; a defaut, une variable SQLAlchemy recoit `postgresql://`.
fn avec_le_schema_attendu(adresse: &str, v: &ynp_core::facts::ConfigVar, base: Database) -> String {
    let Some((_, reste)) = adresse.split_once("://") else {
        return adresse.to_string();
    };
    let exemple = v
        .default
        .as_deref()
        .and_then(|d| d.split_once("://"))
        .map(|(schema, _)| schema.to_ascii_lowercase());
    let compatible = |s: &str| match base {
        Database::PostgreSql => s.starts_with("postgres"),
        Database::MySql => s.starts_with("mysql") || s.starts_with("mariadb"),
        _ => false,
    };
    let schema = match exemple {
        Some(s) if compatible(&s) => s,
        _ if base == Database::PostgreSql && {
            let n = v.name.to_ascii_uppercase();
            n.contains("SQLALCHEMY") || n.contains("SQL_ALCHEMY")
        } =>
        {
            "postgresql".to_string()
        }
        _ => return adresse.to_string(),
    };
    format!("{schema}://{reste}")
}

/// Le nom du moteur dans le vocabulaire de l'application.
///
/// Laravel dit `pgsql`, Strapi, TypeORM, Sequelize ou Gitea `postgres`. Une
/// valeur d'exemple deja dans la bonne famille (`postgresql`) fait foi.
fn type_de_base(
    v: &ynp_core::facts::ConfigVar,
    base: Database,
    cites: &std::collections::BTreeMap<String, usize>,
) -> Option<String> {
    let exemple = v.default.as_deref().map(str::to_ascii_lowercase);
    let famille: &[&str] = match base {
        Database::PostgreSql => &["pgsql", "postgres", "postgresql", "pg"],
        Database::MySql => &["mysql", "mysql2", "mariadb"],
        _ => return None,
    };
    if let Some(e) = exemple.filter(|e| famille.contains(&e.as_str())) {
        return Some(e);
    }
    // Le nom que le code cite le plus, dans la bonne famille. Kutt n'accepte
    // que `pg` pour DB_CLIENT, et Ferdium — AdonisJS, pas Laravel — `pg` pour
    // DB_CONNECTION : deviner d'apres le nom de la variable les aurait
    // empeches de demarrer.
    if let Some((nom, _)) = cites
        .iter()
        .filter(|(n, _)| famille.contains(&n.as_str()))
        .max_by_key(|(_, n)| **n)
    {
        return Some(nom.clone());
    }
    let laravel = v.name.eq_ignore_ascii_case("DB_CONNECTION");
    Some(
        match (base, laravel) {
            (Database::PostgreSql, true) => "pgsql",
            (Database::PostgreSql, false) => "postgres",
            _ => "mysql",
        }
        .to_string(),
    )
}

/// Ligne de configuration transmettant les identifiants de la base.
///
/// La chaine de connexion se compose de facon deterministe a partir des
/// reglages que le coeur fournit ; seul le *nom* sous lequel l'application
/// l'attend reste inconnu quand l'amont ne publie aucun exemple.
fn liaison_base(facts: &RepoFacts) -> Known<String> {
    if let Some(fichier) = fichier_sqlite(facts) {
        return Known::resolved(liaison_sqlite(facts, &fichier));
    }
    let adresse = match facts.services.database {
        Database::PostgreSql => {
            "postgres://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__?sslmode=disable"
        }
        Database::MySql => "mysql://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__",
        // Rien a transmettre : pas de base, ou une base fichier.
        _ => return Known::resolved(String::new()),
    };

    // Le moteur, a donner en plus de l'adresse : Strapi recoit sinon
    // DATABASE_URL, et reste sur SQLite, son defaut.
    let moteur: Vec<String> = facts
        .config
        .variables
        .iter()
        .filter(|v| v.role == ConfigRole::DatabaseType)
        .filter_map(|v| {
            type_de_base(v, facts.services.database, &facts.config.moteurs_cites)
                .map(|t| format!("{}={t}", v.name))
        })
        .collect();

    // Toutes les variables qui portent l'adresse la recoivent : Airflow
    // nomme AIRFLOW__CORE__SQL_ALCHEMY_CONN et AIRFLOW__DATABASE__..., et
    // n'accepte plus que la seconde depuis sa version 3.
    let adresses: Vec<String> = facts
        .config
        .variables
        .iter()
        .filter(|v| v.role == ConfigRole::DatabaseUrl)
        .map(|v| {
            format!(
                "{}={}",
                v.name,
                avec_le_schema_attendu(adresse, v, facts.services.database)
            )
        })
        .collect();
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

    // L'adresse et les champs separes ensemble quand le projet nomme les
    // deux : anonaddy declare DB_URL, que sa version de Laravel ne lit pas,
    // et retombait sur l'utilisateur `forge` de config/database.php.
    if !adresses.is_empty() || !champs.is_empty() {
        return Known::resolved(
            moteur
                .into_iter()
                .chain(adresses)
                .chain(champs)
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    // Rails fusionne `DATABASE_URL` dans config/database.yml ; le schema
    // nomme l'adaptateur (`postgresql`, `mysql2`).
    if facts.stack.cadre_web.as_deref() == Some("rails") {
        let url = match facts.services.database {
            Database::PostgreSql => {
                "postgresql://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__".to_string()
            }
            _ => {
                let adaptateur = if facts.stack.dependances_ruby.iter().any(|g| g == "trilogy") {
                    "trilogy"
                } else {
                    "mysql2"
                };
                format!("{adaptateur}://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__")
            }
        };
        return Known::resolved(
            moteur
                .into_iter()
                .chain([format!("DATABASE_URL={url}")])
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    // Spring Boot lit sa base dans SPRING_DATASOURCE_*, au format JDBC.
    if facts.stack.cadre_web.as_deref() == Some("spring-boot") {
        let url = match facts.services.database {
            Database::PostgreSql => "jdbc:postgresql://127.0.0.1:5432/__DB_NAME__",
            _ => "jdbc:mysql://127.0.0.1:3306/__DB_NAME__",
        };
        return Known::resolved(format!(
            "SPRING_DATASOURCE_URL={url}\nSPRING_DATASOURCE_USERNAME=__DB_USER__\nSPRING_DATASOURCE_PASSWORD=__DB_PWD__"
        ));
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

/// Les processus du Procfile autres que `web`, traduits. `release` est la
/// commande que la plateforme lance a chaque deploiement : une migration,
/// lancee ici avant chaque demarrage. Un processus de developpement
/// (`--watch`) n'a rien a faire en production. Rend aussi le premier
/// processus qui ne se traduit pas.
fn processus_annexes(
    facts: &RepoFacts,
    migrations: &mut Vec<String>,
) -> (Vec<ProcessusAnnexe>, Option<(String, String)>) {
    let mut out = Vec::new();
    let mut intraduisible = None;
    if matches!(facts.stack.primary, Technology::Php | Technology::Static) {
        return (out, None);
    }
    for p in &facts.stack.processus_du_procfile {
        if entree::de_developpement(&p.commande) {
            continue;
        }
        let Some(t) = execstart::traduire_avec(&p.commande, facts) else {
            intraduisible.get_or_insert((p.ou.clone(), p.commande.clone()));
            continue;
        };
        if p.ou == "release" {
            if !migrations.contains(&t) {
                migrations.push(t);
            }
            continue;
        }
        let nom: String =
            p.ou.to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
        out.push(ProcessusAnnexe { nom, execstart: t });
    }
    (out, intraduisible)
}

/// Le fichier d'une base SQLite que le paquet range dans `data_dir`, quand
/// l'application dit ou elle l'attend : une variable de nom de base, ou une
/// adresse `sqlite:`/`file:`.
fn fichier_sqlite(facts: &RepoFacts) -> Option<String> {
    if facts.services.database != Database::Sqlite {
        return None;
    }
    let nommee = facts
        .config
        .variables
        .iter()
        .any(|v| v.role == ConfigRole::DatabaseName && !d_un_autre_moteur(&v.name))
        || facts.config.variables.iter().any(|v| {
            v.role == ConfigRole::DatabaseUrl
                && v.default
                    .as_deref()
                    .is_some_and(|d| d.starts_with("sqlite:") || d.starts_with("file:"))
        });
    nommee.then(|| "database.sqlite".to_string())
}

/// Une variable propre a un moteur client-serveur : `MARIADB_DATABASE` chez
/// Puter ne designe pas sa base SQLite.
fn d_un_autre_moteur(nom: &str) -> bool {
    let n = nom.to_ascii_uppercase();
    ["MYSQL", "MARIADB", "POSTGRES", "PG_", "PGDATABASE", "MONGO"]
        .iter()
        .any(|m| n.contains(m))
}

/// Les variables qui designent la base SQLite, pointees sur `data_dir`.
fn liaison_sqlite(facts: &RepoFacts, fichier: &str) -> String {
    let chemin = format!("__DATA_DIR__/{fichier}");
    let mut lignes = Vec::new();
    for v in facts
        .config
        .variables
        .iter()
        .filter(|v| !d_un_autre_moteur(&v.name))
    {
        match v.role {
            ConfigRole::DatabaseName => lignes.push(format!("{}={chemin}", v.name)),
            ConfigRole::DatabaseUrl => match v.default.as_deref() {
                // `sqlite:///` puis un chemin absolu : la forme de SQLAlchemy
                // et de dj-database-url.
                Some(d) if d.starts_with("sqlite:") => {
                    lignes.push(format!("{}=sqlite:///{chemin}", v.name))
                }
                Some(d) if d.starts_with("file:") => {
                    lignes.push(format!("{}=file:{chemin}", v.name))
                }
                _ => {}
            },
            ConfigRole::DatabaseType => lignes.push(format!("{}=sqlite", v.name)),
            _ => {}
        }
    }
    lignes.join("\n")
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
        ConfigRole::Domain => "__DOMAIN__",
        ConfigRole::DataPath => "__DATA_DIR__",
        ConfigRole::Secret => "__SECRET__",
        // Le port et la base passent par les liaisons dediees, qui savent les
        // composer et signalent leur absence.
        _ => return None,
    })
}

fn features(facts: &RepoFacts) -> Features {
    let php = facts.stack.primary == Technology::Php;
    // Un service systemd n'a de sens que si l'on sait quoi demarrer : pour
    // une application PHP, sa file de taches si elle en a une.
    let daemon =
        (!php && facts.stack.primary != Technology::Static && statique::site(facts).is_none())
            || php::file_de_taches(facts).is_some();

    Features {
        nginx: true,
        systemd: daemon,
        phpfpm: php,
        logrotate: daemon,
        // Les motifs de detection d'intrusion demandent de connaitre le format
        // des journaux de l'application : on ne les invente pas.
        fail2ban: false,
        cron: !php::cron(facts).is_empty(),
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
    // Un site servi par nginx n'ecoute sur rien : un port reserve ferait
    // echouer le test qui verifie que quelque chose y repond.
    if statique::site(facts).is_some() || facts.stack.primary == Technology::Static {
        return false;
    }
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
                moteurs_cites: Default::default(),
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
            format_secret: None,
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
        // L'URL n'est pas la variable du port ; le port publie se reserve.
        assert_eq!(liaison_port(&f, None).value().map(String::as_str), Some(""));
        assert_eq!(port_fixe_du_code(&f), Some(8080));
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
    fn le_moteur_de_base_est_donne_dans_le_vocabulaire_de_l_application() {
        let mut laravel = var("DB_CONNECTION", Some("sqlite"), ".env.example");
        laravel.role = ConfigRole::DatabaseType;
        let rien = Default::default();
        // Le code fait foi : Ferdium (AdonisJS) nomme DB_CONNECTION, et cite `pg`.
        let ferdium: std::collections::BTreeMap<String, usize> =
            [("pg".to_string(), 3), ("mysql".to_string(), 2)]
                .into_iter()
                .collect();
        assert_eq!(
            type_de_base(&laravel, Database::PostgreSql, &ferdium).as_deref(),
            Some("pg")
        );
        assert_eq!(
            type_de_base(&laravel, Database::PostgreSql, &rien).as_deref(),
            Some("pgsql")
        );
        assert_eq!(
            type_de_base(&laravel, Database::MySql, &rien).as_deref(),
            Some("mysql")
        );
        let mut strapi = var("DATABASE_CLIENT", Some("sqlite"), "config/database.js");
        strapi.role = ConfigRole::DatabaseType;
        assert_eq!(
            type_de_base(&strapi, Database::PostgreSql, &rien).as_deref(),
            Some("postgres")
        );
        let mut autre = var("DB_TYPE", Some("postgresql"), ".env");
        autre.role = ConfigRole::DatabaseType;
        assert_eq!(
            type_de_base(&autre, Database::PostgreSql, &rien).as_deref(),
            Some("postgresql")
        );
    }

    #[test]
    fn l_adresse_de_la_base_suit_le_schema_de_l_application() {
        let mut v = var(
            "AIRFLOW__DATABASE__SQL_ALCHEMY_CONN",
            Some("postgresql+psycopg2://airflow:airflow@postgres/airflow"),
            ".env",
        );
        v.role = ConfigRole::DatabaseUrl;
        let a = "postgres://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__?sslmode=disable";
        assert_eq!(
            avec_le_schema_attendu(a, &v, Database::PostgreSql),
            "postgresql+psycopg2://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__?sslmode=disable"
        );
        // Sans exemple, une variable SQLAlchemy recoit le schema qu'il accepte.
        v.default = None;
        assert!(avec_le_schema_attendu(a, &v, Database::PostgreSql).starts_with("postgresql://"));
        // Ailleurs, rien ne change.
        let mut d = var("DATABASE_URL", None, ".env");
        d.role = ConfigRole::DatabaseUrl;
        assert_eq!(avec_le_schema_attendu(a, &d, Database::PostgreSql), a);
    }

    #[test]
    fn le_service_travaille_ou_travaille_l_image() {
        let mut f = faits(vec![], vec![3000]);
        f.build.as_mut().unwrap().workdir = Some("/app/apps/remix".into());
        f.tree = vec!["apps/remix/package.json".into()];
        assert_eq!(repertoire_de_travail(&f).as_deref(), Some("apps/remix"));
        // Un repertoire absent du depot ne se retient pas.
        f.tree = vec!["package.json".into()];
        assert_eq!(repertoire_de_travail(&f), None);
        // La racine de l'image non plus.
        f.build.as_mut().unwrap().workdir = Some("/app".into());
        assert_eq!(repertoire_de_travail(&f), None);
    }

    #[test]
    fn les_processus_du_procfile_deviennent_des_services() {
        use ynp_core::facts::CommandeDeclaree;
        let mut f = faits(vec![], vec![]);
        f.stack.primary = Technology::Ruby;
        f.tree = vec!["bin/rails".into()];
        let p = |nom: &str, c: &str| CommandeDeclaree {
            commande: c.into(),
            ou: nom.into(),
        };
        f.stack.processus_du_procfile = vec![
            p("worker", "bundle exec sidekiq -C config/sidekiq.yml"),
            p("release", "bundle exec rails db:migrate"),
            p("css", "bin/rails tailwindcss:watch"),
        ];
        let mut migrations = Vec::new();
        let (annexes, intraduisible) = processus_annexes(&f, &mut migrations);
        assert_eq!(intraduisible, None);
        assert_eq!(annexes.len(), 1);
        assert_eq!(annexes[0].nom, "worker");
        assert_eq!(
            annexes[0].execstart,
            "__RUBY_DIR__/bundle exec sidekiq -C config/sidekiq.yml"
        );
        assert_eq!(
            migrations,
            vec!["__RUBY_DIR__/bundle exec rails db:migrate"]
        );
    }

    #[test]
    fn une_base_sqlite_vit_dans_le_repertoire_de_donnees() {
        let role = |nom: &str, r: ConfigRole, d: Option<&str>| ConfigVar {
            name: nom.into(),
            role: r,
            default: d.map(String::from),
            ..Default::default()
        };
        let mut f = faits(
            vec![
                role("DB_CONNECTION", ConfigRole::DatabaseType, Some("sqlite")),
                role("DB_DATABASE", ConfigRole::DatabaseName, None),
                role("MARIADB_DATABASE", ConfigRole::DatabaseName, None),
            ],
            vec![],
        );
        f.services.database = Database::Sqlite;
        assert_eq!(fichier_sqlite(&f).as_deref(), Some("database.sqlite"));
        assert_eq!(
            liaison_base(&f).value().map(String::as_str),
            Some("DB_CONNECTION=sqlite\nDB_DATABASE=__DATA_DIR__/database.sqlite")
        );
        // Seule une variable d'un autre moteur : rien a pointer.
        f.config.variables.remove(1);
        f.config.variables.remove(0);
        assert_eq!(fichier_sqlite(&f), None);
    }

    #[test]
    fn un_service_sans_reglage_de_chemin_demande_un_domaine() {
        let mut f = faits(vec![var("SECRET_KEY", None, ".env.example")], vec![8080]);
        f.stack.primary = Technology::Python;
        assert!(sans_reglage_de_chemin(&f));
        // Une adresse publique ne suffit pas (anonymousoverflow).
        let mut url = var("APP_BASE_URL", None, ".env.example");
        url.role = ConfigRole::BaseUrl;
        f.config.variables.push(url);
        assert!(sans_reglage_de_chemin(&f));
        // Pas celle d'un service tiers.
        f.config.variables.clear();
        let mut sms = var("APP_SMS_BASE_URL", None, ".env.example");
        sms.role = ConfigRole::BaseUrl;
        f.config.variables.push(sms);
        assert!(sans_reglage_de_chemin(&f));
        f.config
            .variables
            .push(var("APP_BASE_PATH", None, ".env.example"));
        assert!(!sans_reglage_de_chemin(&f));
        f.config.variables.clear();
        f.stack.primary = Technology::Php;
        assert!(!sans_reglage_de_chemin(&f));
    }

    #[test]
    fn l_adresse_et_les_champs_de_la_base_se_donnent_ensemble() {
        // Cas d'anonaddy : DB_URL, que Laravel ne lisait pas, et DB_*.
        let role = |nom: &str, r: ConfigRole| ConfigVar {
            name: nom.into(),
            role: r,
            ..Default::default()
        };
        let mut f = faits(
            vec![
                role("DB_URL", ConfigRole::DatabaseUrl),
                role("DB_HOST", ConfigRole::DatabaseHost),
                role("DB_DATABASE", ConfigRole::DatabaseName),
                role("DB_USERNAME", ConfigRole::DatabaseUser),
                role("DB_PASSWORD", ConfigRole::DatabasePassword),
            ],
            vec![],
        );
        f.services.database = Database::MySql;
        let l = liaison_base(&f).value().cloned().unwrap();
        assert!(l.contains("DB_URL=mysql://"));
        assert!(l.contains("DB_USERNAME=__DB_USER__"));
        assert!(l.contains("DB_PASSWORD=__DB_PWD__"));
    }

    #[test]
    fn la_memoire_de_construction_suit_ce_qu_il_faut_construire() {
        let mut f = faits(vec![], vec![]);
        f.stack.primary = Technology::NodeJs;
        assert_eq!(memoire_de_construction(&f), "1G");
        f.stack.dependances_npm = vec!["next".into()];
        assert_eq!(memoire_de_construction(&f), "2G");
        f.stack.dependances_npm.clear();
        f.stack.primary = Technology::Rust;
        assert_eq!(memoire_de_construction(&f), "2G");
        f.stack.primary = Technology::Python;
        assert_eq!(memoire_de_construction(&f), "500M");
    }

    #[test]
    fn auth_js_et_l_hote_d_ecoute_de_next() {
        // Cas de benevoles : AUTH_SECRET exige a la construction, et le
        // HOSTNAME=0.0.0.0 du serveur autonome de Next.js.
        let role = |nom: &str, r: ConfigRole, d: Option<&str>| ConfigVar {
            name: nom.into(),
            role: r,
            default: d.map(String::from),
            ..Default::default()
        };
        let f = faits(
            vec![
                role("AUTH_SECRET", ConfigRole::Secret, None),
                role(
                    "AUTH_URL",
                    ConfigRole::BaseUrl,
                    Some("http://localhost:3000"),
                ),
                role("HOSTNAME", ConfigRole::BaseUrl, Some("0.0.0.0")),
                role("OIDC_CLIENT_SECRET", ConfigRole::Secret, None),
            ],
            vec![3000],
        );
        let env = runtime(&f, "app").env;
        assert_eq!(
            env.get("AUTH_SECRET").map(String::as_str),
            Some("__SECRET__")
        );
        assert_eq!(
            env.get("AUTH_URL").map(String::as_str),
            Some("https://__DOMAIN____PATH__")
        );
        assert_eq!(env.get("HOSTNAME").map(String::as_str), Some("127.0.0.1"));
        assert_eq!(env.get("OIDC_CLIENT_SECRET"), None);
    }

    #[test]
    fn un_secret_interne_se_genere_meme_facultatif() {
        assert!(secret_interne("JWT_SIGNING_SECRET"));
        assert!(secret_interne("SECRET_KEY"));
        assert!(secret_interne("AUTH_SECRET"));
        assert!(secret_interne("NEXTAUTH_SECRET"));
        assert!(!secret_interne("GITHUB_OAUTH_SECRET"));
        assert!(!secret_interne("STRIPE_SECRET_KEY"));
        assert!(secret_interne("SESSION_SECRET"));
        assert!(!secret_interne("API_KEY"));
        assert!(!secret_interne("BUZZ_RELAY_PRIVATE_KEY"));
        assert!(!secret_interne("MAILGUN_SECRET"));
    }

    #[test]
    fn un_script_de_preparation_de_l_image_ne_se_transpose_pas() {
        let mut f = faits(vec![], vec![]);
        f.build.as_mut().unwrap().build_steps =
            vec!["npm ci".into(), "bash /bd_build/prepare.sh".into()];
        assert_eq!(script_d_image(&f).as_deref(), Some("/bd_build/prepare.sh"));
        f.build.as_mut().unwrap().build_steps = vec!["/usr/bin/env node build.js".into()];
        assert_eq!(script_d_image(&f), None);
        // Un repertoire de sortie n'est pas un script.
        f.build.as_mut().unwrap().build_steps =
            vec!["cmake -S . -B /build && cmake --build /build".into()];
        assert_eq!(script_d_image(&f), None);
        f.build.as_mut().unwrap().build_steps =
            vec!["chmod +x /docker-entrypoint.d/start.sh".into()];
        assert_eq!(script_d_image(&f), None);
    }

    #[test]
    fn gradle_et_maven_passent_par_le_lanceur_du_projet() {
        let mut f = faits(vec![], vec![]);
        f.tree = vec!["booklore-api/gradlew".into(), "mvnw".into()];
        assert_eq!(
            lanceurs_du_projet(
                vec![
                    "cd booklore-api && gradle clean build -x test".into(),
                    "mvn package".into(),
                    "gradle build".into(),
                ],
                &f
            ),
            vec![
                "cd booklore-api && ./gradlew clean build -x test",
                "./mvnw package",
                "gradle build",
            ]
        );
    }

    #[test]
    fn un_outil_npm_global_s_installe_dans_l_installation() {
        let e = npm_global_dans_l_installation(vec![
            "npm install -g turbo@2".into(),
            "turbo prune --scope=web".into(),
        ]);
        assert_eq!(
            e[0],
            "export PATH=\"$PWD/.npm-global/bin:$PATH\"; npm install -g --prefix \"$PWD/.npm-global\" turbo@2"
        );
        assert_eq!(
            e[1],
            "export PATH=\"$PWD/.npm-global/bin:$PATH\"; turbo prune --scope=web"
        );
        assert_eq!(
            npm_global_dans_l_installation(vec!["npm ci".into()]),
            vec!["npm ci"]
        );
        // Option groupee : `npm i -gf` chez ferdium.
        assert!(
            npm_global_dans_l_installation(vec!["npm i -gf @adonisjs/cli".into()])[0]
                .contains("-gf --prefix \"$PWD/.npm-global\"")
        );
    }

    #[test]
    fn yarn_et_pnpm_passent_par_corepack() {
        let e = yarn_et_pnpm_par_corepack(vec![
            "yarn build".into(),
            "if [ -f yarn.lock ]; then yarn --frozen-lockfile; elif [ -f pnpm-lock.yaml ]; then corepack enable pnpm && pnpm i --frozen-lockfile; fi".into(),
            "corepack pnpm install".into(),
            "npm run build".into(),
        ]);
        let r = "export COREPACK_HOME=\"$PWD/.corepack\" PATH=\"$PWD/.corepack/bin:$PATH\"; mkdir -p .corepack/bin && corepack enable --install-directory .corepack/bin pnpm yarn;";
        assert_eq!(e[0], format!("{r} corepack yarn build"));
        assert_eq!(
            e[1],
            format!("{r} if [ -f yarn.lock ]; then corepack yarn --frozen-lockfile; elif [ -f pnpm-lock.yaml ]; then true && corepack pnpm i --frozen-lockfile; fi")
        );
        assert_eq!(e[2], format!("{r} corepack pnpm install"));
        assert_eq!(e[3], "npm run build");
    }

    #[test]
    fn bun_passe_par_npx() {
        assert_eq!(
            bun_par_npx(vec![
                "bun install --frozen-lockfile".into(),
                "cd web && bun run build".into(),
                "echo bun".into(),
            ]),
            vec![
                "npx --yes bun install --frozen-lockfile",
                "cd web && npx --yes bun run build",
                "echo bun",
            ]
        );
    }

    #[test]
    fn une_installation_npm_tolere_les_conflits_de_pairs() {
        let e = avec_les_pairs_tolerees(vec![
            "npm ci".into(),
            "npm install --omit=dev".into(),
            "npm install -g pm2".into(),
            "npm run build".into(),
        ]);
        assert_eq!(e[0], "npm ci || npm ci --legacy-peer-deps");
        assert_eq!(
            e[1],
            "npm install --omit=dev || npm install --omit=dev --legacy-peer-deps"
        );
        assert_eq!(e[2], "npm install -g pm2");
        assert_eq!(e[3], "npm run build");
    }

    #[test]
    fn la_copie_du_depot_dans_l_image_designe_l_installation() {
        let mut f = faits(vec![], vec![]);
        f.build
            .as_mut()
            .unwrap()
            .copies
            .insert("/buku".into(), ".".into());
        let e = relatives_au_repertoire_de_travail(
            vec![
                "pip install -U pip gunicorn /buku[server]".into(),
                "ls /buku/x".into(),
            ],
            &f,
        );
        assert_eq!(e, vec!["pip install -U pip gunicorn .[server]", "ls ./x"]);
    }

    #[test]
    fn un_outil_de_l_image_va_dans_l_installation() {
        let e = outils_dans_l_installation(vec![
            "mv /tmp/uv-x86_64-unknown-linux-gnu/uv /usr/bin/uv".into(),
            "tar -xzf /tmp/caddy.tar.gz -C /usr/bin caddy".into(),
            "curl -fsSL https://x/composer.phar -o /usr/local/bin/composer".into(),
            "/usr/bin/env python3 build.py".into(),
        ]);
        assert_eq!(e[0], "mv /tmp/uv-x86_64-unknown-linux-gnu/uv $PWD/uv");
        assert_eq!(e[1], "tar -xzf /tmp/caddy.tar.gz -C $PWD caddy");
        assert_eq!(e[2], "curl -fsSL https://x/composer.phar -o $PWD/composer");
        // Lire un outil du systeme n'est pas en poser un.
        assert_eq!(e[3], "/usr/bin/env python3 build.py");
    }

    #[test]
    fn le_serveur_d_un_monorepo_vit_dans_son_repertoire() {
        let mut r = Runtime {
            build_steps: vec!["go build -o counter .".into()],
            execstart: Known::resolved("__INSTALL_DIR__/counter".into()),
            migrations: vec!["__INSTALL_DIR__/venv/bin/python manage.py migrate".into()],
            ..Default::default()
        };
        dans_le_sous_repertoire(&mut r, "backend");
        assert_eq!(r.build_steps, vec!["cd backend && go build -o counter ."]);
        assert_eq!(
            r.execstart.value().map(String::as_str),
            Some("__INSTALL_DIR__/backend/counter")
        );
        assert_eq!(
            r.migrations[0],
            "__INSTALL_DIR__/venv/bin/python manage.py migrate"
        );
        assert_eq!(r.repertoire_de_travail.as_deref(), Some("backend"));
    }

    #[test]
    fn les_outils_des_etapes_sont_installes() {
        let mut f = faits(vec![], vec![]);
        f.stack.primary = Technology::NodeJs;
        let mut r = Runtime {
            build_steps: vec![
                "pip install mkdocs".into(),
                "wget -q https://x/y.tgz".into(),
                "bundle install".into(),
            ],
            ..Default::default()
        };
        let mut res = Resources::default();
        outils_des_etapes(&f, &mut r, &mut res);
        assert!(r.venv_python);
        assert_eq!(res.ruby_version.as_deref(), Some("3.3"));
        assert!(res.apt_packages.iter().any(|a| a == "libjemalloc-dev"));
        for p in ["python3-venv", "wget"] {
            assert!(res.apt_packages.iter().any(|a| a == p), "{p}");
        }
    }

    #[test]
    fn une_etape_qui_appelle_node_exige_node() {
        assert!(appelle_node(&[
            "bundle install".into(),
            "yarn install --frozen-lockfile".into()
        ]));
        assert!(appelle_node(&["cd front && npm ci".into()]));
        assert!(!appelle_node(
            &["bundle exec rake assets:precompile".into()]
        ));
    }

    #[test]
    fn rails_lit_database_url_et_port() {
        let mut f = faits(vec![], vec![3000]);
        f.stack.primary = Technology::Ruby;
        f.stack.cadre_web = Some("rails".into());
        f.services.database = Database::MySql;
        f.stack.dependances_ruby = vec!["rails".into(), "trilogy".into()];
        assert_eq!(
            liaison_port(&f, Some("__RUBY_DIR__/bundle exec rails server"))
                .value()
                .map(String::as_str),
            Some("PORT=__PORT__")
        );
        assert_eq!(
            liaison_base(&f).value().map(String::as_str),
            Some("DATABASE_URL=trilogy://__DB_USER__:__DB_PWD__@127.0.0.1/__DB_NAME__")
        );
    }

    #[test]
    fn spring_boot_donne_ses_propres_noms_au_port_et_a_la_base() {
        let mut f = faits(vec![], vec![8080]);
        f.stack.primary = Technology::Java;
        f.stack.cadre_web = Some("spring-boot".into());
        f.services.database = Database::PostgreSql;
        assert_eq!(
            liaison_port(&f, Some("/usr/bin/java -jar app.jar"))
                .value()
                .map(String::as_str),
            Some("SERVER_PORT=__PORT__\nSERVER_ADDRESS=127.0.0.1")
        );
        let b = liaison_base(&f);
        assert!(
            b.value().is_some_and(
                |v| v.starts_with("SPRING_DATASOURCE_URL=jdbc:postgresql://127.0.0.1:5432/")
            ),
            "{b:?}"
        );
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
    fn le_port_unique_de_l_image_se_reserve_en_dernier_recours() {
        // Cas de bytestash : EXPOSE 5000, aucune variable de port.
        let mut f = faits(vec![], vec![5000]);
        f.stack.primary = Technology::NodeJs;
        assert_eq!(port_fixe_du_code(&f), Some(5000));
        // Le port 80 est celui d'un serveur web place devant l'application.
        let mut g = faits(vec![], vec![80]);
        g.stack.primary = Technology::NodeJs;
        assert_eq!(port_fixe_du_code(&g), None);
        // Une image qui installe nginx non plus.
        if let Some(b) = f.build.as_mut() {
            b.apt_packages = vec!["nginx".into()];
        }
        assert_eq!(port_fixe_du_code(&f), None);
    }

    #[test]
    fn gunicorn_et_uvicorn_sans_adresse_recoivent_le_port() {
        let mut f = faits(vec![], vec![]);
        f.stack.primary = Technology::Python;
        let e = avec_l_option_de_port(
            &f,
            Known::resolved(
                "__INSTALL_DIR__/venv/bin/gunicorn babybuddy.wsgi:application --timeout 30".into(),
            ),
        );
        assert!(e
            .value()
            .is_some_and(|c| c.ends_with("--bind 127.0.0.1:__PORT__")));
        let e = avec_l_option_de_port(
            &f,
            Known::resolved("__INSTALL_DIR__/venv/bin/uvicorn main:app".into()),
        );
        assert_eq!(
            e.value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/uvicorn main:app --host 127.0.0.1 --port __PORT__")
        );
        // Une adresse deja donnee ne change pas.
        let c = "__INSTALL_DIR__/venv/bin/gunicorn a.wsgi -b 127.0.0.1:__PORT__";
        assert_eq!(
            avec_l_option_de_port(&f, Known::resolved(c.into()))
                .value()
                .map(String::as_str),
            Some(c)
        );
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
    fn installer_pnpm_pour_tout_le_systeme_passe_par_corepack() {
        let e = sans_activation_de_corepack(vec![
            "npm install -g pnpm".into(),
            "CI=true pnpm install --frozen-lockfile".into(),
            "pnpm build".into(),
        ]);
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e[0].contains("CI=true corepack pnpm install"), "{e:?}");
        assert!(e[1].contains("corepack pnpm build"), "{e:?}");
        // Un autre outil installe globalement n'est pas concerne.
        let e = sans_activation_de_corepack(vec!["npm install -g typescript".into()]);
        assert_eq!(e, vec!["npm install -g typescript".to_string()]);
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
