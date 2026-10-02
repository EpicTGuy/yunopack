//! Tout depot Git accessible en HTTP, quelle que soit la forge qui l'heberge.
//!
//! sourcehut, cgit (Savannah, et beaucoup d'instances auto-hebergees),
//! Bitbucket n'ont pas d'API commune. Le protocole Git, lui, est partout :
//! `git ls-remote` donne les tags et la branche par defaut sans rien
//! telecharger. Seule l'adresse de l'archive depend de l'hebergeur ; elle est
//! connue pour les formes repandues, et sans elle le depot n'est pas
//! packageable, puisque le manifeste doit pointer une archive.

use crate::github::ForgeError;
use tokio::process::Command;
use ynp_core::facts::{RepoMeta, SourceRef};

pub struct GitBrut {
    client: reqwest::Client,
    /// Forme de l'adresse d'archive de cet hebergeur.
    archives: Archives,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archives {
    /// `https://git.sr.ht/~u/r/archive/<ref>.tar.gz`
    Sourcehut,
    /// `https://hote/<chemin>.git/snapshot/<nom>-<ref>.tar.gz` (cgit)
    Cgit,
    /// `https://bitbucket.org/<o>/<r>/get/<ref>.tar.gz`
    Bitbucket,
}

fn erreur(texte: impl Into<String>) -> ForgeError {
    ForgeError::Unexpected {
        status: 0,
        body: texte.into(),
    }
}

/// `git` avec ses arguments ; la sortie standard, ou l'erreur.
async fn git(args: &[&str]) -> Result<String, ForgeError> {
    let sortie = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        Command::new("git")
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output(),
    )
    .await
    .map_err(|_| erreur(format!("git {} : delai depasse", args.join(" "))))?
    .map_err(|e| erreur(format!("git introuvable : {e}")))?;
    if !sortie.status.success() {
        return Err(erreur(format!(
            "git {} : {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&sortie.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&sortie.stdout).into_owned())
}

/// L'adresse a cloner. cgit sert les archives sous `/cgit/`, et le depot
/// lui-meme sous `/git/` : c'est la forme de Savannah.
fn depot(source: &SourceRef) -> String {
    source.url.trim_end_matches('/').replace("/cgit/", "/git/")
}

impl GitBrut {
    /// Reconnait l'hebergeur par son adresse, ou par une archive qui repond.
    pub async fn pour(source: &SourceRef) -> Option<Self> {
        let client = crate::client_http().ok()?;
        let url = depot(source);
        let archives = if url.contains("sr.ht/") {
            Archives::Sourcehut
        } else if url.contains("bitbucket.org/") {
            Archives::Bitbucket
        } else if url.contains("/cgit/") || url.contains("savannah.") {
            Archives::Cgit
        } else {
            return None;
        };
        // Un depot qui ne repond pas au protocole Git n'est pas un depot.
        git(&["ls-remote", "--symref", &url, "HEAD"]).await.ok()?;
        Some(Self { client, archives })
    }

    pub async fn repo(&self, source: &mut SourceRef) -> Result<RepoMeta, ForgeError> {
        let sortie = git(&["ls-remote", "--symref", &depot(source), "HEAD"]).await?;
        source.default_branch = sortie.lines().find_map(|l| {
            l.strip_prefix("ref: refs/heads/")
                .and_then(|r| r.split_whitespace().next())
                .map(str::to_string)
        });
        Ok(RepoMeta::default())
    }

    pub async fn tags(&self, source: &SourceRef) -> Result<Vec<String>, ForgeError> {
        let sortie = git(&["ls-remote", "--tags", "--refs", &depot(source)]).await?;
        Ok(sortie
            .lines()
            .filter_map(|l| l.split('\t').nth(1))
            .filter_map(|r| r.strip_prefix("refs/tags/"))
            .map(str::to_string)
            .collect())
    }

    /// Le dernier commit d'une branche, et sa date : un clone superficiel,
    /// sans les fichiers, suffit a la lire.
    pub async fn dernier_commit(
        &self,
        source: &SourceRef,
        branche: &str,
    ) -> Result<(String, String), ForgeError> {
        let dossier = std::env::temp_dir().join(format!(
            "yunopack-git-{}",
            std::process::id() as u64 ^ source.url.len() as u64
        ));
        let _ = std::fs::remove_dir_all(&dossier);
        let d = dossier.to_string_lossy().to_string();
        let resultat = async {
            git(&[
                "clone",
                "--quiet",
                "--bare",
                "--depth",
                "1",
                "--filter=blob:none",
                "--branch",
                branche,
                &depot(source),
                &d,
            ])
            .await?;
            let ligne = git(&["-C", &d, "log", "-1", "--format=%H %cs"]).await?;
            let (sha, date) = ligne
                .trim()
                .split_once(' ')
                .ok_or_else(|| erreur("git log : sortie inattendue"))?;
            Ok((sha.to_string(), date.replace('-', ".")))
        }
        .await;
        let _ = std::fs::remove_dir_all(&dossier);
        resultat
    }

    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ForgeError> {
        let r = self.client.get(url).send().await?;
        if !r.status().is_success() {
            return Err(erreur(format!("{url} : {}", r.status())));
        }
        Ok(r.bytes().await?.to_vec())
    }

    pub fn archive_url(&self, source: &SourceRef, reference: &str) -> String {
        let url = source.url.trim_end_matches('/').to_string();
        match self.archives {
            Archives::Sourcehut | Archives::Bitbucket => {
                let chemin = if self.archives == Archives::Sourcehut {
                    "archive"
                } else {
                    "get"
                };
                format!("{url}/{chemin}/{reference}.tar.gz")
            }
            Archives::Cgit => {
                let base = url.trim_end_matches(".git");
                let nom = base.rsplit('/').next().unwrap_or(base);
                format!("{base}.git/snapshot/{nom}-{reference}.tar.gz")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: &str) -> SourceRef {
        SourceRef {
            url: url.into(),
            ..Default::default()
        }
    }

    #[test]
    fn l_adresse_d_archive_suit_l_hebergeur() {
        let c = |a| GitBrut {
            client: crate::client_http().unwrap(),
            archives: a,
        };
        assert_eq!(
            c(Archives::Sourcehut).archive_url(&source("https://git.sr.ht/~nicoco/slidge"), "v1.0"),
            "https://git.sr.ht/~nicoco/slidge/archive/v1.0.tar.gz"
        );
        assert_eq!(
            c(Archives::Cgit).archive_url(
                &source("https://git.savannah.gnu.org/cgit/mediagoblin.git"),
                "v0.14"
            ),
            "https://git.savannah.gnu.org/cgit/mediagoblin.git/snapshot/mediagoblin-v0.14.tar.gz"
        );
    }
}
