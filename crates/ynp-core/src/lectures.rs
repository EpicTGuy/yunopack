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
    let Some((_, ext)) = bas.rsplit_once('.') else {
        return false;
    };
    if !EXTENSIONS.contains(&ext) {
        return false;
    }
    let nom = bas.rsplit('/').next().unwrap_or(&bas);
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
        ]
        .iter()
        .map(|m| Regex::new(m).expect("motif de lecture invalide"))
        .collect()
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaque_langage_livre_ses_lectures() {
        let cas = [
            ("const p = process.env.PORT || 3000", "PORT"),
            ("const p = process.env['HTTP_PORT']", "HTTP_PORT"),
            ("addr := os.Getenv(\"LISTEN_ADDR\")", "LISTEN_ADDR"),
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
