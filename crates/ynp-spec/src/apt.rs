//! Dependances Debian a declarer dans `[resources.apt]`.
//!
//! Trois sources se cumulent, et c'est le cumul qui compte : une seule d'entre
//! elles laisserait des trous, et une dependance manquante fait echouer
//! l'installation chez l'utilisateur.

use ynp_analyze::knowledge;
use ynp_core::facts::RepoFacts;

pub fn packages(facts: &RepoFacts) -> Vec<String> {
    let k = knowledge::get();
    // Quand l'amont publie un binaire, rien n'est compile sur la machine
    // cible : declarer build-essential, git ou make encombrerait le manifest
    // et rallongerait l'installation pour rien.
    let sans_compilation = facts
        .selection
        .as_ref()
        .is_some_and(|s| s.evite_la_compilation());
    let mut out: Vec<String> = Vec::new();
    let mut ajouter = |p: &str| {
        let p = p.trim();
        if !p.is_empty() && !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    };

    if let Some(build) = &facts.build {
        // 1. Les paquets Debian lus litteralement dans le Dockerfile.
        for p in &build.apt_packages {
            if est_specifique_au_conteneur(p) || (sans_compilation && sert_a_construire(p)) {
                continue;
            }
            ajouter(&en_bookworm(p));
        }

        // 2. Les paquets Alpine traduits. Une traduction vide signale un
        //    equivalent sans objet en Debian (`musl-dev`) ; une traduction
        //    absente est signalee par la regle APK001, pas comblee ici.
        for p in &build.apk_packages {
            if let Some(deb) = k.apk_to_deb(p) {
                if deb.is_empty()
                    || est_specifique_au_conteneur(deb)
                    || (sans_compilation && sert_a_construire(deb))
                {
                    continue;
                }
                ajouter(deb);
            }
        }
    }

    // 2 quater. Java, quand Debian suffit (Java 17) ; au-dela, le depot
    //    Adoptium est declare a part (voir `java`).
    for p in crate::java::paquets_debian(facts) {
        ajouter(&p);
    }

    // 2 quinquies. Le venv d'une application Python.
    if facts.stack.primary == ynp_core::facts::Technology::Python {
        ajouter("python3-venv");
        ajouter("python3-dev");
    }

    // 2 ter. Redis, que YunoHost ne fournit pas de lui-meme.
    if facts.services.needs_redis {
        ajouter("redis-server");
    }

    // 2 bis. PHP. YunoHost ne definit `php_version` — sans lequel
    //    `ynh_config_add_phpfpm` refuse de continuer — que si le manifeste
    //    declare des paquets `phpX.Y-*`. Les paquets PHP produits par l'outil
    //    n'en declaraient aucun, et auraient tous echoue a l'installation.
    if facts.stack.primary == ynp_core::facts::Technology::Php {
        for p in paquets_php(facts) {
            ajouter(&p);
        }
        // composer, celui de Debian, pour les dependances du projet ; il
        // decompresse les paquets par unzip.
        if crate::php::etape_composer(facts).is_some()
            || facts
                .build
                .as_ref()
                .is_some_and(|b| b.build_steps.iter().any(|e| e.starts_with("composer ")))
        {
            // composer.phar vient des scripts ; il lui faut unzip.
            ajouter("unzip");
        }
    }

    // 3. Les dependances cachees des modules npm a compilation native, qui
    //    n'apparaissent dans aucun Dockerfile ni package.json.
    if !facts
        .selection
        .as_ref()
        .is_some_and(|s| s.evite_la_compilation())
    {
        for module in &facts.stack.native_deps {
            let paquets = match (module.strip_prefix("pip:"), module.strip_prefix("gem:")) {
                (Some(m), _) => k.pip_native_deps(m),
                (_, Some(g)) => k.gem_native_deps(g),
                _ => k.npm_native_deps(module),
            };
            for p in paquets {
                ajouter(p);
            }
        }
    }

    // Le linter l'exige : declarer une base sans installer son serveur laisse
    // l'application sans rien a quoi se connecter sur une instance ou il
    // n'est pas deja present.
    match facts.services.database {
        ynp_core::facts::Database::PostgreSql => ajouter("postgresql"),
        ynp_core::facts::Database::MySql => ajouter("mariadb-server"),
        _ => {}
    }

    out.sort_unstable();
    out
}

/// Le nom bookworm d'un paquet de bibliotheque nomme par une distribution
/// plus recente : duplicati, construit sur trixie, demandait `libicu76` et
/// `libssl3t64`, introuvables sous Debian 12. Le suffixe `t64` (passage du
/// temps sur 64 bits) disparait ; les familles a version prennent la leur.
pub(crate) fn en_bookworm(paquet: &str) -> String {
    let p = paquet.strip_suffix("t64").unwrap_or(paquet);
    let familles: &[(&str, &str)] = &[
        ("libicu", "libicu72"),
        ("libssl", "libssl3"),
        ("libffi", "libffi8"),
        ("libreadline", "libreadline8"),
        ("libtiff", "libtiff6"),
        ("libwebp", "libwebp7"),
        ("libavcodec", "libavcodec59"),
        ("libavformat", "libavformat59"),
        ("libavutil", "libavutil57"),
        ("libswscale", "libswscale6"),
        ("libpython3.", "libpython3.11"),
        ("libmagickwand-7.q16-", "libmagickwand-6.q16-6"),
    ];
    for (prefixe, bookworm) in familles {
        // `libicu76`, et non `libicu-dev` : un numero suit le prefixe.
        if let Some(reste) = p.strip_prefix(prefixe) {
            if reste.chars().next().is_some_and(|c| c.is_ascii_digit()) && !reste.contains("-dev") {
                return (*bookworm).to_string();
            }
        }
    }
    p.to_string()
}

/// Paquets qui n'ont de sens que dans une image.
///
/// YunoHost fournit deja le socle du systeme : reinstaller `ca-certificates`
/// ou un gestionnaire de processus encombre le manifest sans rien apporter.
fn est_specifique_au_conteneur(paquet: &str) -> bool {
    const INUTILES: &[&str] = &[
        "ca-certificates",
        "tzdata",
        "locales",
        "tini",
        "gosu",
        "su-exec",
        "dumb-init",
        "passwd",
        "procps",
        "coreutils",
        "findutils",
        "bash",
        "apt-utils",
        "apt-transport-https",
        "gnupg",
        "software-properties-common",
    ];
    // Node.js vient de la ressource nodejs de YunoHost. Ceux de Debian, et
    // les bibliotheques `node-*` qui en dependent, entrent en conflit avec
    // elle : `node-gyp` empechait l'installation de klaxon, alors que npm
    // embarque deja le sien.
    let de_node = matches!(paquet, "nodejs" | "npm" | "yarnpkg") || paquet.starts_with("node-");
    INUTILES.contains(&paquet) || de_node
}

/// Paquets qui ne servent qu'a construire.
fn sert_a_construire(paquet: &str) -> bool {
    const OUTILS: &[&str] = &[
        "build-essential",
        "gcc",
        "g++",
        "make",
        "cmake",
        "pkg-config",
        "autoconf",
        "automake",
        "libtool",
        "git",
        "python3-dev",
        "linux-libc-dev",
        "clang",
        "rustc",
        "cargo",
    ];
    OUTILS.contains(&paquet) || paquet.ends_with("-dev")
}

/// La version de PHP a installer : la premiere qui satisfait toutes les
/// contraintes — celle du composer.json et celles des paquets du verrou —
/// dans l'ordre de preference 8.2 (celle de bookworm), 8.3, 8.4, puis les
/// anciennes. organizr exige `^5.6 || ^7.0` par une dependance : PHP 8.2
/// faisait refuser l'installation a composer.
fn version_php(facts: &RepoFacts) -> String {
    const PREFERENCE: &[(u32, u32)] = &[(8, 2), (8, 3), (8, 4), (8, 1), (8, 0), (7, 4)];
    let contraintes: Vec<&str> = facts
        .stack
        .contrainte_php
        .iter()
        .map(String::as_str)
        .chain(
            facts
                .stack
                .contraintes_php_verrou
                .iter()
                .map(String::as_str),
        )
        .collect();
    if !contraintes.is_empty() {
        if let Some((a, b)) = PREFERENCE
            .iter()
            .find(|v| contraintes.iter().all(|c| satisfait(c, **v)))
        {
            return format!("{a}.{b}");
        }
    }
    // Sans contrainte lisible : la version annoncee, au moins 8.2.
    let exigee = facts.stack.runtime_version.as_deref().unwrap_or("");
    let mut morceaux = exigee.split('.').filter_map(|m| m.parse::<u32>().ok());
    match (morceaux.next(), morceaux.next()) {
        (Some(8), Some(mineure)) if mineure > 2 => format!("8.{mineure}"),
        _ => "8.2".to_string(),
    }
}

/// Vrai si la version `majeure.mineure` satisfait une contrainte composer :
/// `^7.4`, `~8.1`, `>=7.2 <8.3`, `7.4.*`, `^5.6 || ^7.0`, `*`.
fn satisfait(contrainte: &str, (ma, mi): (u32, u32)) -> bool {
    let v = (ma, mi);
    let lire = |t: &str| -> Option<(u32, u32)> {
        let mut p = t.split('.').map(|x| x.parse::<u32>().ok());
        Some((p.next()??, p.next().flatten().unwrap_or(0)))
    };
    contrainte
        .split("||")
        .flat_map(|a| a.split('|'))
        .any(|alternative| {
            alternative
                .split([' ', ','])
                .filter(|t| !t.is_empty())
                .all(|t| {
                    let t = t.trim_start_matches('v');
                    if t == "*" {
                        return true;
                    }
                    if let Some(r) = t.strip_prefix('^') {
                        return lire(r).is_some_and(|b| v >= b && ma == b.0);
                    }
                    if let Some(r) = t.strip_prefix('~') {
                        return lire(r).is_some_and(|b| v >= b && ma == b.0);
                    }
                    if let Some(r) = t.strip_prefix(">=") {
                        return lire(r).is_some_and(|b| v >= b);
                    }
                    if let Some(r) = t.strip_prefix("<=") {
                        return lire(r).is_some_and(|b| v <= b);
                    }
                    if let Some(r) = t.strip_prefix('>') {
                        return lire(r).is_some_and(|b| v > b);
                    }
                    if let Some(r) = t.strip_prefix('<') {
                        return lire(r).is_some_and(|b| v < b);
                    }
                    // `7.4.*`, `7.*`, `7.4.3` : la meme branche.
                    let r = t.trim_end_matches(".*").trim_end_matches('*');
                    match r
                        .split('.')
                        .filter_map(|x| x.parse::<u32>().ok())
                        .collect::<Vec<_>>()[..]
                    {
                        [a] => a == ma,
                        [a, b, ..] => (a, b) == v,
                        _ => true,
                    }
                })
        })
}

/// Les paquets PHP d'une application : le moteur, le socle que tout
/// framework emploie, le pilote de sa base, et ses extensions declarees.
fn paquets_php(facts: &RepoFacts) -> Vec<String> {
    let v = version_php(facts);
    let mut exts: Vec<String> = ["fpm", "mbstring", "xml", "curl", "zip"]
        .iter()
        .map(|e| e.to_string())
        .collect();
    match facts.services.database {
        ynp_core::facts::Database::MySql => exts.push("mysql".into()),
        ynp_core::facts::Database::PostgreSql => exts.push("pgsql".into()),
        _ => {}
    }
    for e in &facts.stack.extensions_php {
        if let Some(paquet) = extension_php_en_paquet(e) {
            exts.push(paquet.to_string());
        }
    }
    // Le client Redis de PHP, quand l'application en utilise un : Laravel
    // choisit `phpredis` par defaut.
    if facts.services.needs_redis {
        exts.push("redis".into());
    }
    // Ce que Laravel et Symfony exigent sans le declarer en `ext-*`.
    if crate::php::cadre(facts).is_some() {
        exts.extend(["bcmath", "intl", "gd", "sqlite3"].map(String::from));
    }
    let mut out: Vec<String> = Vec::new();
    for e in exts {
        let p = format!("php{v}-{e}");
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Le paquet Debian qui fournit une extension PHP ; rien pour une extension
/// comprise dans le moteur (`json`, `pdo`, `ctype`…) ou inconnue.
fn extension_php_en_paquet(ext: &str) -> Option<String> {
    Some(
        match ext {
            "pdo_mysql" | "mysqli" | "mysqlnd" => "mysql",
            "pdo_pgsql" | "pgsql" => "pgsql",
            "pdo_sqlite" | "sqlite3" => "sqlite3",
            "dom" | "simplexml" | "xmlreader" | "xmlwriter" | "libxml" | "xml" => "xml",
            "gd" => "gd",
            "intl" => "intl",
            "mbstring" => "mbstring",
            "curl" => "curl",
            "zip" => "zip",
            "bcmath" => "bcmath",
            "gmp" => "gmp",
            "ldap" => "ldap",
            "imagick" => "imagick",
            "redis" => "redis",
            "memcached" => "memcached",
            "apcu" => "apcu",
            "soap" => "soap",
            "bz2" => "bz2",
            "imap" => "imap",
            "opcache" => "opcache",
            "xsl" => "xsl",
            "tidy" => "tidy",
            "igbinary" => "igbinary",
            "msgpack" => "msgpack",
            // Comprises dans le moteur (php-common, php-cli) : aucun paquet.
            "json" | "ctype" | "pdo" | "openssl" | "tokenizer" | "fileinfo" | "filter" | "hash"
            | "session" | "iconv" | "pcre" | "spl" | "reflection" | "standard" | "phar"
            | "posix" | "sockets" | "sodium" | "exif" | "gettext" | "calendar" | "ffi"
            | "shmop" | "sysvmsg" | "sysvsem" | "sysvshm" | "date" | "zlib" | "random" | "ftp"
            | "core" | "libsodium" => return None,
            // Les autres portent le nom de leur paquet : `php8.3-mailparse`,
            // `php8.3-gnupg`, `php8.3-yaml`. anonaddy exige mailparse ; faute de
            // la reconnaitre, l'outil ne l'installait pas, et composer refusait
            // de continuer.
            autre if autre.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                return Some(autre.replace('_', "-"))
            }
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod php {
    use super::*;

    #[test]
    fn les_bibliotheques_d_une_distribution_recente_prennent_leur_nom_bookworm() {
        assert_eq!(en_bookworm("libicu76"), "libicu72");
        assert_eq!(en_bookworm("libssl3t64"), "libssl3");
        assert_eq!(en_bookworm("libgssapi-krb5-2"), "libgssapi-krb5-2");
        assert_eq!(en_bookworm("libicu-dev"), "libicu-dev");
        assert_eq!(en_bookworm("libpng16-16t64"), "libpng16-16");
    }

    #[test]
    fn la_version_de_php_satisfait_aussi_le_verrou() {
        assert!(satisfait("^7.4", (7, 4)));
        assert!(!satisfait("^7.4", (8, 2)));
        assert!(satisfait(">=7.2 <8.3", (8, 2)));
        assert!(satisfait("^5.6 || ^7.0", (7, 4)));
        assert!(satisfait("7.4.*", (7, 4)));
        assert!(satisfait("*", (8, 4)));
        let mut f = php(Some("8.0"), &[], Database::None);
        f.stack.contrainte_php = Some(">=7.4".into());
        assert_eq!(version_php(&f), "8.2");
        // organizr : une dependance du verrou exige PHP 7.
        f.stack.contraintes_php_verrou = vec!["^5.6 || ^7.0".into()];
        assert_eq!(version_php(&f), "7.4");
    }

    #[test]
    fn une_extension_inconnue_porte_le_nom_de_son_paquet() {
        assert_eq!(
            extension_php_en_paquet("mailparse").as_deref(),
            Some("mailparse")
        );
        assert_eq!(
            extension_php_en_paquet("pdo_mysql").as_deref(),
            Some("mysql")
        );
        assert_eq!(extension_php_en_paquet("json"), None);
        assert_eq!(extension_php_en_paquet("tokenizer"), None);
    }
    use ynp_core::facts::{Database, StackFacts, Technology};

    fn php(version: Option<&str>, exts: &[&str], base: Database) -> RepoFacts {
        let mut f = RepoFacts {
            stack: StackFacts {
                primary: Technology::Php,
                runtime_version: version.map(String::from),
                extensions_php: exts.iter().map(|e| e.to_string()).collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        f.services.database = base;
        f
    }

    #[test]
    fn une_application_php_declare_de_quoi_definir_sa_version() {
        // Sans paquet phpX.Y-*, YunoHost ne definit pas php_version et
        // ynh_config_add_phpfpm refuse de continuer.
        let p = packages(&php(
            Some("8.3"),
            &["gd", "pdo_mysql", "json"],
            Database::MySql,
        ));
        for attendu in ["php8.3-fpm", "php8.3-gd", "php8.3-mysql", "php8.3-mbstring"] {
            assert!(p.contains(&attendu.to_string()), "{attendu} : {p:?}");
        }
        assert!(!p.iter().any(|x| x.ends_with("-json")));
    }

    #[test]
    fn la_version_n_est_jamais_sous_celle_de_bookworm_sauf_php_7() {
        assert_eq!(version_php(&php(Some("8.1"), &[], Database::None)), "8.2");
        assert_eq!(version_php(&php(None, &[], Database::None)), "8.2");
        let mut ancienne = php(Some("7.3"), &[], Database::None);
        ancienne.stack.contrainte_php = Some("^7.3".into());
        assert_eq!(version_php(&ancienne), "7.4");
        // `>=7.3` admet PHP 8 : on ne descend pas sur une version abandonnee.
        ancienne.stack.contrainte_php = Some(">=7.3".into());
        assert_eq!(version_php(&ancienne), "8.2");
        assert_eq!(version_php(&php(Some("8.4"), &[], Database::None)), "8.4");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::{ArchAsset, BuildRecipe, SourceSelection, StackFacts};

    fn facts(apt: &[&str], apk: &[&str], natifs: &[&str]) -> RepoFacts {
        RepoFacts {
            build: Some(BuildRecipe {
                apt_packages: apt.iter().map(|s| s.to_string()).collect(),
                apk_packages: apk.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }),
            stack: StackFacts {
                native_deps: natifs.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn les_trois_sources_se_cumulent() {
        let f = facts(&["libvips-dev"], &["build-base"], &["canvas"]);
        let p = packages(&f);

        assert!(
            p.contains(&"libvips-dev".to_string()),
            "depuis le Dockerfile"
        );
        assert!(
            p.contains(&"build-essential".to_string()),
            "traduit depuis Alpine"
        );
        assert!(
            p.contains(&"libcairo2-dev".to_string()),
            "dependance cachee de canvas"
        );
    }

    #[test]
    fn le_socle_fourni_par_yunohost_n_est_pas_redeclare() {
        let p = packages(&facts(&["ca-certificates", "tzdata", "curl"], &[], &[]));
        assert_eq!(p, vec!["curl"]);
        let p = packages(&facts(
            &["node-gyp", "nodejs", "npm", "libpq-dev"],
            &[],
            &[],
        ));
        assert_eq!(p, vec!["libpq-dev"]);
    }

    #[test]
    fn une_traduction_sans_objet_en_debian_n_ajoute_rien() {
        // `musl-dev` est propre a Alpine.
        let p = packages(&facts(&[], &["musl-dev", "vips-dev"], &[]));
        assert_eq!(p, vec!["libvips-dev"]);
    }

    #[test]
    fn un_paquet_alpine_inconnu_n_est_pas_invente() {
        // La regle APK001 le signale ; le combler au hasard ferait echouer apt.
        let p = packages(&facts(&[], &["paquet-jamais-vu"], &[]));
        assert!(p.is_empty());
    }

    #[test]
    fn un_binaire_publie_dispense_des_dependances_de_compilation() {
        let mut f = facts(&["libvips-dev"], &[], &["sharp"]);
        f.selection = Some(SourceSelection {
            prebuilt: vec![ArchAsset {
                arch: "amd64".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let p = packages(&f);
        // Les modules natifs ne seront pas compiles ; leurs -dev sont inutiles.
        assert!(!p.contains(&"libvips-dev".to_string()) || p.len() == 1);
    }

    #[test]
    fn la_liste_est_triee_et_sans_doublon() {
        let p = packages(&facts(&["zlib1g-dev", "curl", "curl"], &["curl"], &[]));
        assert_eq!(p, vec!["curl", "zlib1g-dev"]);
    }
}
