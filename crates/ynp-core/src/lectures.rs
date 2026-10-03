//! Variables d'environnement que le code d'une application lit.
//!
//! La question qui bloquait le plus souvent l'outil etait : sous quel nom
//! l'application attend-elle son port ? Les fichiers d'exemple, le compose et
//! la documentation ne le disent pas toujours. Le code, lui, le dit toujours :
//! `process.env.PORT`, `os.Getenv("PORT")`, `ENV.fetch("PORT")`. Ce n'est pas
//! de la prose a interpreter mais un nom litteral passe a une fonction de
//! lecture — la reponse exacte, a la source.
//!
//! On ne garde que les noms, pas le code : un depot de milliers de fichiers
//! source reste leger en memoire.

use regex::Regex;
use std::sync::OnceLock;

/// Extensions de fichiers source dont on releve les lectures.
const EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "ts", "mts", "cts", "go", "py", "rb", "rs", "php", "java", "kt", "ex",
    "exs",
];

/// Repertoires dont le code ne dit rien de l'application en production :
/// un serveur de test lit `PORT` lui aussi.
const HORS_PRODUCTION: &[&str] = &[
    "test",
    "tests",
    "spec",
    "specs",
    "__tests__",
    "__mocks__",
    "e2e",
    "fixtures",
    "testdata",
    "examples",
    "example",
    "mock",
    "mocks",
    "benchmarks",
    "docs",
    "scripts",
];

/// Vrai pour un fichier source dont les lectures valent pour l'application.
pub fn est_un_source_de_production(chemin: &str) -> bool {
    let bas = chemin.to_lowercase();
    let nom = bas.rsplit('/').next().unwrap_or(&bas);
    // Un executable sans extension sous `bin/` ou `exe/` : c'est la
    // convention des gems Ruby, et des paquets npm ou Python qui livrent une
    // commande. `bin/gollum` y declare son option `--port`.
    let executable = !nom.contains('.') && matches!(bas.rsplit('/').nth(1), Some("bin" | "exe"));
    // La configuration d'un cadre PHP (Symfony) lit ses variables dans
    // `config/packages/doctrine.yaml` : novosga y prend `DATABASE_URL`.
    let configuration =
        bas.starts_with("config/") && (nom.ends_with(".yaml") || nom.ends_with(".yml"));
    if !executable && !configuration {
        let Some((_, ext)) = nom.rsplit_once('.') else {
            return false;
        };
        if !EXTENSIONS.contains(&ext) {
            return false;
        }
    }
    if nom.contains(".test.") || nom.contains(".spec.") || nom.starts_with("test_") {
        return false;
    }
    !bas.split('/')
        .any(|seg| HORS_PRODUCTION.contains(&seg) || est_un_repertoire_de_test(seg))
}

/// `bigbluebutton-tests`, `integration_test`, `testing` : des repertoires de
/// test que la liste exacte ne nomme pas. `latest`, lui, n'en est pas un.
fn est_un_repertoire_de_test(seg: &str) -> bool {
    // `Fixture` : jeux de donnees de test (AzuraCast y lit INIT_ADMIN_PASSWORD).
    seg.contains("fixture")
        || seg.starts_with("test")
        || ["-test", "_test", "-tests", "_tests", "-e2e", "_e2e"]
            .iter()
            .any(|s| seg.ends_with(s))
}

fn motifs() -> &'static [Regex] {
    static M: OnceLock<Vec<Regex>> = OnceLock::new();
    M.get_or_init(|| {
        [
            // JavaScript, TypeScript
            r#"process\.env\.([A-Z][A-Z0-9_]*)"#,
            r#"process\.env\[\s*['"]([A-Z][A-Z0-9_]*)['"]\s*\]"#,
            // Go
            r#"os\.(?:Getenv|LookupEnv)\(\s*"([A-Z][A-Z0-9_]*)"\s*\)"#,
            // Python
            r#"os\.(?:getenv|environ\.get)\(\s*['"]([A-Z][A-Z0-9_]*)['"]"#,
            r#"os\.environ\[\s*['"]([A-Z][A-Z0-9_]*)['"]\s*\]"#,
            // Ruby
            r#"ENV\.fetch\(\s*['"]([A-Z][A-Z0-9_]*)['"]"#,
            r#"ENV\[\s*['"]([A-Z][A-Z0-9_]*)['"]\s*\]"#,
            // Rust
            r#"env::var(?:_os)?\(\s*"([A-Z][A-Z0-9_]*)"\s*\)"#,
            // PHP. Pas `$_SERVER` : il decrit la requete en cours —
            // `$_SERVER['SERVER_PORT']` est le port par lequel elle est
            // arrivee, pas un reglage. Racktables se voyait ainsi attribuer
            // un port a transmettre.
            r#"getenv\(\s*['"]([A-Z][A-Z0-9_]*)['"]\s*\)"#,
            r#"\$_ENV\[\s*['"]([A-Z][A-Z0-9_]*)['"]\s*\]"#,
            // Java, Kotlin
            r#"System\.getenv\(\s*"([A-Z][A-Z0-9_]*)"\s*\)"#,
            // Elixir
            r#"System\.(?:get_env|fetch_env!?)\(\s*"([A-Z][A-Z0-9_]*)""#,
            // Aide `env('NOM')` : Laravel, Strapi (`env.int('NOM')`),
            // django-environ. Strapi lit ainsi toute sa base dans
            // config/database.js, et aucune de ses variables n'etait vue.
            r#"\benv(?:\.\w+)?\(\s*['"]([A-Z][A-Z0-9_]*)['"]"#,
            // AdonisJS.
            r#"\bEnv\.get\(\s*['"]([A-Z][A-Z0-9_]*)['"]"#,
            // Symfony, dans config/ : `%env(resolve:DATABASE_URL)%`.
            r#"%env\((?:[a-z_]+:)*([A-Z][A-Z0-9_]*)\)%"#,
        ]
        .iter()
        .map(|m| Regex::new(m).expect("motif de lecture invalide"))
        .collect()
    })
}

/// Les valeurs par defaut que le code donne aux variables qu'il lit :
/// `env('DB_CONNECTION', 'mysql')` dans la configuration de Laravel. C'est
/// la valeur que l'application prend quand rien ne la regle — linguacafe,
/// sans fichier d'exemple, n'annoncait sa base MySQL que la.
pub fn defauts_lus(contenu: &str) -> Vec<(String, String)> {
    static M: OnceLock<Vec<Regex>> = OnceLock::new();
    let motifs = M.get_or_init(|| {
        [
            r#"\benv(?:\.\w+)?\(\s*['"]([A-Z][A-Z0-9_]*)['"]\s*,\s*['"]([^'"]*)['"]"#,
            r#"os\.(?:getenv|environ\.get)\(\s*['"]([A-Z][A-Z0-9_]*)['"]\s*,\s*['"]([^'"]*)['"]"#,
            r#"ENV\.fetch\(\s*['"]([A-Z][A-Z0-9_]*)['"]\s*,\s*['"]([^'"]*)['"]"#,
            r#"process\.env\.([A-Z][A-Z0-9_]*)\s*(?:\|\||\?\?)\s*['"]([^'"]*)['"]"#,
        ]
        .iter()
        .map(|m| Regex::new(m).expect("motif de valeur par defaut invalide"))
        .collect()
    });
    let mut out: Vec<(String, String)> = Vec::new();
    for m in motifs {
        for c in m.captures_iter(contenu) {
            if !out.iter().any(|(n, _)| n == &c[1]) {
                out.push((c[1].to_string(), c[2].to_string()));
            }
        }
    }
    out
}

/// Les noms de variables qu'un fichier source lit, dans l'ordre, sans doublon.
pub fn variables_lues(contenu: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in motifs() {
        for c in m.captures_iter(contenu) {
            let nom = c[1].to_string();
            if !out.contains(&nom) {
                out.push(nom);
            }
        }
    }
    out
}

/// Les noms de moteurs de base qu'un fichier source cite comme valeurs ou
/// comme cles : `'pgsql' => [` chez Laravel, `pg: {` chez AdonisJS,
/// `choices: ["pg", "mysql2"]` chez Kutt. Le meme reglage — le moteur a
/// utiliser — s'ecrit `pgsql`, `pg` ou `postgres` selon le cadre, et seul le
/// code dit lequel.
pub fn moteurs_cites(contenu: &str) -> Vec<String> {
    static M: OnceLock<Regex> = OnceLock::new();
    let m = M.get_or_init(|| {
        Regex::new(
            r#"["'](pgsql|pg|postgres|postgresql|mysql|mysql2|mariadb)["']|\b(pgsql|pg|postgres|postgresql|mysql|mysql2|mariadb)\s*:\s*\{"#,
        )
        .expect("motif de moteur invalide")
    });
    if !(contenu.contains("pg") || contenu.contains("postgres") || contenu.contains("mysql")) {
        return Vec::new();
    }
    m.captures_iter(contenu)
        .filter_map(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .map(|x| x.as_str().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_moteurs_cites_donnent_le_vocabulaire_du_cadre() {
        assert_eq!(
            moteurs_cites("'pgsql' => [ 'driver' => 'pgsql',"),
            vec!["pgsql", "pgsql"]
        );
        assert_eq!(
            moteurs_cites("connections: { pg: {\n client: 'pg',"),
            vec!["pg", "pg"]
        );
        assert_eq!(
            moteurs_cites(r#"DB_CLIENT: str({ choices: ["pg", "mysql2", "better-sqlite3"] })"#),
            vec!["pg", "mysql2"]
        );
        assert!(moteurs_cites("const page = 1;").is_empty());
    }

    #[test]
    fn chaque_langage_livre_ses_lectures() {
        let cas = [
            ("const p = process.env.PORT || 3000", "PORT"),
            ("const p = process.env['HTTP_PORT']", "HTTP_PORT"),
            ("addr := os.Getenv(\"LISTEN_ADDR\")", "LISTEN_ADDR"),
            ("host: env('DATABASE_HOST', '127.0.0.1'),", "DATABASE_HOST"),
            ("port: env.int('DATABASE_PORT', 5432),", "DATABASE_PORT"),
            ("'host' => env('DB_HOST', '127.0.0.1'),", "DB_HOST"),
            ("host: Env.get('DB_HOST')", "DB_HOST"),
            ("port = int(os.environ.get('PORT', 8000))", "PORT"),
            ("port = os.environ['APP_PORT']", "APP_PORT"),
            ("port ENV.fetch(\"PORT\") { 3000 }", "PORT"),
            ("let p = std::env::var(\"SERVER_PORT\")", "SERVER_PORT"),
            ("$p = getenv('PORT');", "PORT"),
            ("$p = $_ENV['PORT'];", "PORT"),
            ("int p = Integer.parseInt(System.getenv(\"PORT\"));", "PORT"),
        ];
        for (code, attendu) in cas {
            assert_eq!(variables_lues(code), vec![attendu.to_string()], "{code}");
        }
    }

    #[test]
    fn la_requete_php_n_est_pas_la_configuration() {
        // Cas de Racktables.
        assert!(variables_lues("$p = $_SERVER['SERVER_PORT'];").is_empty());
    }

    #[test]
    fn le_code_de_test_ne_compte_pas() {
        assert!(est_un_source_de_production("src/server.js"));
        assert!(est_un_source_de_production("config/puma.rb"));
        assert_eq!(
            defauts_lus(
                "'default' => env('DB_CONNECTION', 'mysql'),\nport: process.env.PORT || '3000'"
            ),
            vec![
                ("DB_CONNECTION".to_string(), "mysql".to_string()),
                ("PORT".to_string(), "3000".to_string())
            ]
        );
        assert!(est_un_source_de_production("bin/gollum"));
        assert!(est_un_source_de_production("config/packages/doctrine.yaml"));
        assert_eq!(
            variables_lues("        url: '%env(resolve:DATABASE_URL)%'"),
            vec!["DATABASE_URL"]
        );
        assert!(!est_un_source_de_production("LICENSE"));
        assert!(!est_un_source_de_production("bin/setup.cfg"));
        assert!(!est_un_source_de_production("test/server.js"));
        assert!(!est_un_source_de_production("src/app.test.ts"));
        assert!(!est_un_source_de_production("tests/test_api.py"));
        assert!(!est_un_source_de_production("README.md"));
        // Cas de BigBlueButton.
        assert!(!est_un_source_de_production(
            "bigbluebutton-tests/playwright/core/parameters.ts"
        ));
        assert!(est_un_source_de_production("releases/latest/server.js"));
        // Cas d'AzuraCast : des jeux de donnees de test.
        assert!(!est_un_source_de_production(
            "backend/src/Entity/Fixture/User.php"
        ));
    }
}
