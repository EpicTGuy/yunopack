//! Conformite statique du paquet genere : la gate G2.
//!
//! Quatre familles de controles, du moins couteux au plus bavard :
//!
//! 1. la structure — les fichiers qu'un paquet doit contenir ;
//! 2. le manifest, valide contre le schema officiel de YunoHost ;
//! 3. les scripts, avec les regles portees de `package_linter` ;
//! 4. les jetons `__MAJUSCULE__` des fichiers de configuration, dont chacun
//!    doit correspondre a un reglage que le paquet fournit reellement.
//!
//! Le quatrieme point vient d'une installation qui a echoue : un jeton sans
//! reglage arrete l'installation, et rien dans le paquet ne le laissait voir.

pub mod audit;
pub mod correctif;
pub mod placeholders;
pub mod scripts;

use std::path::Path;
use ynp_core::finding::{Evidence, Finding, Severity};
use ynp_core::AppSpec;

/// Schema officiel, incorpore au binaire pour que la verification fonctionne
/// hors ligne et sur un hote qui n'a recu que l'executable.
const SCHEMA_MANIFEST: &str = include_str!("../../../assets/schemas/manifest.v2.schema.json");

/// Scripts qu'un paquet doit fournir.
const SCRIPTS_REQUIS: &[&str] = &["install", "remove", "upgrade", "backup", "restore"];

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("lecture de {chemin} : {source}")]
    Lecture {
        chemin: String,
        source: std::io::Error,
    },
}

/// Verifie un paquet. `spec` sert a savoir quels reglages il fournit.
pub fn verify(racine: &Path, spec: &AppSpec) -> Result<Vec<Finding>, VerifyError> {
    let mut out = Vec::new();
    out.extend(structure(racine));
    out.extend(manifest(racine)?);
    out.extend(tous_les_scripts(racine)?);
    out.extend(jetons(racine, spec)?);
    out.extend(marqueurs_restants(racine)?);

    out.sort_by(|a, b| a.severity.cmp(&b.severity).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

fn structure(racine: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    let manque = |chemin: &str, severite: Severity, raison: &str| {
        Finding::new(
            "STRUCT001",
            severite,
            format!("Fichier manquant : {chemin}"),
        )
        .detail(raison.to_string())
        .evidence(Evidence::file(chemin))
    };

    if !racine.join("manifest.toml").is_file() {
        out.push(manque(
            "manifest.toml",
            Severity::Blocker,
            "C'est la carte d'identite du paquet.",
        ));
    }
    for script in SCRIPTS_REQUIS {
        let chemin = racine.join("scripts").join(script);
        if !chemin.is_file() {
            out.push(manque(
                &format!("scripts/{script}"),
                Severity::Blocker,
                "YunoHost appelle ce script ; son absence rend l'operation impossible.",
            ));
        } else if !est_executable(&chemin) {
            out.push(
                Finding::new(
                    "STRUCT002",
                    Severity::Blocker,
                    format!("scripts/{script} n'est pas executable"),
                )
                .remediation("chmod +x".to_string())
                .evidence(Evidence::file(format!("scripts/{script}"))),
            );
        }
    }
    // Controles appris du linter officiel, qui les a signales sur nos propres
    // paquets avant que nous les reproduisions ici.
    let licence = racine.join("LICENSE");
    match std::fs::read_to_string(&licence) {
        Err(_) => out.push(manque(
            "LICENSE",
            Severity::Blocker,
            "Le linter officiel refuse un paquet sans fichier de licence.",
        )),
        Ok(texte) if texte.trim().len() < 200 => out.push(
            Finding::new(
                "STRUCT003",
                Severity::Major,
                "LICENSE trop court pour etre une licence",
            )
            .detail(format!(
                "{} caracteres : c'est un renvoi, pas un texte de licence. Le linter \
                     officiel refuse les LICENSE reduits a une mention.",
                texte.trim().len()
            ))
            .remediation("Recopier le texte amont dans docs.license_text de l'appspec.".to_string())
            .evidence(Evidence::file("LICENSE")),
        ),
        Ok(_) => {}
    }

    if let Ok(readme) = std::fs::read_to_string(racine.join("README.md")) {
        // Le linter reconnait un README genere a ces deux marqueurs ; sans eux
        // il suppose une edition a la main et avertit.
        if !readme.contains("This README was automatically generated") {
            out.push(
                Finding::new(
                    "STRUCT004",
                    Severity::Minor,
                    "README sans marqueur de generation",
                )
                .detail(
                    "Le linter officiel avertit quand il ne reconnait pas un README genere."
                        .to_string(),
                )
                .evidence(Evidence::file("README.md")),
            );
        }
    }

    if !racine.join("doc/DESCRIPTION.md").is_file() {
        out.push(manque(
            "doc/DESCRIPTION.md",
            Severity::Minor,
            "Le catalogue et le README s'appuient dessus.",
        ));
    }
    out
}

#[cfg(unix)]
fn est_executable(chemin: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(chemin)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn est_executable(_: &Path) -> bool {
    true
}

fn manifest(racine: &Path) -> Result<Vec<Finding>, VerifyError> {
    let chemin = racine.join("manifest.toml");
    let Ok(texte) = std::fs::read_to_string(&chemin) else {
        return Ok(Vec::new()); // deja signale par la structure
    };

    let valeur: toml::Value = match toml::from_str(&texte) {
        Ok(v) => v,
        Err(e) => {
            return Ok(vec![Finding::new(
                "MANIFEST001",
                Severity::Blocker,
                "manifest.toml illisible",
            )
            .detail(e.to_string())
            .evidence(Evidence::file("manifest.toml"))]);
        }
    };

    let json = serde_json::to_value(&valeur).unwrap_or(serde_json::Value::Null);
    let schema: serde_json::Value = serde_json::from_str(SCHEMA_MANIFEST).expect("schema valide");
    let validateur = jsonschema::validator_for(&schema).expect("schema compilable");

    let mut out: Vec<Finding> = validateur
        .iter_errors(&json)
        .map(|e| {
            let chemin_champ = e.instance_path.to_string();
            let ou = if chemin_champ.is_empty() {
                "(racine)".into()
            } else {
                chemin_champ
            };
            Finding::new(
                "MANIFEST002",
                Severity::Blocker,
                format!("Manifest invalide : {ou}"),
            )
            .detail(e.to_string())
            .remediation("Corriger appspec.toml, puis regenerer.".to_string())
            .evidence(Evidence::file("manifest.toml"))
        })
        .collect();

    // Contraintes que le schema n'exprime pas mais que le linter applique.
    if let Some(nom) = json.get("name").and_then(|v| v.as_str()) {
        if nom.chars().count() > 23 {
            out.push(
                Finding::new("MANIFEST003", Severity::Minor, "Nom trop long")
                    .detail(format!(
                        "{} caracteres ; la limite est 23.",
                        nom.chars().count()
                    ))
                    .evidence(Evidence::file("manifest.toml")),
            );
        }
    }
    if let Some(d) = json.pointer("/description/en").and_then(|v| v.as_str()) {
        if d.chars().count() > 150 {
            out.push(
                Finding::new("MANIFEST004", Severity::Minor, "Description trop longue")
                    .detail(format!(
                        "{} caracteres ; la limite est 150.",
                        d.chars().count()
                    ))
                    .evidence(Evidence::file("manifest.toml")),
            );
        }
    }
    // Declarer une base sans installer son serveur laisse l'application sans
    // rien a quoi se connecter sur une instance qui ne l'a pas deja.
    let a_une_base = json.pointer("/resources/database").is_some();
    let a_des_paquets = json
        .pointer("/resources/apt/packages")
        .and_then(|p| p.as_array())
        .is_some_and(|a| !a.is_empty());
    if a_une_base && !a_des_paquets {
        out.push(
            Finding::new(
                "MANIFEST006",
                Severity::Major,
                "Base declaree sans serveur installe",
            )
            .detail(
                "`[resources.database]` provisionne une base, mais `[resources.apt]` \
                     n'installe ni postgresql ni mariadb-server."
                    .to_string(),
            )
            .remediation("Ajouter le serveur aux dependances apt.".to_string())
            .evidence(Evidence::file("manifest.toml")),
        );
    }

    if let Some(v) = json.get("version").and_then(|v| v.as_str()) {
        if !v.contains("~ynh") {
            out.push(
                Finding::new(
                    "MANIFEST005",
                    Severity::Blocker,
                    "Version sans suffixe de revision",
                )
                .detail(format!(
                    "« {v} » devrait se terminer par ~ynhN. Sans ce suffixe, aucune mise a \
                         jour du paquet seul n'est possible."
                ))
                .evidence(Evidence::file("manifest.toml")),
            );
        }
    }
    Ok(out)
}

fn tous_les_scripts(racine: &Path) -> Result<Vec<Finding>, VerifyError> {
    let mut out = Vec::new();
    let dossier = racine.join("scripts");
    let Ok(entrees) = std::fs::read_dir(&dossier) else {
        return Ok(out);
    };

    let mut chemins: Vec<_> = entrees.filter_map(Result::ok).map(|e| e.path()).collect();
    chemins.sort();

    for chemin in chemins {
        if !chemin.is_file() {
            continue;
        }
        let nom = chemin
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let Some(contenu) = lire_si_texte(&chemin)? else {
            continue;
        };
        out.extend(scripts::verifier(&format!("scripts/{nom}"), &contenu));
    }
    Ok(out)
}

fn jetons(racine: &Path, spec: &AppSpec) -> Result<Vec<Finding>, VerifyError> {
    let mut out = Vec::new();
    let fournis = placeholders::disponibles(spec);
    let dossier = racine.join("conf");
    let Ok(entrees) = std::fs::read_dir(&dossier) else {
        return Ok(out);
    };

    let mut chemins: Vec<_> = entrees.filter_map(Result::ok).map(|e| e.path()).collect();
    chemins.sort();

    for chemin in chemins {
        if !chemin.is_file() {
            continue;
        }
        let nom = chemin
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let Some(contenu) = lire_si_texte(&chemin)? else {
            continue;
        };

        for jeton in placeholders::extraire(&contenu) {
            if fournis.contains(&jeton) {
                continue;
            }
            out.push(
                Finding::new(
                    "CONF001",
                    Severity::Blocker,
                    format!("Jeton sans reglage correspondant : __{jeton}__"),
                )
                .detail(format!(
                    "YunoHost remplace tout jeton de cette forme par la valeur du reglage \
                     ${}, y compris dans les commentaires. Ce reglage n'existe pas : \
                     l'installation s'arretera sur « Variable ${} wasn't initialized ».",
                    jeton.to_lowercase(),
                    jeton.to_lowercase()
                ))
                .remediation(
                    "Soit definir le reglage dans le script d'installation avec \
                     `ynh_app_setting_set`, soit ne pas ecrire ce jeton — un exemple dans un \
                     commentaire suffit a declencher l'erreur."
                        .to_string(),
                )
                .evidence(Evidence::file(format!("conf/{nom}"))),
            );
        }
    }
    Ok(out)
}

/// Marqueurs laisses par la generation quand un champ n'a pas ete complete.
fn marqueurs_restants(racine: &Path) -> Result<Vec<Finding>, VerifyError> {
    let mut out = Vec::new();
    for chemin in fichiers_texte(racine) {
        // Un binaire egare ne porte pas de marqueur : on passe plutot que
        // d'interrompre toute la verification.
        let Ok(contenu) = std::fs::read_to_string(&chemin) else {
            continue;
        };
        for (numero, ligne) in contenu.lines().enumerate() {
            if ligne.contains("FIXME(yunopack)") {
                let relatif = chemin
                    .strip_prefix(racine)
                    .unwrap_or(&chemin)
                    .display()
                    .to_string();
                out.push(
                    Finding::new("FIXME001", Severity::Blocker, "Champ non complete")
                        .detail(
                            "Le paquet contient un marqueur depose par la generation. Il ne \
                             doit jamais sortir du pipeline : c'est la garantie qu'aucune \
                             valeur n'a ete inventee."
                                .to_string(),
                        )
                        .remediation(
                            "Completer le champ dans appspec.toml, puis regenerer.".to_string(),
                        )
                        .evidence(Evidence::at(relatif, numero as u32 + 1, ligne.trim())),
                );
            }
        }
    }
    Ok(out)
}

fn fichiers_texte(racine: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut piles = vec![racine.to_path_buf()];
    while let Some(dossier) = piles.pop() {
        let Ok(entrees) = std::fs::read_dir(&dossier) else {
            continue;
        };
        for entree in entrees.filter_map(Result::ok) {
            let chemin = entree.path();
            if chemin.is_dir() {
                piles.push(chemin);
            } else if chemin.is_file() {
                out.push(chemin);
            }
        }
    }
    out.sort();
    out
}

/// Le contenu textuel d'un fichier, ou rien s'il est binaire.
///
/// Un paquet contient legitimement des octets qui ne sont pas du texte — un
/// logo, une image de documentation. Les verifications de ce module portent
/// sur du texte ; les leur soumettre faisait echouer tout le paquet sur un
/// « stream did not contain valid UTF-8 », ce qui n'apprend rien a personne.
/// Une erreur d'acces, elle, reste une erreur.
fn lire_si_texte(chemin: &Path) -> Result<Option<String>, VerifyError> {
    match std::fs::read_to_string(chemin) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(None),
        Err(source) => Err(VerifyError::Lecture {
            chemin: chemin.display().to_string(),
            source,
        }),
    }
}

/// Gate G2 : le paquet est-il conforme ?
pub fn gate(constats: &[Finding]) -> ynp_core::gate::GateOutcome {
    let bloquants: Vec<_> = constats
        .iter()
        .filter(|f| f.severity == Severity::Blocker)
        .cloned()
        .collect();
    if bloquants.is_empty() {
        ynp_core::gate::GateOutcome::Pass
    } else {
        ynp_core::gate::GateOutcome::fail(bloquants)
    }
}

#[cfg(test)]
mod schema_incorpore {
    /// Les deux `expect` du controle de manifest ne sont justifies que si le
    /// schema incorpore est reellement valide et compilable. C'est ce test qui
    /// le garantit : sans lui, une resynchronisation malheureuse du schema
    /// ferait paniquer `verify` chez l'utilisateur.
    #[test]
    fn le_schema_incorpore_est_valide_et_compilable() {
        let schema: serde_json::Value =
            serde_json::from_str(super::SCHEMA_MANIFEST).expect("le schema doit etre du JSON");
        assert!(
            jsonschema::validator_for(&schema).is_ok(),
            "le schema doit compiler"
        );

        // Et il doit bien decrire un manifest : sans quoi on validerait contre
        // n'importe quoi sans s'en apercevoir.
        let requis = schema
            .get("required")
            .and_then(|r| r.as_array())
            .expect("le schema declare des champs obligatoires");
        let noms: Vec<&str> = requis.iter().filter_map(|v| v.as_str()).collect();
        for champ in ["packaging_format", "id", "version", "resources"] {
            assert!(noms.contains(&champ), "{champ} devrait etre obligatoire");
        }
    }
}

#[cfg(test)]
mod fichiers_binaires {
    use super::*;

    #[test]
    fn un_fichier_binaire_est_ignore_plutot_que_fatal() {
        // Un paquet contient legitimement un logo. Le soumettre aux
        // verifications de texte faisait echouer tout le paquet.
        let d = std::env::temp_dir().join(format!("ynp-bin-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let png = d.join("logo.png");
        std::fs::write(&png, [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff]).unwrap();

        assert!(matches!(lire_si_texte(&png), Ok(None)));

        let txt = d.join("conf.env");
        std::fs::write(&txt, "PORT=__PORT__\n").unwrap();
        assert_eq!(
            lire_si_texte(&txt).unwrap().as_deref(),
            Some("PORT=__PORT__\n")
        );

        // Un fichier absent reste une erreur : l'ignorer masquerait un paquet
        // incomplet.
        assert!(lire_si_texte(&d.join("absent")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
