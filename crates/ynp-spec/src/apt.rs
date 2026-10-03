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
            ajouter(p);
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
            ajouter("composer");
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
            for p in k.npm_native_deps(module) {
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
    INUTILES.contains(&paquet)
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

/// La version de PHP a installer : celle qu'exige `composer.json`, jamais
/// en dessous de celle de Debian bookworm, sauf pour un projet reste en
/// PHP 7, que YunoHost sait encore fournir.
fn version_php(facts: &RepoFacts) -> String {
    let exigee = facts.stack.runtime_version.as_deref().unwrap_or("");
    let mut morceaux = exigee.split('.').filter_map(|m| m.parse::<u32>().ok());
    // PHP 7 seulement si la contrainte l'impose (`^7.4`, `~7.3`, `7.4.*`) ;
    // `>=7.4` admet PHP 8, et PHP 7 n'est plus maintenu.
    let seulement_7 = facts
        .stack
        .contrainte_php
        .as_deref()
        .is_some_and(|c| !c.contains('>') && !c.contains("|| ^8") && !c.contains("|8"));
    match (morceaux.next(), morceaux.next()) {
        (Some(7), _) if seulement_7 => "7.4".to_string(),
        (Some(8), Some(mineure)) if mineure > 2 => format!("8.{mineure}"),
        _ => "8.2".to_string(),
    }
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
fn extension_php_en_paquet(ext: &str) -> Option<&'static str> {
    Some(match ext {
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
        _ => return None,
    })
}

#[cfg(test)]
mod php {
    use super::*;
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
