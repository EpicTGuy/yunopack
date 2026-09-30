//! Client GitHub.
//!
//! Seules les lectures dont le pipeline a besoin sont implementees. Le jeton
//! `GITHUB_TOKEN` est facultatif : sans lui l'API publique suffit, mais elle
//! limite a soixante requetes par heure, ce qui se sent vite.

use serde::Deserialize;
use ynp_core::facts::{Release, ReleaseAsset, RepoMeta, SourceRef};

const API: &str = "https://api.github.com";
#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("depot introuvable : {0}")]
    NotFound(String),
    /// La forge refuse de repondre pour cause de quota. Le nom de la variable
    /// a definir depend d'elle : le dire evite de chercher au mauvais endroit.
    #[error("quota d'API epuise sur {forge} — definir {variable} pour le relever")]
    RateLimited { forge: String, variable: String },
    #[error("reponse inattendue de la forge ({status}) : {body}")]
    Unexpected { status: u16, body: String },
    #[error("reseau : {0}")]
    Http(#[from] reqwest::Error),
    #[error("archive illisible : {0}")]
    Archive(#[from] std::io::Error),
}

/// Identifiant et date d'un commit, lus dans la reponse d'une forge.
///
/// La date est rendue sous la forme `AAAA.MM.JJ` : c'est ainsi que
/// l'autoupdater de YunoHost date une version quand l'amont n'en publie pas,
/// et deux conventions differentes feraient diverger le paquet au premier
/// cycle de mise a jour.
pub fn commit_de(
    sha: &serde_json::Value,
    date: &serde_json::Value,
) -> Result<(String, String), ForgeError> {
    let (Some(sha), Some(date)) = (sha.as_str(), date.as_str()) else {
        return Err(ForgeError::Unexpected {
            status: 200,
            body: "commit sans identifiant ni date".into(),
        });
    };
    let jour = date.get(..10).unwrap_or(date).replace('-', ".");
    Ok((sha.to_string(), jour))
}

pub struct GitHub {
    client: reqwest::Client,
    token: Option<String>,
}

impl GitHub {
    pub fn new() -> Result<Self, ForgeError> {
        Ok(Self {
            client: crate::client_http()?,
            token: std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()),
        })
    }

    async fn get(&self, path: &str) -> Result<String, ForgeError> {
        let (status, body) = crate::obtenir(|| {
            let req = self.client.get(format!("{API}{path}"));
            match &self.token {
                Some(t) => req.bearer_auth(t),
                None => req,
            }
        })
        .await?;

        match status {
            200 => Ok(body),
            404 => Err(ForgeError::NotFound(path.to_string())),
            // 403 sans jeton est presque toujours le quota ; le distinguer
            // evite a l'utilisateur de chercher un probleme de droits.
            403 | 429 => Err(ForgeError::RateLimited {
                forge: "github.com".into(),
                variable: "GITHUB_TOKEN".into(),
            }),
            other => Err(ForgeError::Unexpected {
                status: other,
                body: truncate(&body),
            }),
        }
    }

    /// Metadonnees du depot, et branche par defaut renseignee dans `source`.
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
            description: raw.description,
            homepage: raw.homepage.filter(|h| !h.trim().is_empty()),
            topics: raw.topics,
            stars: raw.stargazers_count,
            archived: raw.archived,
            pushed_at: raw.pushed_at,
            // Le texte de licence est lu depuis l'arborescence, pas ici.
            license_text: None,
            license_spdx: raw
                .license
                .and_then(|l| l.spdx_id)
                // GitHub rend « NOASSERTION » quand il voit un fichier de
                // licence qu'il ne sait pas identifier : ce n'est pas un SPDX.
                .filter(|s| s != "NOASSERTION"),
            fourche_de: raw.parent.map(|p| p.full_name),
            proprietaire_collectif: raw.owner.map(|o| o.r#type == "Organization"),
            contributeurs: None,
        })
    }

    pub async fn releases(&self, source: &SourceRef) -> Result<Vec<Release>, ForgeError> {
        let path = format!(
            "/repos/{}/{}/releases?per_page=30",
            source.owner, source.repo
        );
        let body = self.get(&path).await?;
        let raw: Vec<RawRelease> = serde_json::from_str(&body).unwrap_or_default();

        Ok(raw
            .into_iter()
            .filter(|r| !r.draft)
            .map(|r| Release {
                tarball_url: Some(archive_url(source, &r.tag_name)),
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
                        // `sha256:abc…` : on ne garde que l'empreinte.
                        sha256: a
                            .digest
                            .as_deref()
                            .and_then(|d| d.strip_prefix("sha256:"))
                            .map(str::to_string),
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn tags(&self, source: &SourceRef) -> Result<Vec<String>, ForgeError> {
        let path = format!("/repos/{}/{}/tags?per_page=50", source.owner, source.repo);
        let body = self.get(&path).await?;
        let raw: Vec<RawTag> = serde_json::from_str(&body).unwrap_or_default();
        Ok(raw.into_iter().map(|t| t.name).collect())
    }

    /// Tous les tags qui commencent par `prefixe`.
    pub async fn tags_du_prefixe(
        &self,
        source: &SourceRef,
        prefixe: &str,
    ) -> Result<Vec<String>, ForgeError> {
        #[derive(serde::Deserialize)]
        struct Ref {
            #[serde(rename = "ref")]
            nom: String,
        }
        let path = format!(
            "/repos/{}/{}/git/matching-refs/tags/{prefixe}?per_page=100",
            source.owner, source.repo
        );
        let body = self.get(&path).await?;
        let refs: Vec<Ref> = serde_json::from_str(&body).unwrap_or_default();
        Ok(refs
            .into_iter()
            .filter_map(|r| r.nom.strip_prefix("refs/tags/").map(str::to_string))
            .collect())
    }

    /// Le dernier commit d'une branche : son identifiant et sa date.
    pub async fn dernier_commit(
        &self,
        source: &SourceRef,
        branche: &str,
    ) -> Result<(String, String), ForgeError> {
        let path = format!("/repos/{}/{}/commits/{branche}", source.owner, source.repo);
        let body = self.get(&path).await?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| ForgeError::Unexpected {
                status: 200,
                body: e.to_string(),
            })?;
        commit_de(&v["sha"], &v["commit"]["author"]["date"])
    }

    /// Telecharge et extrait l'archive du depot a une reference donnee.
    pub async fn tree(
        &self,
        source: &SourceRef,
        reference: &str,
    ) -> Result<ynp_core::tree::RepoTree, ForgeError> {
        let bytes = self.download(&archive_url(source, reference)).await?;
        Ok(crate::archive::extract(&bytes)?)
    }

    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ForgeError> {
        let mut req = self.client.get(url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return match status.as_u16() {
                404 => Err(ForgeError::NotFound(url.to_string())),
                403 | 429 => Err(ForgeError::RateLimited {
                    forge: "github.com".into(),
                    variable: "GITHUB_TOKEN".into(),
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

/// URL d'archive d'une reference. Vaut pour un tag comme pour une branche ou
/// un commit, ce qui evite trois cas particuliers.
pub fn archive_url(source: &SourceRef, reference: &str) -> String {
    format!(
        "https://github.com/{}/{}/archive/{reference}.tar.gz",
        source.owner, source.repo
    )
}

fn truncate(s: &str) -> String {
    s.chars().take(200).collect()
}

// --- Representation brute des reponses de l'API ---

#[derive(Deserialize)]
struct RawRepo {
    default_branch: String,
    description: Option<String>,
    homepage: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    stargazers_count: u32,
    #[serde(default)]
    archived: bool,
    pushed_at: Option<String>,
    license: Option<RawLicense>,
    /// Present uniquement quand le depot est une fourche.
    parent: Option<RawParent>,
    owner: Option<RawOwner>,
}

#[derive(Deserialize)]
struct RawParent {
    full_name: String,
}

#[derive(Deserialize)]
struct RawOwner {
    /// `User` ou `Organization`.
    #[serde(default)]
    r#type: String,
}

#[derive(Deserialize)]
struct RawLicense {
    spdx_id: Option<String>,
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
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Deserialize)]
struct RawTag {
    name: String,
}

#[cfg(test)]
mod tests {

    #[test]
    fn un_commit_donne_son_identifiant_et_sa_date_au_format_de_yunohost() {
        let (sha, date) = commit_de(
            &serde_json::json!("3e192bcc"),
            &serde_json::json!("2026-09-03T14:12:00Z"),
        )
        .unwrap();
        assert_eq!(sha, "3e192bcc");
        assert_eq!(date, "2026.09.03");
    }

    #[test]
    fn une_reponse_incomplete_est_une_erreur_et_non_une_date_inventee() {
        assert!(commit_de(&serde_json::json!("abc"), &serde_json::Value::Null).is_err());
    }
    use super::*;
    use ynp_core::facts::Forge;

    fn source() -> SourceRef {
        SourceRef {
            forge: Forge::GitHub,
            owner: "gristlabs".into(),
            repo: "grist-core".into(),
            url: "https://github.com/gristlabs/grist-core".into(),
            default_branch: Some("main".into()),
            commit: None,
        }
    }

    #[test]
    fn l_url_d_archive_vaut_pour_un_tag_comme_pour_une_branche() {
        assert_eq!(
            archive_url(&source(), "v1.2.3"),
            "https://github.com/gristlabs/grist-core/archive/v1.2.3.tar.gz"
        );
        assert_eq!(
            archive_url(&source(), "main"),
            "https://github.com/gristlabs/grist-core/archive/main.tar.gz"
        );
    }

    #[test]
    fn une_licence_non_identifiee_par_github_n_est_pas_prise_pour_un_spdx() {
        // GitHub rend « NOASSERTION » quand il voit un LICENSE qu'il ne
        // reconnait pas. La traiter comme un identifiant ferait passer la
        // gate G0 a tort.
        let raw: RawRepo = serde_json::from_str(
            r#"{"default_branch":"main","license":{"spdx_id":"NOASSERTION"}}"#,
        )
        .unwrap();
        let spdx = raw
            .license
            .and_then(|l| l.spdx_id)
            .filter(|s| s != "NOASSERTION");
        assert_eq!(spdx, None);
    }

    #[test]
    fn une_reponse_de_release_reelle_est_decodee() {
        let body = r#"[{
            "tag_name":"v1.1.15","name":"Version 1.1.15","published_at":"2026-01-02T10:00:00Z",
            "prerelease":false,"draft":false,
            "assets":[{"name":"app-amd64.tar.gz","browser_download_url":"https://x/a","size":1024}]
        },{"tag_name":"v2.0-rc1","draft":true,"assets":[]}]"#;
        let raw: Vec<RawRelease> = serde_json::from_str(body).unwrap();
        let kept: Vec<_> = raw.into_iter().filter(|r| !r.draft).collect();

        assert_eq!(kept.len(), 1, "les brouillons ne sont pas des releases");
        assert_eq!(kept[0].tag_name, "v1.1.15");
        assert_eq!(kept[0].assets[0].size, 1024);
    }
}
