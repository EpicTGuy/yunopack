//! Client Gitea, et donc Forgejo — Codeberg compris.
//!
//! Forgejo est une fourche de Gitea et en a garde l'API ; un seul client sert
//! donc les deux. Sa forme est proche de celle de GitHub, mais pas identique :
//! `stars_count` plutot que `stargazers_count`, `website` plutot que
//! `homepage`, et aucune reconnaissance de licence. Ces ecarts sont la raison
//! d'etre de ce module : les masquer derriere une abstraction commune
//! reviendrait a pretendre que les forges se ressemblent plus qu'elles ne le
//! font.
//!
//! Il n'y a pas de quota par defaut sur une instance Gitea, contrairement a
//! GitHub. Un jeton reste possible pour les depots prives.

use serde::Deserialize;
use ynp_core::facts::{Release, ReleaseAsset, RepoMeta, SourceRef};

use crate::github::ForgeError;

pub struct Gitea {
    client: reqwest::Client,
    /// Racine de l'instance, ex. `https://codeberg.org`.
    hote: String,
    jeton: Option<String>,
}

impl Gitea {
    pub fn new(hote: &str) -> Result<Self, ForgeError> {
        Ok(Self {
            client: crate::client_http()?,
            hote: format!("https://{}", hote.trim_start_matches("https://")),
            // Un jeton par instance serait plus juste, mais demanderait a
            // l'utilisateur d'en declarer un par forge. Une variable unique
            // couvre le cas courant : une seule instance frequentee.
            jeton: std::env::var("FORGEJO_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
        })
    }

    async fn get(&self, chemin: &str) -> Result<String, ForgeError> {
        let (status, body) = crate::obtenir(|| {
            let req = self.client.get(format!("{}/api/v1{chemin}", self.hote));
            match &self.jeton {
                Some(t) => req.header("Authorization", format!("token {t}")),
                None => req,
            }
        })
        .await?;

        match status {
            200 => Ok(body),
            404 => Err(ForgeError::NotFound(chemin.to_string())),
            403 | 429 => Err(ForgeError::RateLimited {
                forge: self.hote.clone(),
                variable: "FORGEJO_TOKEN".into(),
            }),
            other => Err(ForgeError::Unexpected {
                status: other,
                body: body.chars().take(200).collect(),
            }),
        }
    }

    pub async fn repo(&self, source: &mut SourceRef) -> Result<RepoMeta, ForgeError> {
        let body = self
            .get(&format!("/repos/{}/{}", source.owner, source.repo))
            .await?;
        let raw: RawRepo = serde_json::from_str(&body).map_err(|e| ForgeError::Unexpected {
            status: 200,
            body: e.to_string(),
        })?;

        source.default_branch = Some(raw.default_branch.clone());
        Ok(RepoMeta {
            description_readme: None,
            description: raw.description.filter(|d| !d.trim().is_empty()),
            homepage: raw.website.filter(|h| !h.trim().is_empty()),
            topics: raw.topics,
            stars: raw.stars_count,
            archived: raw.archived,
            // Gitea ne distingue pas le dernier push du dernier changement de
            // metadonnee. C'est une approximation, et elle va dans le bon
            // sens : elle ne fait jamais passer un depot mort pour vivant.
            pushed_at: raw.updated_at,
            license_text: None,
            // L'API ne rend aucun identifiant SPDX. La licence sera lue dans
            // l'arborescence, comme pour un depot GitHub sans licence reconnue.
            license_spdx: None,
            fourche_de: raw.parent.map(|p| p.full_name),
            // Gitea distingue `user` et `organization` sur le proprietaire.
            proprietaire_collectif: raw
                .owner
                .map(|o| o.r#type.eq_ignore_ascii_case("organization")),
            contributeurs: None,
        })
    }

    pub async fn releases(&self, source: &SourceRef) -> Result<Vec<Release>, ForgeError> {
        let chemin = format!("/repos/{}/{}/releases?limit=30", source.owner, source.repo);
        let body = self.get(&chemin).await?;
        let raw: Vec<RawRelease> = serde_json::from_str(&body).unwrap_or_default();

        Ok(raw
            .into_iter()
            .filter(|r| !r.draft)
            .map(|r| Release {
                tarball_url: Some(archive_url(&self.hote, source, &r.tag_name)),
                tag: r.tag_name,
                name: r.name,
                published_at: r.published_at,
                prerelease: r.prerelease,
                assets: r
                    .assets
                    .into_iter()
                    .map(|a| ReleaseAsset {
                        name: a.name,
                        url: a.browser_download_url,
                        size: a.size,
                        // Gitea ne publie pas d'empreinte.
                        sha256: None,
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn tags(&self, source: &SourceRef) -> Result<Vec<String>, ForgeError> {
        let chemin = format!("/repos/{}/{}/tags?limit=50", source.owner, source.repo);
        let body = self.get(&chemin).await?;
        let raw: Vec<RawTag> = serde_json::from_str(&body).unwrap_or_default();
        Ok(raw.into_iter().map(|t| t.name).collect())
    }

    /// Le dernier commit d'une branche : son identifiant et sa date.
    pub async fn dernier_commit(
        &self,
        source: &SourceRef,
        branche: &str,
    ) -> Result<(String, String), ForgeError> {
        let chemin = format!("/repos/{}/{}/branches/{branche}", source.owner, source.repo);
        let body = self.get(&chemin).await?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| ForgeError::Unexpected {
                status: 200,
                body: e.to_string(),
            })?;
        crate::github::commit_de(&v["commit"]["id"], &v["commit"]["timestamp"])
    }

    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ForgeError> {
        let mut req = self.client.get(url);
        if let Some(t) = &self.jeton {
            req = req.header("Authorization", format!("token {t}"));
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return match status.as_u16() {
                404 => Err(ForgeError::NotFound(url.to_string())),
                403 | 429 => Err(ForgeError::RateLimited {
                    forge: self.hote.clone(),
                    variable: "FORGEJO_TOKEN".into(),
                }),
                other => Err(ForgeError::Unexpected {
                    status: other,
                    body: url.to_string(),
                }),
            };
        }
        Ok(resp.bytes().await?.to_vec())
    }

    pub fn hote(&self) -> &str {
        &self.hote
    }
}

/// URL d'archive d'une reference, valable pour un tag, une branche ou un commit.
pub fn archive_url(hote: &str, source: &SourceRef, reference: &str) -> String {
    format!(
        "{hote}/{}/{}/archive/{reference}.tar.gz",
        source.owner, source.repo
    )
}

// --- Representation brute des reponses de l'API ---

#[derive(Deserialize)]
struct RawRepo {
    default_branch: String,
    description: Option<String>,
    website: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    stars_count: u32,
    #[serde(default)]
    archived: bool,
    updated_at: Option<String>,
    parent: Option<RawParent>,
    owner: Option<RawOwner>,
}

#[derive(Deserialize)]
struct RawParent {
    full_name: String,
}

#[derive(Deserialize)]
struct RawOwner {
    #[serde(default)]
    r#type: String,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    name: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct RawTag {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::Forge;

    fn source() -> SourceRef {
        SourceRef {
            forge: Forge::Forgejo,
            owner: "forgejo".into(),
            repo: "forgejo".into(),
            url: "https://codeberg.org/forgejo/forgejo".into(),
            default_branch: None,
            commit: None,
        }
    }

    #[test]
    fn l_url_d_archive_suit_la_forme_de_gitea() {
        assert_eq!(
            archive_url("https://codeberg.org", &source(), "v11.0.0"),
            "https://codeberg.org/forgejo/forgejo/archive/v11.0.0.tar.gz"
        );
    }

    #[test]
    fn une_branche_ou_un_commit_donnent_la_meme_forme() {
        // C'est ce qui evite trois cas particuliers dans le selecteur de source.
        for r in ["main", "a1b2c3d"] {
            assert!(archive_url("https://codeberg.org", &source(), r)
                .ends_with(&format!("/{r}.tar.gz")));
        }
    }

    #[test]
    fn l_hote_est_normalise_avec_son_schema() {
        let g = Gitea::new("codeberg.org").unwrap();
        assert_eq!(g.hote(), "https://codeberg.org");
        let g = Gitea::new("https://codeberg.org").unwrap();
        assert_eq!(g.hote(), "https://codeberg.org");
    }

    #[test]
    fn la_reponse_reelle_de_codeberg_se_relit() {
        // Extrait reel, reduit aux champs qui nous interessent : c'est le
        // nommage qui differe de GitHub, et c'est la qu'on se trompe.
        let j = r#"{
          "default_branch": "forgejo",
          "description": "Forgejo is a self-hosted lightweight software forge.",
          "website": "https://forgejo.org",
          "topics": ["forge", "git"],
          "stars_count": 2100,
          "archived": false,
          "updated_at": "2026-09-20T10:00:00Z"
        }"#;
        let r: RawRepo = serde_json::from_str(j).unwrap();
        assert_eq!(r.stars_count, 2100);
        assert_eq!(r.website.as_deref(), Some("https://forgejo.org"));
        assert_eq!(r.default_branch, "forgejo");
    }

    #[test]
    fn une_release_sans_asset_reste_exploitable() {
        let j = r#"[{"tag_name":"v1.0","name":"v1.0","published_at":"2026-01-01T00:00:00Z"}]"#;
        let r: Vec<RawRelease> = serde_json::from_str(j).unwrap();
        assert_eq!(r[0].tag_name, "v1.0");
        assert!(r[0].assets.is_empty());
        assert!(!r[0].draft);
    }
}
