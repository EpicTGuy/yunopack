//! Extraction d'une archive de depot vers un [`RepoTree`].
//!
//! On telecharge le tarball plutot que de cloner : l'analyse n'a pas besoin de
//! l'historique, et cela evite d'embarquer libgit2.
//!
//! Seul le contenu des fichiers utiles aux detecteurs est conserve. Les autres
//! chemins sont memorises sans leurs octets, ce qui permet de repondre « ce
//! fichier existe » sur un depot de plusieurs dizaines de milliers de fichiers
//! sans le charger en memoire.

use flate2::read::GzDecoder;
use std::io::Read;
use tar::Archive;
use ynp_core::tree::RepoTree;

/// Au-dela, un fichier n'est plus de la configuration : on n'en garde que le chemin.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Profondeur au-dela de laquelle un fichier de configuration n'en est plus un.
/// Evite de lire les fixtures de test de `node_modules` ou des sous-projets.
const MAX_DEPTH: usize = 4;

pub fn extract(gzipped_tar: &[u8]) -> std::io::Result<RepoTree> {
    let mut tree = RepoTree::new();
    let mut archive = Archive::new(GzDecoder::new(gzipped_tar));

    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().into_owned();

        // Le tarball d'une forge enveloppe tout dans un repertoire `repo-sha/`.
        let Some(relative) = path.split_once('/').map(|(_, rest)| rest) else {
            continue;
        };
        if relative.is_empty() || is_ignored(relative) {
            continue;
        }

        let size = entry.header().size().unwrap_or(0);
        if !is_interesting(relative) || size > MAX_FILE_BYTES {
            // Un fichier source n'est pas garde, mais on releve au passage les
            // variables qu'il lit : c'est la reponse exacte a « sous quel nom
            // l'application attend-elle son port ? ».
            if size <= MAX_FILE_BYTES && ynp_core::lectures::est_un_source_de_production(relative) {
                let mut buf = String::new();
                if entry.read_to_string(&mut buf).is_ok() {
                    tree.noter_les_lectures(relative, &buf);
                }
            }
            tree.add_path(relative);
            continue;
        }

        let mut buf = String::new();
        match entry.read_to_string(&mut buf) {
            Ok(_) => tree.insert(relative, buf),
            // Fichier binaire malgre son extension : le chemin suffit.
            Err(_) => tree.add_path(relative),
        }
    }
    Ok(tree)
}

/// Repertoires dont le contenu n'apprend rien sur l'application elle-meme.
fn is_ignored(path: &str) -> bool {
    const DIRS: &[&str] = &[
        "node_modules/",
        "vendor/",
        ".git/",
        "target/",
        "dist/",
        ".venv/",
        "__pycache__/",
    ];
    DIRS.iter()
        .any(|d| path.starts_with(d) || path.contains(&format!("/{d}")))
}

/// Fichiers dont les detecteurs lisent le contenu.
fn is_interesting(path: &str) -> bool {
    if path.matches('/').count() > MAX_DEPTH {
        return false;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_lowercase();

    const EXACT: &[&str] = &[
        "package.json",
        "composer.json",
        "go.mod",
        "go.sum",
        "cargo.toml",
        "gemfile",
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "pipfile",
        "makefile",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lockb",
        "composer.lock",
        "poetry.lock",
        "uv.lock",
        "gemfile.lock",
        "cargo.lock",
        "pipfile.lock",
        ".nvmrc",
        ".python-version",
        ".ruby-version",
        ".tool-versions",
        "chart.yaml",
        "procfile",
        "readme.md",
        "license",
        "license.md",
        "license.txt",
        "copying",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "index.html",
    ];

    EXACT.contains(&lower.as_str())
        || est_un_fichier_de_paquet(&path.to_lowercase())
        || ynp_core::noms::est_un_dockerfile(&lower)
        // Le script d'entree d'une image finit presque toujours par `exec`,
        // qui nomme le vrai processus a lancer. Sans son contenu, l'outil ne
        // pouvait que refuser de traduire la commande de demarrage.
        || (lower.ends_with(".sh") && lower.contains("entrypoint"))
        // Une unite systemd livree par le projet dit comment il se lance en
        // demon : c'est exactement ce que le paquet doit ecrire.
        || lower.ends_with(".service")
        || lower.starts_with("docker-compose.")
        || lower.starts_with("compose.")
        || lower.starts_with(".env")
        || lower.starts_with("env.")
        || est_un_fichier_env(&lower)
        // Fichiers de configuration d'exemple, sous leurs formes courantes.
        || (lower.contains("example") || lower.contains("sample") || lower.contains("template"))
            && (lower.ends_with(".yml")
                || lower.ends_with(".yaml")
                || lower.ends_with(".json")
                || lower.ends_with(".toml")
                || lower.ends_with(".ini")
                || lower.ends_with(".conf"))
}

/// Vrai pour un fichier appartenant a un paquet YunoHost.
///
/// Examiner un paquet deja publie demande d'en lire le manifeste et les
/// scripts. Sans cette regle, ils etaient retenus par leur chemin mais vides
/// de contenu, et l'examen concluait a un format inconnu et a des scripts
/// absents — sur un paquet parfaitement correct.
fn est_un_fichier_de_paquet(chemin: &str) -> bool {
    const RACINE: &[&str] = &[
        "manifest.toml",
        "manifest.json",
        "tests.toml",
        "config_panel.toml",
    ];
    const SCRIPTS: &[&str] = &[
        "install",
        "remove",
        "upgrade",
        "backup",
        "restore",
        "change_url",
        "_common.sh",
    ];
    RACINE.contains(&chemin)
        || chemin
            .strip_prefix("scripts/")
            .is_some_and(|n| SCRIPTS.contains(&n))
}

/// Vrai pour un fichier au format `.env` dont le nom est prefixe.
///
/// `starts_with(".env")` ne voit que la forme canonique. Beaucoup de projets
/// prefixent le fichier du nom de l'application — gotify publie
/// `gotify-server.env.example`, qui documente son port et sa base. Ne pas le
/// lire faisait declarer ces deux champs indeterminables.
fn est_un_fichier_env(nom: &str) -> bool {
    const SUITES: &[&str] = &["", ".example", ".sample", ".template", ".dist", ".defaults"];
    SUITES
        .iter()
        .any(|suite| nom.ends_with(&format!(".env{suite}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_fichiers_d_un_paquet_yunohost_sont_lus() {
        // Sans eux, examiner un paquet publie conclut a un format inconnu et
        // a des scripts absents, sur un paquet parfaitement correct.
        for f in [
            "manifest.toml",
            "tests.toml",
            "config_panel.toml",
            "scripts/install",
            "scripts/backup",
            "scripts/_common.sh",
        ] {
            assert!(is_interesting(f), "{f} devrait etre lu");
        }
        // Ce qui n'est pas un fichier de paquet le reste.
        assert!(!is_interesting("scripts/build.sh"));
        assert!(!is_interesting("autre/install"));
    }

    #[test]
    fn un_fichier_env_prefixe_du_nom_de_l_application_est_lu() {
        assert!(is_interesting("gotify-server.env.example"));
        assert!(is_interesting("app.env"));
        assert!(is_interesting("config/monapp.env.sample"));
        // Ce qui contient « env » sans etre un fichier .env reste ecarte.
        assert!(!is_interesting("src/environment.ts"));
        assert!(!is_interesting("scripts/setup-env.sh"));
    }

    #[test]
    fn les_fichiers_de_projet_sont_retenus_avec_leur_contenu() {
        for f in [
            "package.json",
            "Dockerfile",
            "docker-compose.yml",
            ".env.example",
            "go.mod",
        ] {
            assert!(is_interesting(f), "{f} devrait etre lu");
        }
    }

    #[test]
    fn le_code_source_et_les_ressources_ne_sont_pas_charges() {
        for f in [
            "src/main.rs",
            "assets/logo.png",
            "docs/guide.md",
            "test/fixture.bin",
        ] {
            assert!(!is_interesting(f), "{f} ne devrait pas etre lu");
        }
    }

    #[test]
    fn un_fichier_de_configuration_trop_profond_est_ignore() {
        assert!(is_interesting("config/app/.env.example"));
        assert!(!is_interesting("a/b/c/d/e/f/package.json"));
    }

    #[test]
    fn les_repertoires_de_dependances_sont_ecartes() {
        assert!(is_ignored("node_modules/foo/package.json"));
        assert!(is_ignored("web/node_modules/x/package.json"));
        assert!(is_ignored("vendor/autoload.php"));
        assert!(!is_ignored("src/vendors.ts"));
    }

    #[test]
    fn une_archive_est_extraite_sans_son_repertoire_enveloppe() {
        // Le tarball d'une forge enveloppe tout dans `repo-sha/`.
        let mut tar = tar::Builder::new(Vec::new());
        let content = b"{\"name\":\"x\"}";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header.clone(),
            "repo-abc123/package.json",
            &content[..],
        )
        .unwrap();
        tar.append_data(&mut header.clone(), "repo-abc123/src/main.js", &content[..])
            .unwrap();
        let tar_bytes = tar.into_inner().unwrap();

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        let gzipped = gz.finish().unwrap();

        let tree = extract(&gzipped).unwrap();
        assert_eq!(tree.text("package.json"), Some("{\"name\":\"x\"}"));
        // Le chemin du code source est connu, son contenu n'est pas charge.
        assert!(tree.has("src/main.js"));
        assert_eq!(tree.text("src/main.js"), None);
    }
}

#[cfg(test)]
mod licence_conservee {
    use super::*;

    #[test]
    fn les_variantes_de_nom_de_licence_sont_conservees() {
        // Le fichier sert de repli quand la forge rend « NOASSERTION » : s'il
        // n'etait pas conserve, le repli serait sans effet.
        for f in ["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING", "license"] {
            assert!(is_interesting(f), "{f} doit etre conserve");
        }
    }
}
