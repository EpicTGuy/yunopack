//! `AppSpec` -> arborescence du paquet.
//!
//! Aucune decision ne se prend ici : le rendu est mecanique. Tout ce qui
//! demandait un arbitrage l'a recu dans `appspec.toml`, et un correctif sur un
//! paquet genere se fait dans le template ou dans la specification, jamais dans
//! le fichier de sortie.
//!
//! Les templates sont incorpores au binaire : `yunopack` doit pouvoir generer un
//! paquet sur une machine qui n'a recu que l'executable.

pub mod context;

use std::path::{Path, PathBuf};
use tera::Tera;
use ynp_core::AppSpec;

#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("template {nom} : {source}")]
    Template {
        nom: String,
        source: Box<tera::Error>,
    },
    #[error("ecriture de {chemin} : {source}")]
    Ecriture {
        chemin: String,
        source: std::io::Error,
    },
}

/// Un fichier du paquet.
struct Fichier {
    /// Chemin relatif dans le paquet.
    destination: &'static str,
    template: &'static str,
    contenu: &'static str,
    /// Les scripts doivent etre executables.
    executable: bool,
    /// Rendu seulement si la specification le demande.
    requis: fn(&AppSpec) -> bool,
}

const TOUJOURS: fn(&AppSpec) -> bool = |_| true;

fn fichiers() -> Vec<Fichier> {
    macro_rules! f {
        ($dest:expr, $tpl:expr, $exe:expr, $requis:expr) => {
            Fichier {
                destination: $dest,
                template: $tpl,
                contenu: include_str!(concat!("../../../assets/templates/", $tpl)),
                executable: $exe,
                requis: $requis,
            }
        };
    }

    vec![
        f!("manifest.toml", "manifest.toml.tera", false, TOUJOURS),
        f!("tests.toml", "tests.toml.tera", false, TOUJOURS),
        f!("README.md", "README.md.tera", false, TOUJOURS),
        f!("LICENSE", "LICENSE.tera", false, TOUJOURS),
        f!(
            "doc/DESCRIPTION.md",
            "doc/DESCRIPTION.md.tera",
            false,
            TOUJOURS
        ),
        // Les consignes d'exploitation, quand il y en a. YunoHost les affiche
        // dans l'administration, la ou l'administrateur les cherchera.
        f!("doc/ADMIN.md", "doc/ADMIN.md.tera", false, |s| s
            .docs
            .admin
            .is_some()),
        f!(
            "scripts/_common.sh",
            "scripts/_common.sh.tera",
            false,
            TOUJOURS
        ),
        f!("scripts/install", "scripts/install.tera", true, TOUJOURS),
        f!("scripts/remove", "scripts/remove.tera", true, TOUJOURS),
        f!("scripts/upgrade", "scripts/upgrade.tera", true, TOUJOURS),
        f!("scripts/backup", "scripts/backup.tera", true, TOUJOURS),
        f!("scripts/restore", "scripts/restore.tera", true, TOUJOURS),
        f!("scripts/change_url", "scripts/change_url.tera", true, |s| s
            .features
            .change_url),
        f!("conf/nginx.conf", "conf/nginx.conf.tera", false, |s| s
            .features
            .nginx),
        f!(
            "conf/systemd.service",
            "conf/systemd.service.tera",
            false,
            |s| s.features.systemd
        ),
        // Le stockage S3 embarque, quand l'application en exige un.
        f!(
            "conf/seaweedfs.service",
            "conf/seaweedfs.service.tera",
            false,
            |s| s.resources.s3
        ),
        f!(
            "conf/seaweedfs.json",
            "conf/seaweedfs.json.tera",
            false,
            |s| s.resources.s3
        ),
        f!(
            "conf/seaweedfs-security.toml",
            "conf/seaweedfs-security.toml.tera",
            false,
            |s| s.resources.s3
        ),
        Fichier {
            destination: "conf/__CONFIG__",
            template: "conf/app-config.tera",
            contenu: include_str!("../../../assets/templates/conf/app-config.tera"),
            executable: false,
            requis: AppSpec::a_une_configuration,
        },
    ]
}

/// Ce que la generation a produit.
pub struct Genere {
    pub racine: PathBuf,
    pub fichiers: Vec<String>,
    /// Champs restes a completer, presents dans les fichiers sous forme de
    /// marqueurs `FIXME(yunopack)`. `verify` refusera le paquet tant qu'il en
    /// subsiste un.
    pub a_completer: Vec<String>,
}

/// Rend l'arborescence du paquet dans `<parent>/<app_id>_ynh`.
pub fn generate(spec: &AppSpec, parent: &Path) -> Result<Genere, GenError> {
    let racine = parent.join(format!("{}_ynh", spec.app.id));
    let (valeurs, a_completer) = context::build(spec);
    let contexte = tera::Context::from_value(valeurs).map_err(|e| GenError::Template {
        nom: "contexte".into(),
        source: Box::new(e),
    })?;

    let mut ecrits = Vec::new();
    for fichier in fichiers() {
        if !(fichier.requis)(spec) {
            continue;
        }

        let rendu =
            Tera::one_off(fichier.contenu, &contexte, false).map_err(|e| GenError::Template {
                nom: fichier.template.into(),
                source: Box::new(e),
            })?;

        // Le fichier de configuration porte le nom choisi dans la spec.
        let destination = match fichier.destination {
            "conf/__CONFIG__" => {
                format!(
                    "conf/{}",
                    spec.runtime.config_file.clone().unwrap_or_default()
                )
            }
            d => d.to_string(),
        };

        ecrire(
            &racine.join(&destination),
            &nettoyer(&rendu),
            fichier.executable,
        )?;
        ecrits.push(destination);
    }

    ecrits.sort();
    Ok(Genere {
        racine,
        fichiers: ecrits,
        a_completer,
    })
}

/// Supprime les lignes vides en trop laissees par les blocs conditionnels.
///
/// Un template a conditions produit facilement des paquets d'espaces ; le
/// linter officiel ne s'en plaint pas, mais un fichier relu par un humain si.
fn nettoyer(rendu: &str) -> String {
    let mut sortie = String::with_capacity(rendu.len());
    let mut vides = 0;

    for ligne in rendu.lines() {
        if ligne.trim().is_empty() {
            vides += 1;
            if vides > 1 {
                continue;
            }
        } else {
            vides = 0;
        }
        sortie.push_str(ligne.trim_end());
        sortie.push('\n');
    }
    // Exactement une ligne vide finale.
    format!("{}\n", sortie.trim_end())
}

fn ecrire(chemin: &Path, contenu: &str, executable: bool) -> Result<(), GenError> {
    let echec = |source: std::io::Error| GenError::Ecriture {
        chemin: chemin.display().to_string(),
        source,
    };

    if let Some(parent) = chemin.parent() {
        std::fs::create_dir_all(parent).map_err(echec)?;
    }
    std::fs::write(chemin, contenu).map_err(echec)?;

    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(chemin, std::fs::Permissions::from_mode(0o755)).map_err(echec)?;
    }
    Ok(())
}
