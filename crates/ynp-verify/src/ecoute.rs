//! Adresse d'ecoute des services du paquet.
//!
//! Une application YunoHost ecoute sur la boucle locale ; nginx est devant,
//! avec l'authentification unique. Une application qui ecoute sur toutes les
//! interfaces est joignable sans passer par eux : son port est ouvert au
//! reseau, sans chiffrement, sans controle d'acces.
//!
//! Ce controle est ne d'un defaut reel : InvenTree sortait avec
//! `-b 0.0.0.0:__PORT__`, parce que l'adresse etait ramenee sur la boucle
//! locale avant que son port soit connu. Aucune relecture de specification ne
//! l'aurait vu ; le fichier ecrit, lui, le montre.

use std::path::Path;
use ynp_core::finding::{Evidence, Finding, Severity};

/// Formes sous lesquelles un service annonce qu'il ecoute partout.
const TOUTES_LES_INTERFACES: &[&str] = &["0.0.0.0", "[::]", ":::"];

pub fn controler(racine: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    let Ok(entrees) = std::fs::read_dir(racine.join("conf")) else {
        return out;
    };
    for entree in entrees.flatten() {
        let chemin = entree.path();
        let Ok(texte) = std::fs::read_to_string(&chemin) else {
            continue; // binaire : rien a lire
        };
        let nom = format!("conf/{}", entree.file_name().to_string_lossy());
        // La configuration nginx ecoute legitimement partout : c'est elle
        // qui fait face au reseau.
        if nom.starts_with("conf/nginx") {
            continue;
        }
        for (i, ligne) in texte.lines().enumerate() {
            let l = ligne.trim();
            if l.starts_with('#') {
                continue;
            }
            if TOUTES_LES_INTERFACES.iter().any(|a| l.contains(a)) {
                out.push(
                    Finding::new(
                        "ECOUTE001",
                        Severity::Blocker,
                        "Le service ecouterait sur toutes les interfaces",
                    )
                    .detail(format!(
                        "{nom}, ligne {} : « {} ». L'application serait joignable \
                         directement, sans nginx ni l'authentification de YunoHost.",
                        i + 1,
                        l
                    ))
                    .remediation(
                        "Ecouter sur 127.0.0.1 : nginx relaie les requetes jusqu'a \
                         l'application.",
                    )
                    .evidence(Evidence::file(nom.clone())),
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paquet(fichiers: &[(&str, &str)]) -> tempdir::Dir {
        tempdir::Dir::avec(fichiers)
    }

    #[test]
    fn un_service_ouvert_a_tout_le_reseau_bloque() {
        let p = paquet(&[(
            "conf/systemd.service",
            "[Service]\nExecStart=__INSTALL_DIR__/venv/bin/gunicorn -b 0.0.0.0:__PORT__ app\n",
        )]);
        let c = controler(p.chemin());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].severity, Severity::Blocker);
    }

    #[test]
    fn la_boucle_locale_et_nginx_passent() {
        let p = paquet(&[
            (
                "conf/systemd.service",
                "[Service]\nExecStart=x -b 127.0.0.1:__PORT__\n",
            ),
            ("conf/nginx.conf", "listen 0.0.0.0:80;\n"),
            (
                "conf/.env",
                "# HOST=0.0.0.0 pour tout ouvrir\nHOST=127.0.0.1\n",
            ),
        ]);
        assert!(controler(p.chemin()).is_empty());
    }

    /// Un repertoire temporaire, efface a la fin du test.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicUsize, Ordering};

        static N: AtomicUsize = AtomicUsize::new(0);

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn avec(fichiers: &[(&str, &str)]) -> Self {
                let d = std::env::temp_dir().join(format!(
                    "ynp-ecoute-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::SeqCst)
                ));
                for (chemin, contenu) in fichiers {
                    let f = d.join(chemin);
                    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                    std::fs::write(f, contenu).unwrap();
                }
                Dir(d)
            }
            pub fn chemin(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
