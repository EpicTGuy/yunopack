//! Client GitLab — gitlab.com et les instances auto-hebergees.
//!
//! GitLab n'a rien de commun avec GitHub ni avec Gitea : un projet s'y designe
//! par son chemin complet encode (`groupe%2Fsous-groupe%2Fprojet`), les
//! releases y sont des objets a part des tags, et les noms de champs different
//! partout. D'ou un module entier plutot qu'une adaptation.
//!
//! Les groupes imbriques sont la particularite qui compte : `framagit.org` et
//! `salsa.debian.org` en font un usage courant, et ne garder que les deux
//! premiers segments d'une URL y designerait un autre projet — ou aucun.

use serde::Deserialize;
use ynp_core::facts::{Release, ReleaseAsset, RepoMeta, SourceRef};

use crate::github::ForgeError;

pub struct GitLab {
    client: reqwest::Client,
    /// Racine de l'instance, ex. `https://framagit.org`.
    hote: String,
    jeton: Option<String>,
}

impl GitLab {
    pub fn new(hote: &str) -> Result<Self, ForgeError> {
        Ok(Self {
            client: crate::client_http()?,
            hote: format!("https://{}", hote.trim_start_matches("https://")),
            jeton: std::env::var("GITLAB_TOKEN").ok().filter(|t| !t.is_empty()),
        })
    }

    pub fn hote(&self) -> &str {
        &self.hote
    }

    async fn get(&self, chemin: &str) -> Result<String, ForgeError> {
        let mut req = self.client.get(format!("{}/api/v4{chemin}", self.hote));
        if let Some(t) = &self.jeton {
            req = req.header("PRIVATE-TOKEN", t);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.text().await?;

        match status.as_u16() {
            200 => Ok(body),
            // GitLab rend 404 sur un projet prive comme sur un projet absent :
            // c'est voulu de sa part, et indistinguable de l'exterieur.
            401 | 404 => Err(ForgeError::NotFound(chemin.to_string())),
            403 | 429 => Err(ForgeError::RateLimited {
                forge: self.hote.clone(),
                variable: "GITLAB_TOKEN".into(),
            }),
            other => Err(ForgeError::Unexpected {
                status: other,
                body: body.chars().take(200).collect(),
            }),
        }
    }

    pub async fn repo(&self, source: &mut SourceRef) -> Result<RepoMeta, ForgeError> {
        let body = self
            .get(&format!("/projects/{}?license=true", chemin_encode(source)))
            .await?;
        let raw: RawProjet = serde_json::from_str(&body).map_err(|e| ForgeError::Unexpected {
            status: 200,
            body: e.to_string(),
        })?;

        source.default_branch = Some(raw.default_branch.clone().unwrap_or_else(|| "main".into()));
        Ok(RepoMeta {
            description_readme: None,
            description: raw.description.filter(|d| !d.trim().is_empty()),
            homepage: raw.web_url,
            topics: raw.topics,
            stars: raw.star_count,
            archived: raw.archived,
            pushed_at: raw.last_activity_at,
            license_text: None,
            // `license.key` est en minuscules (`agpl-3.0`) la ou le manifest
            // attend un identifiant SPDX. `nickname` est libre et souvent
            // absent : la cle, normalisee, reste le plus fiable des deux.
            license_spdx: raw.license.and_then(|l| l.key).map(|k| k.to_uppercase()),
            fourche_de: raw.forked_from_project.map(|p| p.path_with_namespace),
            // Un projet dans un groupe est collectif ; dans un espace
            // personnel, il ne l'est pas.
            proprietaire_collectif: raw.namespace.map(|n| n.kind == "group"),
            contributeurs: None,
        })
    }

    pub async fn releases(&self, source: &SourceRef) -> Result<Vec<Release>, ForgeError> {
        let body = self
            .get(&format!(
                "/projects/{}/releases?per_page=30",
                chemin_encode(source)
            ))
            .await?;
        let raw: Vec<RawRelease> = serde_json::from_str(&body).unwrap_or_default();

        Ok(raw
            .into_iter()
            .filter(|r| !r.upcoming_release)
            .map(|r| Release {
                tarball_url: Some(archive_url(&self.hote, source, &r.tag_name)),
                assets: r
                    .assets
                    .map(|a| {
                        a.links
                            .into_iter()
                            .map(|l| ReleaseAsset {
                                name: l.name,
                                url: l.url,
                                // GitLab ne publie ni la taille de ses liens
                                // ni leur empreinte.
                                size: 0,
                                sha256: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                tag: r.tag_name,
                name: r.name,
                published_at: r.released_at,
                // GitLab n'a pas de notion de pre-release ; le nom du tag
                // reste le seul indice, et c'est le selecteur qui le lit.
                prerelease: false,
            })
            .collect())
    }

    pub async fn tags(&self, source: &SourceRef) -> Result<Vec<String>, ForgeError> {
        let body = self
            .get(&format!(
                "/projects/{}/repository/tags?per_page=50",
                chemin_encode(source)
            ))
            .await?;
        let raw: Vec<RawTag> = serde_json::from_str(&body).unwrap_or_default();
        Ok(raw.into_iter().map(|t| t.name).collect())
    }

    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ForgeError> {
        let mut req = self.client.get(url);
        if let Some(t) = &self.jeton {
            req = req.header("PRIVATE-TOKEN", t);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return match status.as_u16() {
                401 | 404 => Err(ForgeError::NotFound(url.to_string())),
                403 | 429 => Err(ForgeError::RateLimited {
                    forge: self.hote.clone(),
                    variable: "GITLAB_TOKEN".into(),
                }),
                other => Err(ForgeError::Unexpected {
                    status: other,
                    body: url.to_string(),
                }),
            };
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

/// Le chemin complet du projet, encode pour tenir dans un segment d'URL.
///
/// C'est ainsi que l'API v4 designe un projet sans en connaitre l'identifiant
/// numerique. Les groupes imbriques imposent d'encoder toutes les barres.
pub fn chemin_encode(source: &SourceRef) -> String {
    format!("{}/{}", source.owner, source.repo).replace('/', "%2F")
}

/// URL d'archive d'une reference.
///
/// La forme `/-/archive/` du site vaut pour un tag, une branche ou un commit,
/// et ne demande pas d'authentification sur un projet public — contrairement a
/// l'equivalent de l'API.
pub fn archive_url(hote: &str, source: &SourceRef, reference: &str) -> String {
    let nom = source.repo.replace('/', "-");
    format!(
        "{hote}/{}/{}/-/archive/{reference}/{nom}-{reference}.tar.gz",
        source.owner, source.repo
    )
}

// --- Representation brute des reponses de l'API ---

#[derive(Deserialize)]
struct RawProjet {
    default_branch: Option<String>,
    description: Option<String>,
    web_url: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    star_count: u32,
    #[serde(default)]
    archived: bool,
    last_activity_at: Option<String>,
    license: Option<RawLicence>,
    forked_from_project: Option<RawParent>,
    namespace: Option<RawNamespace>,
}

#[derive(Deserialize)]
struct RawLicence {
    key: Option<String>,
}

#[derive(Deserialize)]
struct RawParent {
    path_with_namespace: String,
}

#[derive(Deserialize)]
struct RawNamespace {
    /// `group` ou `user`.
    #[serde(default)]
    kind: String,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    name: Option<String>,
    released_at: Option<String>,
    #[serde(default)]
    upcoming_release: bool,
    assets: Option<RawAssets>,
}

#[derive(Deserialize)]
struct RawAssets {
    #[serde(default)]
    links: Vec<RawLink>,
}

#[derive(Deserialize)]
struct RawLink {
    name: String,
    url: String,
}

#[derive(Deserialize)]
struct RawTag {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::Forge;

    fn source(owner: &str, repo: &str) -> SourceRef {
        SourceRef {
            forge: Forge::GitLab,
            owner: owner.into(),
            repo: repo.into(),
            url: format!("https://framagit.org/{owner}/{repo}"),
            default_branch: None,
            commit: None,
        }
    }

    #[test]
    fn un_projet_simple_s_encode_avec_une_seule_barre() {
        assert_eq!(
            chemin_encode(&source("framasoft", "framadate")),
            "framasoft%2Fframadate"
        );
    }

    #[test]
    fn un_groupe_imbrique_encode_toutes_ses_barres() {
        // C'est le cas courant sur framagit et salsa.debian.org : n'en garder
        // que deux segments designerait un autre projet, ou aucun.
        assert_eq!(
            chemin_encode(&source("framasoft/framaspace", "argos")),
            "framasoft%2Fframaspace%2Fargos"
        );
    }

    #[test]
    fn l_url_d_archive_suit_la_forme_du_site() {
        assert_eq!(
            archive_url(
                "https://framagit.org",
                &source("framasoft", "framadate"),
                "v1.2"
            ),
            "https://framagit.org/framasoft/framadate/-/archive/v1.2/framadate-v1.2.tar.gz"
        );
    }

    #[test]
    fn une_branche_ou_un_commit_donnent_la_meme_forme() {
        for r in ["main", "a1b2c3d"] {
            let u = archive_url("https://gitlab.com", &source("a", "b"), r);
            assert!(u.ends_with(&format!("/-/archive/{r}/b-{r}.tar.gz")), "{u}");
        }
    }

    #[test]
    fn la_reponse_reelle_de_gitlab_se_relit() {
        // Extrait reel reduit : c'est le nommage qui differe des autres forges.
        let j = r#"{
          "default_branch": "main",
          "description": "Un service de sondage",
          "web_url": "https://framagit.org/framasoft/framadate",
          "topics": ["php"],
          "star_count": 412,
          "archived": false,
          "last_activity_at": "2026-09-01T12:00:00.000Z",
          "license": {"key": "agpl-3.0", "name": "GNU AGPLv3"},
          "namespace": {"kind": "group", "full_path": "framasoft"}
        }"#;
        let r: RawProjet = serde_json::from_str(j).unwrap();
        assert_eq!(r.star_count, 412);
        assert_eq!(r.license.unwrap().key.as_deref(), Some("agpl-3.0"));
        assert_eq!(r.namespace.unwrap().kind, "group");
    }

    #[test]
    fn une_fourche_nomme_son_parent() {
        let j = r#"{"forked_from_project":{"path_with_namespace":"gitlab-org/gitlab"}}"#;
        let r: RawProjet = serde_json::from_str(j).unwrap();
        assert_eq!(
            r.forked_from_project.unwrap().path_with_namespace,
            "gitlab-org/gitlab"
        );
    }

    #[test]
    fn une_release_sans_asset_reste_exploitable() {
        let j = r#"[{"tag_name":"v1.0","name":"1.0","released_at":"2026-01-01T00:00:00Z"}]"#;
        let r: Vec<RawRelease> = serde_json::from_str(j).unwrap();
        assert_eq!(r[0].tag_name, "v1.0");
        assert!(r[0].assets.is_none());
        assert!(!r[0].upcoming_release);
    }
}
