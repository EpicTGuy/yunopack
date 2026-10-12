//! Le client de la forge qui heberge un depot donne.
//!
//! Deux implementations pour l'instant : GitHub, et Gitea — dont Forgejo est
//! une fourche, Codeberg une instance. Les masquer derriere un trait commun
//! n'apporterait rien ici : les appels sont quatre, et une enumeration se lit
//! mieux qu'une indirection.
//!
//! Une instance auto-hebergee ne se reconnait pas a son nom de domaine :
//! `git.exemple.fr` peut faire tourner n'importe quoi. On l'interroge donc,
//! plutot que de deviner — c'est ce que fait l'outil partout ailleurs.

use crate::git::GitBrut;
use crate::gitea::Gitea;
use crate::github::{ForgeError, GitHub};
use crate::gitlab::GitLab;
use ynp_core::facts::{Forge, Release, RepoMeta, SourceRef};

pub enum Client {
    GitHub(GitHub),
    Gitea(Gitea),
    GitLab(GitLab),
    Git(GitBrut),
}

impl Client {
    /// Choisit le client d'apres la forge reconnue, en interrogeant l'instance
    /// quand le nom de domaine ne dit rien.
    pub async fn pour(source: &SourceRef) -> Result<Self, ForgeError> {
        let hote = hote_de(&source.url);
        match source.forge {
            Forge::GitHub => Ok(Client::GitHub(GitHub::new()?)),
            Forge::Gitea | Forge::Forgejo => Ok(Client::Gitea(Gitea::new(&hote)?)),
            Forge::GitLab => Ok(Client::GitLab(GitLab::new(&hote)?)),
            Forge::Git => {
                GitBrut::pour(source)
                    .await
                    .map(Client::Git)
                    .ok_or_else(|| ForgeError::Unexpected {
                        status: 0,
                        body: format!(
                            "{hote} : depot Git injoignable, ou forme d'archive inconnue"
                        ),
                    })
            }
            Forge::Inconnue => {
                // Gitea d'abord : sa reponse est franche et sans
                // authentification. GitLab ensuite, teste sur le projet
                // lui-meme puisque ses points generiques demandent un jeton.
                if est_une_instance_gitea(&hote).await {
                    Ok(Client::Gitea(Gitea::new(&hote)?))
                } else if est_une_instance_gitlab(&hote, source).await {
                    Ok(Client::GitLab(GitLab::new(&hote)?))
                } else if let Some(g) = GitBrut::pour(source).await {
                    Ok(Client::Git(g))
                } else {
                    Err(ForgeError::Unexpected {
                        status: 0,
                        body: format!(
                            "{hote} : forge non reconnue — GitHub, GitLab, Gitea et \
                             Forgejo (dont Codeberg) sont pris en charge"
                        ),
                    })
                }
            }
        }
    }

    pub async fn repo(&self, source: &mut SourceRef) -> Result<RepoMeta, ForgeError> {
        match self {
            Client::GitHub(c) => c.repo(source).await,
            Client::Gitea(c) => c.repo(source).await,
            Client::GitLab(c) => c.repo(source).await,
            Client::Git(c) => c.repo(source).await,
        }
    }

    pub async fn releases(&self, source: &SourceRef) -> Result<Vec<Release>, ForgeError> {
        match self {
            Client::GitHub(c) => c.releases(source).await,
            Client::Gitea(c) => c.releases(source).await,
            Client::GitLab(c) => c.releases(source).await,
            // Pas de releases sans forge : seulement des tags.
            Client::Git(_) => Ok(Vec::new()),
        }
    }

    pub async fn dernier_commit(
        &self,
        source: &SourceRef,
        branche: &str,
    ) -> Result<(String, String), ForgeError> {
        match self {
            Client::GitHub(c) => c.dernier_commit(source, branche).await,
            Client::Gitea(c) => c.dernier_commit(source, branche).await,
            Client::GitLab(c) => c.dernier_commit(source, branche).await,
            Client::Git(c) => c.dernier_commit(source, branche).await,
        }
    }

    pub async fn tags(&self, source: &SourceRef) -> Result<Vec<String>, ForgeError> {
        match self {
            Client::GitHub(c) => c.tags(source).await,
            Client::Gitea(c) => c.tags(source).await,
            Client::GitLab(c) => c.tags(source).await,
            Client::Git(c) => c.tags(source).await,
        }
    }

    /// Tous les tags qui commencent par `prefixe`.
    ///
    /// La liste generale s'arrete aux 50 premiers : Buzz en a des centaines,
    /// et ses `relay-v*` n'y figuraient pas. GitHub sait filtrer par prefixe ;
    /// ailleurs, on filtre ce que la liste generale donne.
    pub async fn tags_du_prefixe(
        &self,
        source: &SourceRef,
        prefixe: &str,
    ) -> Result<Vec<String>, ForgeError> {
        match self {
            Client::GitHub(c) => c.tags_du_prefixe(source, prefixe).await,
            _ => Ok(self
                .tags(source)
                .await?
                .into_iter()
                .filter(|t| t.starts_with(prefixe))
                .collect()),
        }
    }

    /// Les premiers octets d'un fichier, par une requete partielle.
    ///
    /// Assez pour lire la forme d'une archive sans la rapatrier. Un serveur
    /// qui ignore la plage renvoie le fichier entier : on le coupe alors.
    pub async fn debut(&self, url: &str, taille: usize) -> Option<Vec<u8>> {
        let r = crate::client_http()
            .ok()?
            .get(url)
            .header("Range", format!("bytes=0-{}", taille - 1))
            .send()
            .await
            .ok()?;
        if !r.status().is_success() {
            return None;
        }
        let mut octets = r.bytes().await.ok()?.to_vec();
        octets.truncate(taille);
        Some(octets)
    }

    /// Les derniers octets d'un fichier, sans le rapatrier : le repertoire
    /// central d'un zip y tient.
    ///
    /// Une plage explicite, d'apres la taille du fichier : le CDN de GitHub
    /// refuse `bytes=-N` (« 501 Unsupported client range »).
    pub async fn fin(&self, url: &str, total: u64, taille: usize) -> Option<Vec<u8>> {
        if total == 0 {
            return None;
        }
        let debut = total.saturating_sub(taille as u64);
        let r = crate::client_http()
            .ok()?
            .get(url)
            .header("Range", format!("bytes={debut}-{}", total - 1))
            .send()
            .await
            .ok()?;
        if !r.status().is_success() {
            return None;
        }
        let octets = r.bytes().await.ok()?.to_vec();
        // Un serveur qui ignore Range renvoie tout : on garde la fin.
        let debut = octets.len().saturating_sub(taille);
        Some(octets[debut..].to_vec())
    }

    /// Telecharge, en retentant sur une erreur passagere.
    ///
    /// Une coupure au milieu d'une archive faisait echouer toute l'analyse :
    /// sur un echantillon de 86 depots de la liste de souhaits, 13 tombaient
    /// ainsi, et tous passaient quand on les relancait seuls. Une connexion
    /// domestique, un serveur charge, un proxy capricieux produisent la meme
    /// chose. Deux nouvelles tentatives, espacees, suffisent a l'absorber ;
    /// une erreur qui dit quelque chose du depot — introuvable, quota — n'est
    /// pas retentee, elle se reproduirait a l'identique.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ForgeError> {
        let mut essai = 0;
        loop {
            let resultat = match self {
                Client::GitHub(c) => c.download(url).await,
                Client::Gitea(c) => c.download(url).await,
                Client::GitLab(c) => c.download(url).await,
                Client::Git(c) => c.download(url).await,
            };
            match resultat {
                Err(ForgeError::Http(e)) if essai < 2 && est_passagere(&e) => {
                    essai += 1;
                    tracing::warn!("{url} : {e} — nouvelle tentative ({essai}/2)");
                    tokio::time::sleep(std::time::Duration::from_secs(3 * essai)).await;
                }
                autre => return autre,
            }
        }
    }

    pub fn archive_url(&self, source: &SourceRef, reference: &str) -> String {
        match self {
            Client::GitHub(_) => crate::github::archive_url(source, reference),
            Client::Gitea(c) => crate::gitea::archive_url(c.hote(), source, reference),
            Client::GitLab(c) => crate::gitlab::archive_url(c.hote(), source, reference),
            Client::Git(c) => c.archive_url(source, reference),
        }
    }
}

/// Vrai pour une erreur de transport, qui a des chances de ne pas se repeter.
///
/// Un statut HTTP, lui, vient du serveur et dit quelque chose de la demande :
/// le retenter ne changerait rien.
fn est_passagere(e: &reqwest::Error) -> bool {
    e.status().is_none()
        && (e.is_timeout() || e.is_connect() || e.is_body() || e.is_decode() || e.is_request())
}

/// L'hote d'une URL, sans schema ni chemin.
pub fn hote_de(url: &str) -> String {
    url.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

/// Vrai si l'instance repond a l'API de Gitea.
///
/// `/api/v1/version` est le point le plus leger qu'exposent Gitea et Forgejo,
/// et il ne demande aucune authentification. Une reponse est un fait ; son
/// absence n'accuse personne, elle dit seulement qu'on ne sait pas.
async fn est_une_instance_gitea(hote: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .user_agent(concat!("yunopack/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(8))
        .build()
    else {
        return false;
    };
    let Ok(r) = client
        .get(format!("https://{hote}/api/v1/version"))
        .send()
        .await
    else {
        return false;
    };
    if !r.status().is_success() {
        return false;
    }
    r.text()
        .await
        .is_ok_and(|t| t.contains("\"version\"") && t.len() < 512)
}

/// Vrai si l'instance repond a l'API de GitLab pour ce projet.
///
/// Les points generiques de GitLab (`/api/v4/version`, `/api/v4/metadata`)
/// exigent un jeton ; interroger le projet lui-meme est le seul test qui
/// fonctionne sans authentification sur un projet public.
async fn est_une_instance_gitlab(hote: &str, source: &SourceRef) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .user_agent(concat!("yunopack/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(8))
        .build()
    else {
        return false;
    };
    let url = format!(
        "https://{hote}/api/v4/projects/{}",
        crate::gitlab::chemin_encode(source)
    );
    let Ok(r) = client.get(url).send().await else {
        return false;
    };
    r.status().is_success()
        && r.text()
            .await
            .is_ok_and(|t| t.contains("\"path_with_namespace\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_hote_se_tire_de_n_importe_quelle_forme_d_url() {
        for u in [
            "https://codeberg.org/a/b",
            "http://codeberg.org/a/b",
            "codeberg.org/a/b",
            "https://CODEBERG.org/a/b/tree/main",
        ] {
            assert_eq!(hote_de(u), "codeberg.org", "{u}");
        }
    }

    #[tokio::test]
    async fn chaque_forge_reconnue_donne_son_client() {
        let cas = [
            (Forge::GitHub, "https://github.com/a/b"),
            (Forge::GitLab, "https://framagit.org/a/b"),
            (Forge::Forgejo, "https://codeberg.org/a/b"),
            (Forge::Gitea, "https://gitea.com/a/b"),
        ];
        for (forge, url) in cas {
            let s = SourceRef {
                forge,
                owner: "a".into(),
                repo: "b".into(),
                url: url.into(),
                default_branch: None,
                commit: None,
            };
            // Aucun appel reseau : la forge est deja connue.
            assert!(Client::pour(&s).await.is_ok(), "{url}");
        }
    }
}
