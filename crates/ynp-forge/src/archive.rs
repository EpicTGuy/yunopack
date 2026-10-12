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
        "license.rst",
        "licence",
        "licence.md",
        "licence.txt",
        "copying",
        "copying.md",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "index.html",
    ];

    EXACT.contains(&lower.as_str())
        // Les workflows disent quelle famille de tags publie quoi : dans un
        // depot qui heberge plusieurs logiciels, c'est la qu'on apprend lequel
        // porte la version du serveur (voir `workflows`).
        || path.starts_with(".github/workflows/")
            && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
        || est_un_fichier_de_paquet(&path.to_lowercase())
        || ynp_core::noms::est_un_dockerfile(&lower)
        // Le script d'entree d'une image finit presque toujours par `exec`,
        // qui nomme le vrai processus a lancer. Il ne s'appelle pas toujours
        // `entrypoint` : `run.sh` chez Mealie, `bootstrap.sh` chez Linkding,
        // `docker_start.sh` chez Loomio. Ces scripts sont courts.
        || lower.ends_with(".sh")
        // Sans extension aussi : `bin/docker-entrypoint`, celui que Rails
        // genere depuis sa version 7.1.
        || lower.contains("entrypoint") && !lower.contains('.')
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
mod zip {
    use super::*;

    /// Un zip minimal : une entree par (nom, mode), sans donnees.
    fn zip(entrees: &[(&str, u32)]) -> Vec<u8> {
        let mut central = Vec::new();
        for (nom, mode) in entrees {
            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes()); // fait sous Unix
            central.extend_from_slice(&[0u8; 22]); // version, drapeaux, methode, dates, crc, tailles
            central.extend_from_slice(&(nom.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0u8; 8]); // extra, commentaire, disque, attributs internes
            central.extend_from_slice(&(mode << 16).to_le_bytes());
            central.extend_from_slice(&[0u8; 4]); // decalage
            central.extend_from_slice(nom.as_bytes());
        }
        let mut z = vec![0u8; 10]; // donnees locales, ignorees
        let debut = z.len();
        z.extend_from_slice(&central);
        z.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        z.extend_from_slice(&(entrees.len() as u16).to_le_bytes());
        z.extend_from_slice(&(entrees.len() as u16).to_le_bytes());
        z.extend_from_slice(&(central.len() as u32).to_le_bytes());
        z.extend_from_slice(&(debut as u32).to_le_bytes());
        z.extend_from_slice(&[0, 0]);
        z
    }

    #[test]
    fn les_executables_d_un_zip_se_lisent_a_sa_fin() {
        let z = zip(&[
            ("dmarc-viewer/", 0o040755),
            ("dmarc-viewer/dmarc-report-viewer", 0o100644),
            ("dmarc-viewer/README.md", 0o100644),
        ]);
        let e = entrees_zip(&z);
        assert_eq!(e.len(), 3);
        assert_eq!(executables_des_entrees(&e), vec!["dmarc-report-viewer"]);
        assert_eq!(racine_des_entrees(&e), Some(true));
    }
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
        assert!(!is_interesting("scripts/build.py"));
        assert!(!is_interesting("autre/install"));
    }

    #[test]
    fn les_scripts_shell_sont_lus_quel_que_soit_leur_nom() {
        // `run.sh` chez Mealie, `bootstrap.sh` chez Linkding : le point
        // d'entree d'une image ne s'appelle pas toujours `entrypoint.sh`.
        for f in ["docker/entry.sh", "bootstrap.sh", "scripts/build.sh"] {
            assert!(is_interesting(f), "{f}");
        }
    }

    #[test]
    fn un_fichier_env_prefixe_du_nom_de_l_application_est_lu() {
        assert!(is_interesting("gotify-server.env.example"));
        assert!(is_interesting("app.env"));
        assert!(is_interesting("config/monapp.env.sample"));
        // Ce qui contient « env » sans etre un fichier .env reste ecarte.
        assert!(!is_interesting("src/environment.ts"));
        assert!(!is_interesting("scripts/setup-env.py"));
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

/// Dit, d'apres le debut d'une archive `.tar.gz`, si tout y est range dans un
/// repertoire racine.
///
/// La reponse decide de `in_subdir` dans le manifeste, donc de l'endroit ou
/// atterrit le binaire. FlareSolverr publie `flaresolverr/flaresolverr` : le
/// declarer plat faisait lancer au service un repertoire. Le debut de
/// l'archive suffit — les premiers en-tetes tar disent la forme — et permet
/// de ne pas rapatrier des centaines de megaoctets pour cela.
///
/// Rend `None` quand le debut ne permet pas de conclure.
/// Les fichiers executables d'une archive tar.gz, meme lue en partie, avec
/// le repertoire racine retire s'il enveloppe tout.
///
/// Une release Go ou Rust publie souvent une archive (`farside_linux_amd64
/// .tar.gz`) dont le binaire porte le nom du projet ; sans le lire, l'outil
/// ne savait pas quoi lancer.
pub fn executables(debut: &[u8]) -> Vec<String> {
    let mut noms: Vec<String> = Vec::new();
    let mut tous: Vec<String> = Vec::new();
    let mut archive = Archive::new(GzDecoder::new(debut));
    if let Ok(entrees) = archive.entries() {
        for entree in entrees {
            let Ok(entree) = entree else { break };
            let Ok(chemin) = entree.path() else { break };
            let nom = chemin
                .to_string_lossy()
                .trim_start_matches("./")
                .to_string();
            let fichier = entree.header().entry_type().is_file();
            let mode = entree.header().mode().unwrap_or(0);
            if !nom.is_empty() {
                tous.push(nom.clone());
            }
            if fichier && mode & 0o111 != 0 {
                noms.push(nom);
            }
            if tous.len() >= 50 {
                break;
            }
        }
    }
    let racine = |n: &String| n.split('/').next().unwrap_or_default().to_string();
    let enveloppe = !tous.is_empty()
        && tous.iter().all(|n| n.contains('/') || n.ends_with('/'))
        && tous.iter().all(|n| racine(n) == racine(&tous[0]));
    noms.into_iter()
        .map(|n| match (enveloppe, n.split_once('/')) {
            (true, Some((_, reste))) => reste.to_string(),
            _ => n,
        })
        .collect()
}

/// Les entrees d'une archive zip — nom, fichier ou non, mode Unix — d'apres
/// son repertoire central, qui en occupe la fin : les derniers kilooctets
/// suffisent, sans rapatrier l'archive. dmarc-report-viewer ne publie que
/// des zip, et l'outil supposait un binaire a la racine.
pub fn entrees_zip(fin: &[u8]) -> Vec<(String, bool, u32)> {
    let u16le = |b: &[u8], i: usize| -> Option<usize> {
        Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize)
    };
    let u32le = |b: &[u8], i: usize| -> Option<u32> {
        Some(u32::from_le_bytes([
            *b.get(i)?,
            *b.get(i + 1)?,
            *b.get(i + 2)?,
            *b.get(i + 3)?,
        ]))
    };
    // La fin du repertoire central : signature PK\x05\x06, la derniere.
    let Some(eocd) = fin.windows(4).rposition(|w| w == [0x50, 0x4b, 0x05, 0x06]) else {
        return Vec::new();
    };
    let (Some(taille), Some(n)) = (u32le(fin, eocd + 12), u16le(fin, eocd + 10)) else {
        return Vec::new();
    };
    // Le repertoire central precede immediatement cette fin.
    let Some(mut i) = eocd.checked_sub(taille as usize) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for _ in 0..n.min(500) {
        if fin.get(i..i + 4) != Some(&[0x50, 0x4b, 0x01, 0x02][..]) {
            break;
        }
        let (Some(fait_par), Some(lg_nom), Some(lg_extra), Some(lg_comm), Some(externes)) = (
            u16le(fin, i + 4),
            u16le(fin, i + 28),
            u16le(fin, i + 30),
            u16le(fin, i + 32),
            u32le(fin, i + 38),
        ) else {
            break;
        };
        let Some(nom) = fin.get(i + 46..i + 46 + lg_nom) else {
            break;
        };
        let nom = String::from_utf8_lossy(nom)
            .trim_start_matches("./")
            .to_string();
        // Le mode Unix n'existe que pour une archive faite sous Unix.
        let mode = if fait_par >> 8 == 3 {
            externes >> 16
        } else {
            0
        };
        let fichier = !nom.ends_with('/');
        out.push((nom, fichier, mode));
        i += 46 + lg_nom + lg_extra + lg_comm;
    }
    out
}

/// Les executables d'une liste d'entrees, le repertoire racine retire s'il
/// enveloppe tout. Sans mode Unix (zip fait ailleurs), un fichier sans
/// extension a la racine tient lieu d'executable.
pub fn executables_des_entrees(entrees: &[(String, bool, u32)]) -> Vec<String> {
    let tous: Vec<&String> = entrees.iter().map(|(n, _, _)| n).collect();
    let racine = |n: &str| n.split('/').next().unwrap_or_default().to_string();
    let enveloppe = !tous.is_empty()
        && tous.iter().all(|n| n.contains('/'))
        && tous.iter().all(|n| racine(n) == racine(tous[0]));
    let sans_racine = |n: &str| match (enveloppe, n.split_once('/')) {
        (true, Some((_, reste))) => reste.to_string(),
        _ => n.to_string(),
    };
    // Des modes Unix, et au moins un executable : ils font foi. Sinon —
    // dmarc-report-viewer publie son binaire en -rw-r--r-- — un fichier sans
    // extension tient lieu d'executable (voir `chmod` a l'installation).
    let avec_mode = entrees.iter().any(|(_, f, m)| *f && *m & 0o111 != 0);
    entrees
        .iter()
        .filter(|(n, fichier, mode)| {
            *fichier
                && if avec_mode {
                    mode & 0o111 != 0
                } else {
                    let court = sans_racine(n);
                    !court.contains('/') && !court.contains('.')
                }
        })
        .map(|(n, _, _)| sans_racine(n))
        .filter(|n| !n.is_empty())
        .collect()
}

/// Vrai si toutes les entrees sont sous un meme repertoire racine.
pub fn racine_des_entrees(entrees: &[(String, bool, u32)]) -> Option<bool> {
    let tous: Vec<&String> = entrees.iter().map(|(n, _, _)| n).collect();
    if tous.is_empty() {
        return None;
    }
    let racine = |n: &str| n.split('/').next().unwrap_or_default().to_string();
    Some(tous.iter().all(|n| n.contains('/')) && tous.iter().all(|n| racine(n) == racine(tous[0])))
}

pub fn repertoire_racine(debut: &[u8]) -> Option<bool> {
    let mut noms: Vec<String> = Vec::new();
    let mut archive = Archive::new(GzDecoder::new(debut));
    // Un debut tronque finit en erreur : on garde ce qui a ete lu avant.
    if let Ok(entrees) = archive.entries() {
        for entree in entrees {
            let Ok(entree) = entree else { break };
            let Ok(chemin) = entree.path() else { break };
            let nom = chemin
                .to_string_lossy()
                .trim_start_matches("./")
                .to_string();
            if !nom.is_empty() {
                noms.push(nom);
            }
            if noms.len() >= 20 {
                break;
            }
        }
    }
    if noms.is_empty() {
        return None;
    }
    // Un fichier a la racine : l'archive est plate.
    if noms
        .iter()
        .any(|n| !n.trim_end_matches('/').contains('/') && !n.ends_with('/'))
    {
        return Some(false);
    }
    let premier = |n: &String| n.split('/').next().unwrap_or_default().to_string();
    let racine = premier(&noms[0]);
    Some(noms.iter().all(|n| premier(n) == racine))
}

#[cfg(test)]
mod repertoire_racine_tests {
    use super::*;

    fn tar_gz(chemins: &[&str]) -> Vec<u8> {
        let mut t = tar::Builder::new(Vec::new());
        for c in chemins {
            let mut h = tar::Header::new_gnu();
            let donnees = b"x";
            h.set_size(if c.ends_with('/') { 0 } else { 1 });
            h.set_entry_type(if c.ends_with('/') {
                tar::EntryType::Directory
            } else {
                tar::EntryType::Regular
            });
            h.set_mode(0o755);
            h.set_cksum();
            let contenu: &[u8] = if c.ends_with('/') { &[] } else { donnees };
            t.append_data(&mut h, c, contenu).unwrap();
        }
        let brut = t.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &brut).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn un_repertoire_racine_est_reconnu() {
        // Cas de FlareSolverr.
        let a = tar_gz(&[
            "flaresolverr/",
            "flaresolverr/flaresolverr",
            "flaresolverr/_internal/x",
        ]);
        assert_eq!(repertoire_racine(&a), Some(true));
    }

    #[test]
    fn une_archive_plate_est_reconnue() {
        let a = tar_gz(&["miniflux", "LICENSE"]);
        assert_eq!(repertoire_racine(&a), Some(false));
    }

    #[test]
    fn un_debut_tronque_suffit() {
        // Comme en vrai : un gros binaire, dont on ne lit que 64 Ko.
        let mut t = tar::Builder::new(Vec::new());
        let mut graine: u64 = 42;
        let lourd: Vec<u8> = (0..2_000_000)
            .map(|_| {
                graine = graine.wrapping_mul(6364136223846793005).wrapping_add(1);
                (graine >> 33) as u8
            })
            .collect();
        for (chemin, contenu) in [("app-1.0/bin", &lourd[..]), ("app-1.0/lib", &b"x"[..])] {
            let mut h = tar::Header::new_gnu();
            h.set_size(contenu.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            t.append_data(&mut h, chemin, contenu).unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &t.into_inner().unwrap()).unwrap();
        let a = gz.finish().unwrap();
        assert!(a.len() > 1_000_000);
        assert_eq!(repertoire_racine(&a[..65_536]), Some(true));
    }

    #[test]
    fn des_octets_quelconques_ne_concluent_rien() {
        assert_eq!(repertoire_racine(b"pas une archive"), None);
    }
}
