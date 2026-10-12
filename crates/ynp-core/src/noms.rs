//! Reconnaissance de fichiers par leur nom.
//!
//! Ces predicats vivent ici parce que plusieurs etages s'en servent et
//! doivent s'accorder. Quand le filtre qui decide ce qu'on retient d'une
//! archive et le detecteur qui cherche un Dockerfile portaient chacun leur
//! version, ils ont fini par diverger : `docker/default.Dockerfile` etait
//! listé dans l'arbre mais son contenu jamais conservé, et l'analyse
//! concluait « aucun Dockerfile » sur un depot qui en avait deux.

/// Vrai pour un nom de fichier qui designe un Dockerfile.
///
/// Le seul prefixe « dockerfile » ne suffit pas : ce projet contient des
/// fichiers source nommes `dockerfile.rs`, que l'outil a pris pour des
/// Dockerfiles en s'analysant lui-meme. Il en a tire les dependances de ses
/// propres fixtures de test, et conclu qu'il etait ecrit en Python.
pub fn est_un_dockerfile(nom: &str) -> bool {
    // Extensions de code, de documentation ou de donnees : jamais celles d'un
    // Dockerfile, dont le suffixe decrit une variante (`.prod`, `.alpine`).
    const EXTENSIONS_EXCLUES: &[&str] = &[
        "rs", "py", "go", "js", "mjs", "cjs", "ts", "tsx", "jsx", "java", "kt", "rb", "php", "c",
        "h", "cc", "cpp", "hpp", "cs", "swift", "sh", "bash", "ps1", "md", "txt", "rst", "adoc",
        "html", "css", "yml", "yaml", "json", "toml", "ini", "cfg", "lock", "xml", "j2", "tera",
        "tmpl", "template", "bak", "orig", "patch", "diff",
    ];

    let minuscule = nom.to_lowercase();
    if let Some((_, extension)) = minuscule.rsplit_once('.') {
        if EXTENSIONS_EXCLUES.contains(&extension) {
            return false;
        }
    }
    minuscule.starts_with("dockerfile")
        || minuscule.ends_with(".dockerfile")
        || minuscule == "containerfile"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_variantes_nommees_par_prefixe_sont_des_dockerfiles() {
        // Cas de linkding : `docker/default.Dockerfile`. Le filtre d'archive
        // ne gardait que ce qui commencait par « dockerfile », si bien que le
        // fichier apparaissait dans l'arbre, vide.
        for n in [
            "Dockerfile",
            "dockerfile",
            "Dockerfile.prod",
            "default.Dockerfile",
            "alpine.Dockerfile",
            "Containerfile",
        ] {
            assert!(est_un_dockerfile(n), "{n}");
        }
    }

    #[test]
    fn un_fichier_source_homonyme_n_en_est_pas_un() {
        for n in ["dockerfile.rs", "dockerfile.md", "docker-compose.yml"] {
            assert!(!est_un_dockerfile(n), "{n}");
        }
    }
}

/// Le paquet et le script d'une commande d'espace de travail :
/// `yarn workspace @chibisafe/backend start`, `pnpm --filter api start`,
/// `npm run start -w api`.
pub fn commande_d_espace_de_travail(commande: &str) -> Option<(String, String)> {
    let mots: Vec<&str> = commande.split_whitespace().collect();
    match mots.as_slice() {
        ["yarn", "workspace", paquet, "run", script, ..]
        | ["yarn", "workspace", paquet, script, ..] => {
            Some(((*paquet).to_string(), (*script).to_string()))
        }
        ["pnpm", option, paquet, reste @ ..] if matches!(*option, "--filter" | "-F") => {
            let script = match reste {
                ["run", s, ..] | [s, ..] => *s,
                [] => return None,
            };
            Some(((*paquet).to_string(), script.to_string()))
        }
        ["pnpm", option, reste @ ..] if option.starts_with("--filter=") => {
            let script = match reste {
                ["run", s, ..] | [s, ..] => *s,
                [] => return None,
            };
            Some((option["--filter=".len()..].to_string(), script.to_string()))
        }
        ["npm", "run" | "run-script", script, reste @ ..] => {
            let paquet = reste
                .windows(2)
                .find_map(|w| matches!(w[0], "-w" | "--workspace").then(|| w[1].to_string()))
                .or_else(|| {
                    reste
                        .iter()
                        .find_map(|m| m.strip_prefix("--workspace=").map(str::to_string))
                })?;
            Some((paquet, (*script).to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests_espace_de_travail {
    use super::*;

    #[test]
    fn les_commandes_d_espace_de_travail_se_lisent() {
        let c = |s: &str| commande_d_espace_de_travail(s);
        assert_eq!(
            c("yarn workspace @chibisafe/backend start"),
            Some(("@chibisafe/backend".into(), "start".into()))
        );
        assert_eq!(
            c("pnpm --filter api run start"),
            Some(("api".into(), "start".into()))
        );
        assert_eq!(
            c("pnpm --filter=api start"),
            Some(("api".into(), "start".into()))
        );
        assert_eq!(
            c("npm run start -w api"),
            Some(("api".into(), "start".into()))
        );
        assert_eq!(c("yarn workspaces focus @x/y"), None);
        assert_eq!(c("npm run start"), None);
    }
}
