//! Ecriture des correctifs qu'un examen a juges mecaniques.
//!
//! Un constat marque `reparable` l'est parce que sa correction ne demande
//! aucun arbitrage : renommer un helper, ajouter un fichier de tests, declarer
//! la mise a jour automatique. Les autres — choisir une licence, passer un
//! paquet au format 2 — demandent un jugement, et l'outil ne s'y substitue
//! pas.
//!
//! Le correctif est ecrit dans l'arborescence du paquet, telle qu'elle a ete
//! recuperee. C'est ensuite a `ynp-publish` de la pousser et d'ouvrir la
//! demande d'integration.

use crate::audit::{Audit, Constat};
use std::path::Path;

/// Ce qu'un correctif a change.
#[derive(Debug, Clone, Default)]
pub struct Correctif {
    /// Fichiers modifies ou crees, chemins relatifs au paquet.
    pub fichiers: Vec<String>,
    /// Constats effectivement corriges.
    pub corriges: Vec<String>,
    /// Ce qui restait a corriger mais demandait un jugement.
    pub laisses: Vec<String>,
}

impl Correctif {
    pub fn rien(&self) -> bool {
        self.fichiers.is_empty()
    }

    /// Le message de commit, qui dit ce qui a change et pourquoi.
    pub fn message(&self, app: &str) -> String {
        let mut m = format!("Amelioration du paquet {app}\n\n");
        for id in &self.corriges {
            m.push_str(&format!("- {}\n", explication(id)));
        }
        m.push_str(
            "\nCorrectif ecrit par yunopack a partir d'un examen du paquet. \
             Chaque point ci-dessus est mecanique : aucun arbitrage n'a ete \
             pris a la place du mainteneur.\n",
        );
        m
    }
}

fn explication(id: &str) -> &'static str {
    match id {
        "HLP001" => "helpers renommes selon la version 2.1",
        "FIC004" => "ajout d'un fichier de tests",
        "FIC005" => "ajout d'une description longue",
        "MAJ001" => "declaration de la mise a jour automatique",
        "DOC001" => "ajout d'un README",
        _ => "correction",
    }
}

/// Applique ce qui peut l'etre. Ne touche a rien d'autre.
pub fn appliquer(racine: &Path, audit: &Audit) -> std::io::Result<Correctif> {
    let mut c = Correctif::default();
    for constat in &audit.constats {
        if !constat.reparable {
            c.laisses.push(constat.id.clone());
            continue;
        }
        let touches = match constat.id.as_str() {
            "HLP001" => renommer_les_helpers(racine)?,
            "MAJ001" => declarer_la_mise_a_jour(racine)?,
            "FIC004" => ecrire_les_tests(racine, &audit.app)?,
            "FIC005" => ecrire_la_description(racine, &audit.app)?,
            _ => Vec::new(),
        };
        if touches.is_empty() {
            c.laisses.push(constat.id.clone());
        } else {
            c.corriges.push(constat.id.clone());
            for f in touches {
                if !c.fichiers.contains(&f) {
                    c.fichiers.push(f);
                }
            }
        }
    }
    c.fichiers.sort();
    Ok(c)
}

/// La table des renommages de la version 2.1 des helpers.
///
/// Chaque paire est un renommage pur : meme comportement, nom different. Un
/// helper dont la signature a change n'y figure pas — le renommer casserait
/// le script.
const RENOMMES: &[(&str, &str)] = &[
    ("ynh_add_nginx_config", "ynh_config_add_nginx"),
    ("ynh_add_systemd_config", "ynh_config_add_systemd"),
    ("ynh_add_fpm_config", "ynh_config_add_phpfpm"),
    ("ynh_systemd_action", "ynh_systemctl"),
    ("ynh_replace_string", "ynh_replace"),
    ("ynh_secure_remove", "ynh_safe_rm"),
    ("ynh_exec_warn_less", "ynh_hide_warnings"),
    ("ynh_restore_file", "ynh_restore"),
];

fn renommer_les_helpers(racine: &Path) -> std::io::Result<Vec<String>> {
    let mut touches = Vec::new();
    let Ok(entrees) = std::fs::read_dir(racine.join("scripts")) else {
        return Ok(touches);
    };
    let mut chemins: Vec<_> = entrees.filter_map(Result::ok).map(|e| e.path()).collect();
    chemins.sort();

    for chemin in chemins {
        let Ok(avant) = std::fs::read_to_string(&chemin) else {
            continue;
        };
        let mut apres = avant.clone();
        for (ancien, neuf) in RENOMMES {
            // Le nom doit etre entier : `ynh_replace_string` contient
            // `ynh_replace`, et remplacer a l'aveugle ferait des chimeres.
            apres = remplacer_mot(&apres, ancien, neuf);
        }
        if apres != avant {
            std::fs::write(&chemin, apres)?;
            if let Some(n) = chemin.file_name() {
                touches.push(format!("scripts/{}", n.to_string_lossy()));
            }
        }
    }
    Ok(touches)
}

/// Remplace un identifiant entier, jamais un fragment.
///
/// `ynh_replace_string` contient `ynh_replace` : un remplacement naif le
/// transformerait en `ynh_replace_string`, ou pire.
pub fn remplacer_mot(texte: &str, ancien: &str, neuf: &str) -> String {
    let limite = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::with_capacity(texte.len());
    let mut reste = texte;
    while let Some(i) = reste.find(ancien) {
        let avant = reste[..i].chars().next_back();
        let apres = reste[i + ancien.len()..].chars().next();
        out.push_str(&reste[..i]);
        if limite(avant) && limite(apres) {
            out.push_str(neuf);
        } else {
            out.push_str(ancien);
        }
        reste = &reste[i + ancien.len()..];
    }
    out.push_str(reste);
    out
}

/// Declare la mise a jour automatique, quand la forge est reconnaissable.
fn declarer_la_mise_a_jour(racine: &Path) -> std::io::Result<Vec<String>> {
    let chemin = racine.join("manifest.toml");
    let Ok(avant) = std::fs::read_to_string(&chemin) else {
        return Ok(Vec::new());
    };
    if avant.contains("autoupdate") {
        return Ok(Vec::new());
    }
    // Une source qui n'est pas une archive de forge — une image de conteneur,
    // par exemple — ne se met pas a jour par ce mecanisme. Y declarer une
    // strategie produirait un manifest qui ment.
    if avant.contains("format = \"docker\"") {
        return Ok(Vec::new());
    }
    // La strategie depend de la forge, lisible dans l'URL des sources.
    let strategie = if avant.contains("github.com") {
        "latest_github_release"
    } else if avant.contains("codeberg.org") || avant.contains("forgejo") {
        "latest_forgejo_release"
    } else if avant.contains("gitlab") {
        "latest_gitlab_release"
    } else {
        // Forge inconnue : deviner la strategie produirait un manifest qui ne
        // se met jamais a jour, sans que personne s'en apercoive.
        return Ok(Vec::new());
    };

    // La declaration se place dans la section des sources, pas ailleurs. Elle
    // s'ecrit aussi bien `[resources.sources]` que `[resources.sources.main]`,
    // la seconde forme etant la plus repandue.
    let Some(i) = avant
        .find("[resources.sources.main]")
        .or_else(|| avant.find("[resources.sources]"))
    else {
        return Ok(Vec::new());
    };
    // La section s'arrete au prochain en-tete, quelle que soit son indentation.
    let fin_section = avant[i..]
        .match_indices('\n')
        .find(|(j, _)| {
            avant[i + j + 1..]
                .split('\n')
                .next()
                .unwrap_or_default()
                .trim_start()
                .starts_with('[')
        })
        .map_or(avant.len(), |(j, _)| i + j);
    let mut apres = String::with_capacity(avant.len() + 80);
    apres.push_str(&avant[..fin_section]);
    apres.push_str(&format!(
        "\n\n# Ajoute par yunopack : sans cela, les montees de version\n\
         # demandent une intervention manuelle a chaque fois.\nautoupdate.strategy = \"{strategie}\"\n"
    ));
    apres.push_str(&avant[fin_section..]);
    std::fs::write(&chemin, apres)?;
    Ok(vec!["manifest.toml".to_string()])
}

fn ecrire_les_tests(racine: &Path, app: &str) -> std::io::Result<Vec<String>> {
    let chemin = racine.join("tests.toml");
    if chemin.exists() {
        return Ok(Vec::new());
    }
    std::fs::write(
        &chemin,
        format!(
            "#:schema https://raw.githubusercontent.com/YunoHost/apps/main/schemas/tests.v1.schema.json\n\
             # Ajoute par yunopack. Le scenario par defaut suffit a la plupart\n\
             # des paquets ; ajouter un `test_upgrade_from` si {app} a connu\n\
             # une montee de version delicate.\n\n\
             test_format = 1.0\n\n[default]\n"
        ),
    )?;
    Ok(vec!["tests.toml".to_string()])
}

fn ecrire_la_description(racine: &Path, app: &str) -> std::io::Result<Vec<String>> {
    let chemin = racine.join("doc/DESCRIPTION.md");
    if chemin.exists() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(racine.join("doc"))?;
    // Le manifeste porte deja une description courte ; la reprendre vaut mieux
    // que d'inventer, et le mainteneur completera.
    let courte = std::fs::read_to_string(racine.join("manifest.toml"))
        .ok()
        .and_then(|m| toml::from_str::<toml::Value>(&m).ok())
        .and_then(|v| {
            v.get("description")
                .and_then(|d| d.get("en"))
                .and_then(|d| d.as_str())
                .map(std::string::ToString::to_string)
        })
        .unwrap_or_else(|| format!("{app} for YunoHost."));
    std::fs::write(&chemin, format!("{courte}\n"))?;
    Ok(vec!["doc/DESCRIPTION.md".to_string()])
}

/// Une empreinte courte du correctif, pour nommer la branche.
///
/// Deux correctifs identiques donnent la meme branche : relancer ne cree pas
/// une seconde demande d'integration pour le meme travail.
pub fn empreinte(c: &Correctif) -> String {
    let mut n: u64 = 1469598103934665603;
    for f in c.corriges.iter().chain(c.fichiers.iter()) {
        for o in f.bytes() {
            n ^= u64::from(o);
            n = n.wrapping_mul(1099511628211);
        }
    }
    format!("{:x}", n & 0xffff_ffff)
}

/// Vrai si le constat est de ceux que ce module sait traiter.
pub fn sait_corriger(c: &Constat) -> bool {
    c.reparable && matches!(c.id.as_str(), "HLP001" | "MAJ001" | "FIC004" | "FIC005")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::examiner;

    struct Paquet(std::path::PathBuf);

    impl Paquet {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "ynp-fix-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(p.join("scripts")).unwrap();
            std::fs::create_dir_all(p.join("doc")).unwrap();
            Self(p)
        }
        fn avec(self, chemin: &str, contenu: &str) -> Self {
            let f = self.0.join(chemin);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, contenu).unwrap();
            self
        }
        fn lire(&self, chemin: &str) -> String {
            std::fs::read_to_string(self.0.join(chemin)).unwrap_or_default()
        }
        fn complet(self) -> Self {
            self.avec(
                "manifest.toml",
                "packaging_format = 2\nid = \"demo\"\ndescription.en = \"Une demo\"\n\n[resources.sources]\nurl = \"https://github.com/a/b/archive/v1.tar.gz\"\n\n[resources.system_user]\n",
            )
            .avec("scripts/install", "x")
            .avec("scripts/backup", "x")
            .avec("scripts/restore", "x")
            .avec("scripts/upgrade", "x")
            .avec("tests.toml", "x")
            .avec("doc/DESCRIPTION.md", "x")
            .avec("README.md", "x")
            .avec("LICENSE", "x")
        }
    }

    impl Drop for Paquet {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn un_nom_de_helper_n_est_remplace_qu_entier() {
        // `ynh_replace_string` contient `ynh_replace` : un remplacement naif
        // ferait des chimeres.
        assert_eq!(
            remplacer_mot("ynh_replace_string --f x", "ynh_replace", "AUTRE"),
            "ynh_replace_string --f x"
        );
        assert_eq!(
            remplacer_mot("ynh_replace x", "ynh_replace", "AUTRE"),
            "AUTRE x"
        );
        assert_eq!(
            remplacer_mot("a_ynh_replace", "ynh_replace", "AUTRE"),
            "a_ynh_replace"
        );
    }

    #[test]
    fn les_helpers_sont_renommes_dans_tous_les_scripts() {
        let p = Paquet::new()
            .complet()
            .avec(
                "scripts/install",
                "ynh_add_nginx_config\nynh_systemd_action --action=start\n",
            )
            .avec("scripts/remove", "ynh_secure_remove --file=/x\n");
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();

        assert!(c.corriges.contains(&"HLP001".to_string()));
        assert!(c.fichiers.contains(&"scripts/install".to_string()));
        assert!(c.fichiers.contains(&"scripts/remove".to_string()));
        // Un script sans ancien nom n'est pas reecrit pour rien.
        assert!(!c.fichiers.contains(&"scripts/backup".to_string()));
        assert!(p.lire("scripts/install").contains("ynh_config_add_nginx"));
        assert!(p.lire("scripts/install").contains("ynh_systemctl"));
        assert!(p.lire("scripts/remove").contains("ynh_safe_rm"));
    }

    #[test]
    fn la_mise_a_jour_est_declaree_dans_la_section_des_sources() {
        let p = Paquet::new().complet();
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        let m = p.lire("manifest.toml");

        assert!(c.corriges.contains(&"MAJ001".to_string()));
        assert!(m.contains("autoupdate.strategy = \"latest_github_release\""));
        // Elle doit tomber dans la bonne section, pas apres la suivante.
        assert!(m.find("autoupdate").unwrap() < m.find("[resources.system_user]").unwrap());
    }

    #[test]
    fn la_strategie_suit_la_forge() {
        let p = Paquet::new().complet().avec(
            "manifest.toml",
            "packaging_format = 2\nid = \"d\"\n\n[resources.sources]\nurl = \"https://codeberg.org/a/b/archive/v1.tar.gz\"\n",
        );
        appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        assert!(p.lire("manifest.toml").contains("latest_forgejo_release"));
    }

    #[test]
    fn une_forge_inconnue_ne_fait_rien_plutot_que_de_deviner() {
        // Une mauvaise strategie produit un manifest qui ne se met jamais a
        // jour, sans que personne s'en apercoive.
        let p = Paquet::new().complet().avec(
            "manifest.toml",
            "packaging_format = 2\nid = \"d\"\n\n[resources.sources]\nurl = \"https://exemple.fr/b.tar.gz\"\n",
        );
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        assert!(!c.corriges.contains(&"MAJ001".to_string()));
        assert!(c.laisses.contains(&"MAJ001".to_string()));
        assert!(!p.lire("manifest.toml").contains("autoupdate"));
    }

    #[test]
    fn la_forme_avec_sous_section_est_reconnue() {
        // `[resources.sources.main]` est la forme la plus repandue.
        let p = Paquet::new().complet().avec(
            "manifest.toml",
            "packaging_format = 2\nid = \"d\"\n\n[resources.sources]\n\n    [resources.sources.main]\n    url = \"https://github.com/a/b/archive/v1.tar.gz\"\n\n[resources.system_user]\n",
        );
        appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        let m = p.lire("manifest.toml");
        assert!(m.contains("latest_github_release"), "{m}");
        assert!(m.find("autoupdate").unwrap() < m.find("[resources.system_user]").unwrap());
    }

    #[test]
    fn une_source_en_image_de_conteneur_n_est_pas_touchee() {
        // Elle ne se met pas a jour par ce mecanisme ; y declarer une
        // strategie produirait un manifest qui ment. Constate sur seafile.
        let p = Paquet::new().complet().avec(
            "manifest.toml",
            "packaging_format = 2\nid = \"d\"\n\n[resources.sources.main]\nformat = \"docker\"\namd64.url = \"exemple/image:1.0\"\n",
        );
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        assert!(!c.corriges.contains(&"MAJ001".to_string()));
        assert!(!p.lire("manifest.toml").contains("autoupdate"));
    }

    #[test]
    fn une_declaration_existante_n_est_pas_dupliquee() {
        let p = Paquet::new().complet().avec(
            "manifest.toml",
            "packaging_format = 2\nid = \"d\"\n\n[resources.sources]\nautoupdate.strategy = \"latest_github_release\"\n",
        );
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        assert!(c.rien());
        assert_eq!(p.lire("manifest.toml").matches("autoupdate").count(), 1);
    }

    #[test]
    fn la_description_reprend_celle_du_manifeste_plutot_que_d_inventer() {
        let p = Paquet::new().complet();
        std::fs::remove_file(p.0.join("doc/DESCRIPTION.md")).unwrap();
        appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        assert_eq!(p.lire("doc/DESCRIPTION.md").trim(), "Une demo");
    }

    #[test]
    fn ce_qui_demande_un_jugement_est_laisse() {
        // Choisir une licence ou passer au format 2 n'est pas mecanique.
        let p = Paquet::new().complet();
        std::fs::remove_file(p.0.join("LICENSE")).unwrap();
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        assert!(c.laisses.contains(&"DOC002".to_string()));
        assert!(!p.0.join("LICENSE").exists());
    }

    #[test]
    fn un_paquet_sain_n_est_pas_touche() {
        let p = Paquet::new().complet().avec(
            "manifest.toml",
            "packaging_format = 2\nid = \"d\"\ndescription.en = \"x\"\n\n[resources.sources]\nautoupdate.strategy = \"latest_github_release\"\n",
        );
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        assert!(c.rien(), "{:?}", c.fichiers);
    }

    #[test]
    fn deux_correctifs_identiques_donnent_la_meme_branche() {
        // Relancer ne doit pas ouvrir une seconde demande pour le meme travail.
        let a = Correctif {
            corriges: vec!["HLP001".into()],
            fichiers: vec!["scripts/install".into()],
            laisses: vec![],
        };
        let b = a.clone();
        let mut c = a.clone();
        c.corriges.push("MAJ001".into());
        assert_eq!(empreinte(&a), empreinte(&b));
        assert_ne!(empreinte(&a), empreinte(&c));
        assert_eq!(empreinte(&a).len(), 8);
    }

    #[test]
    fn le_message_de_commit_dit_ce_qui_a_change() {
        let p = Paquet::new()
            .complet()
            .avec("scripts/install", "ynh_add_nginx_config\n");
        let c = appliquer(&p.0, &examiner(&p.0, None)).unwrap();
        let m = c.message("demo");
        assert!(m.starts_with("Amelioration du paquet demo"));
        assert!(m.contains("helpers renommes"));
        assert!(m.contains("aucun arbitrage"));
    }
}
