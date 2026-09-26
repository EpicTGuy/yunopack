//! Types partages du pipeline yunopack.
//!
//! Le pipeline est une chaine de transformations, chacune avec un contrat de
//! donnees explicite :
//!
//! ```text
//! URL -> RepoFacts -> Feasibility -> AppSpec -> arbre de paquet -> GateReport
//!        (collecte)   (regles)       (decision) (rendu)            (validation)
//! ```
//!
//! Ce crate ne fait *que* definir ces contrats. Il ne connait ni le reseau, ni
//! le systeme de fichiers, ni YunoHost : c'est ce qui permet a chaque etage
//! d'etre teste isolement, et a plusieurs agents de travailler en parallele une
//! fois ces types figes.

pub mod facts;
pub mod finding;
pub mod gate;
pub mod known;
pub mod spec;
pub mod traduction;
pub mod tree;

pub use facts::{
    ArchAsset, BuildRecipe, ComposeFacts, ConfigRole, ConfigVar, Database, Forge, RepoFacts,
    SourceRef, SourceSelection, Technology,
};
pub use finding::{Evidence, Feasibility, Finding, Severity, Verdict};
pub use gate::{GateId, GateOutcome, GateReport, GateResult};
pub use known::{Known, Unresolved};
pub use spec::AppSpec;
pub use tree::RepoTree;

/// Seuil de score par defaut separant « faisable » de « faisable avec travail ».
pub const DEFAULT_FEASIBILITY_THRESHOLD: u8 = 60;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("identifiant d'app invalide : {0} (attendu : minuscules, chiffres et tirets)")]
    InvalidAppId(String),
    #[error("serialisation TOML : {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("lecture TOML : {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("JSON : {0}")]
    Json(#[from] serde_json::Error),
}

/// Derive un identifiant d'app YunoHost valide a partir d'un nom de depot.
///
/// Contraintes du manifest : minuscules, chiffres et tirets, commencant par une
/// lettre. Le suffixe `_ynh` frequent dans les noms de depot est retire, sinon
/// on obtiendrait des apps nommees `foo-ynh`.
pub fn app_id_from_repo(repo: &str) -> Result<String, CoreError> {
    app_id_from(None, repo)
}

/// Noms de depot qui ne designent pas l'application mais une composante.
///
/// `miniflux/v2` et `gotify/server` doivent donner « miniflux » et « gotify »,
/// comme le font les paquets officiels — pas « v2 » ni « server ».
const NOMS_GENERIQUES: &[&str] = &[
    "server",
    "core",
    "app",
    "web",
    "api",
    "main",
    "backend",
    "frontend",
    "client",
    "docker",
    "self-hosted",
    "selfhosted",
    "community",
    "oss",
    "ce",
    "src",
];

/// Derive l'identifiant, en preferant le proprietaire quand le nom du depot
/// n'identifie rien par lui-meme.
pub fn app_id_from(owner: Option<&str>, repo: &str) -> Result<String, CoreError> {
    let base = repo
        .trim_end_matches("_ynh")
        .trim_end_matches("-ynh")
        .to_lowercase();
    // `v2`, `v3`... sont des noms de version, pas d'application.
    let est_une_version =
        base.starts_with('v') && base.len() <= 3 && base[1..].chars().all(|c| c.is_ascii_digit());

    if let Some(owner) = owner {
        if NOMS_GENERIQUES.contains(&base.as_str()) || est_une_version {
            return normaliser(owner);
        }
    }
    normaliser(repo)
}

fn normaliser(repo: &str) -> Result<String, CoreError> {
    let base = repo.trim_end_matches("_ynh").trim_end_matches("-ynh");
    let mut id: String = base
        .chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' => c,
            _ => '-',
        })
        .collect();

    while id.contains("--") {
        id = id.replace("--", "-");
    }
    let id = id.trim_matches('-').to_string();

    if id.is_empty() || !id.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err(CoreError::InvalidAppId(repo.to_string()));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_identifiant_est_normalise_depuis_le_nom_de_depot() {
        assert_eq!(app_id_from_repo("Uptime-Kuma").unwrap(), "uptime-kuma");
        assert_eq!(app_id_from_repo("my_cool.app").unwrap(), "my-cool-app");
        assert_eq!(app_id_from_repo("foo--bar").unwrap(), "foo-bar");
    }

    #[test]
    fn le_suffixe_ynh_du_depot_n_entre_pas_dans_l_identifiant() {
        assert_eq!(app_id_from_repo("nextcloud_ynh").unwrap(), "nextcloud");
        assert_eq!(app_id_from_repo("grist-ynh").unwrap(), "grist");
    }

    #[test]
    fn un_nom_de_depot_generique_cede_la_place_au_proprietaire() {
        // Comme le font les paquets officiels : miniflux_ynh, gotify_ynh.
        assert_eq!(app_id_from(Some("miniflux"), "v2").unwrap(), "miniflux");
        assert_eq!(app_id_from(Some("gotify"), "server").unwrap(), "gotify");
        assert_eq!(
            app_id_from(Some("toeverything"), "AFFiNE").unwrap(),
            "affine"
        );
        // Un nom de depot parlant l'emporte sur le proprietaire.
        assert_eq!(app_id_from(Some("block"), "buzz").unwrap(), "buzz");
        assert_eq!(app_id_from(Some("usememos"), "memos").unwrap(), "memos");
    }

    #[test]
    fn un_nom_qui_ne_peut_pas_donner_d_identifiant_valide_echoue() {
        assert!(app_id_from_repo("123").is_err());
        assert!(app_id_from_repo("___").is_err());
    }
}
