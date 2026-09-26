//! Proposer un correctif au depot d'un paquet, par une demande d'integration.
//!
//! Le correctif est ecrit ailleurs ; ce module s'occupe de le faire parvenir a
//! qui maintient le paquet. La sequence est celle de n'importe quelle
//! contribution : fourcher, pousser une branche, ouvrir la demande.
//!
//! Rien n'est fusionne : c'est au mainteneur de decider. Un outil qui
//! modifierait directement le paquet de quelqu'un d'autre serait indefendable,
//! meme quand il a raison.

use serde::Deserialize;

const API: &str = "https://api.github.com";
const UA: &str = concat!("yunopack/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum ContributionError {
    #[error(
        "aucun jeton : ouvrir une demande d'integration demande un jeton GitHub autorise a ecrire"
    )]
    SansJeton,
    #[error("le jeton n'a pas le droit d'ecrire : un jeton en lecture seule suffit pour analyser, pas pour contribuer")]
    LectureSeule,
    #[error("depot non reconnu : {0}")]
    Depot(String),
    #[error("GitHub a repondu {statut} : {corps}")]
    Refus { statut: u16, corps: String },
    #[error("reseau : {0}")]
    Http(#[from] reqwest::Error),
    #[error("git : {0}")]
    Git(String),
}

/// Ce qu'une contribution a produit.
#[derive(Debug, Clone)]
pub struct Contribution {
    /// URL de la demande d'integration ouverte.
    pub demande: String,
    /// Fourche utilisee, `proprietaire/depot`.
    pub fourche: String,
    pub branche: String,
}

pub struct Contributeur {
    client: reqwest::Client,
    jeton: String,
    /// Compte sous lequel la fourche est creee.
    compte: String,
}

impl Contributeur {
    /// Construit un contributeur a partir de l'environnement.
    ///
    /// Le jeton doit pouvoir ecrire. Celui qui sert a analyser peut etre en
    /// lecture seule : le distinguer evite de demander plus de droits que
    /// necessaire a qui ne veut que consulter.
    pub async fn depuis_environnement() -> Result<Self, ContributionError> {
        let jeton = std::env::var("GITHUB_TOKEN_ECRITURE")
            .or_else(|_| std::env::var("GITHUB_TOKEN"))
            .ok()
            .filter(|t| !t.is_empty())
            .ok_or(ContributionError::SansJeton)?;

        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        // Qui sommes-nous ? La reponse sert de nom de fourche, et son echec
        // dit tout de suite que le jeton ne vaut rien.
        let r = client
            .get(format!("{API}/user"))
            .bearer_auth(&jeton)
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(ContributionError::Refus {
                statut: r.status().as_u16(),
                corps: "jeton refuse".into(),
            });
        }
        #[derive(Deserialize)]
        struct Compte {
            login: String,
        }
        let compte: Compte = r.json().await?;

        Ok(Self {
            client,
            jeton,
            compte: compte.login,
        })
    }

    pub fn compte(&self) -> &str {
        &self.compte
    }

    /// Cree la fourche du depot, ou reprend celle qui existe deja.
    ///
    /// GitHub rend 202 a la creation : la fourche n'existe pas encore au
    /// moment ou il repond, d'ou l'attente qui suit.
    pub async fn fourcher(&self, depot: &str) -> Result<String, ContributionError> {
        let (proprietaire, nom) = couper(depot)?;
        let fourche = format!("{}/{nom}", self.compte);

        if self.depot_existe(&fourche).await? {
            return Ok(fourche);
        }

        let r = self
            .client
            .post(format!("{API}/repos/{proprietaire}/{nom}/forks"))
            .bearer_auth(&self.jeton)
            .header("content-length", "0")
            .send()
            .await?;
        match r.status().as_u16() {
            200 | 202 => {}
            403 => return Err(ContributionError::LectureSeule),
            s => {
                return Err(ContributionError::Refus {
                    statut: s,
                    corps: r
                        .text()
                        .await
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect(),
                })
            }
        }

        // La fourche met quelques secondes a exister. Pousser avant qu'elle
        // soit la echoue sans rien dire d'utile.
        for _ in 0..15 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if self.depot_existe(&fourche).await? {
                return Ok(fourche);
            }
        }
        Err(ContributionError::Refus {
            statut: 202,
            corps: "la fourche n'est pas apparue".into(),
        })
    }

    async fn depot_existe(&self, depot: &str) -> Result<bool, ContributionError> {
        let r = self
            .client
            .get(format!("{API}/repos/{depot}"))
            .bearer_auth(&self.jeton)
            .send()
            .await?;
        Ok(r.status().is_success())
    }

    /// Ouvre la demande d'integration du depot d'origine.
    pub async fn ouvrir_la_demande(
        &self,
        depot: &str,
        branche: &str,
        base: &str,
        titre: &str,
        corps: &str,
    ) -> Result<String, ContributionError> {
        let (proprietaire, nom) = couper(depot)?;
        let r = self
            .client
            .post(format!("{API}/repos/{proprietaire}/{nom}/pulls"))
            .bearer_auth(&self.jeton)
            .json(&serde_json::json!({
                "title": titre,
                "body": corps,
                "head": format!("{}:{branche}", self.compte),
                "base": base,
            }))
            .send()
            .await?;

        match r.status().as_u16() {
            200 | 201 => {
                #[derive(Deserialize)]
                struct Demande {
                    html_url: String,
                }
                let d: Demande = r.json().await?;
                Ok(d.html_url)
            }
            403 => Err(ContributionError::LectureSeule),
            s => Err(ContributionError::Refus {
                statut: s,
                corps: r
                    .text()
                    .await
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect(),
            }),
        }
    }

    /// URL de push portant le jeton, pour une machine sans cle SSH.
    pub fn url_push(&self, fourche: &str) -> String {
        format!(
            "https://{}:{}@github.com/{fourche}.git",
            self.compte, self.jeton
        )
    }
}

/// `proprietaire/depot` a partir d'une URL ou d'un chemin.
fn couper(depot: &str) -> Result<(String, String), ContributionError> {
    let sans = depot
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .take(2)
        .collect::<Vec<_>>();
    if sans.len() != 2 || sans.iter().any(|s| s.is_empty()) {
        return Err(ContributionError::Depot(depot.to_string()));
    }
    Ok((sans[1].to_string(), sans[0].to_string()))
}

/// Le corps de la demande : ce qui a change, et ce qui ne l'a pas ete.
///
/// Un mainteneur qui recoit un correctif d'un outil doit pouvoir juger sans
/// relire toute la diff : ce qui suit dit ce qui a ete fait, pourquoi, et ce
/// qui a ete laisse de cote faute de pouvoir en decider.
pub fn corps_de_la_demande(
    app: &str,
    niveau_actuel: Option<u8>,
    niveau_vise: u8,
    corriges: &[String],
    laisses: &[String],
) -> String {
    let mut c = String::new();
    c.push_str(&format!(
        "Ce correctif vient de [yunopack](https://github.com/EpicTGuy/yunopack), \
         qui a examine le paquet `{app}` et propose ce qu'il sait corriger \
         mecaniquement.\n\n"
    ));
    if let Some(n) = niveau_actuel {
        c.push_str(&format!("Niveau au catalogue : **{n}/8**"));
        if niveau_vise > n {
            c.push_str(&format!(
                ", **{niveau_vise}/8** une fois ces points corriges"
            ));
        }
        c.push_str(".\n\n");
    }
    c.push_str("### Ce qui a ete change\n\n");
    for id in corriges {
        c.push_str(&format!("- `{id}`\n"));
    }
    if !laisses.is_empty() {
        c.push_str(
            "\n### Ce qui n'a pas ete touche\n\n\
             Ces points demandent un jugement que l'outil ne prend pas a votre place :\n\n",
        );
        for id in laisses {
            c.push_str(&format!("- `{id}`\n"));
        }
    }
    c.push_str(
        "\n---\n\nRien n'a ete fusionne ni pousse ailleurs que sur cette branche. \
         Si ce correctif ne convient pas, fermer la demande suffit.\n",
    );
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_depot_se_coupe_sous_toutes_ses_formes() {
        for d in [
            "https://github.com/YunoHost-Apps/demo_ynh",
            "https://github.com/YunoHost-Apps/demo_ynh.git",
            "https://github.com/YunoHost-Apps/demo_ynh/",
            "YunoHost-Apps/demo_ynh",
        ] {
            assert_eq!(
                couper(d).unwrap(),
                ("YunoHost-Apps".to_string(), "demo_ynh".to_string()),
                "{d}"
            );
        }
    }

    #[test]
    fn un_depot_incomplet_est_refuse() {
        assert!(couper("demo").is_err());
        assert!(couper("").is_err());
        assert!(couper("/").is_err());
    }

    #[test]
    fn le_corps_dit_ce_qui_a_change_et_ce_qui_ne_l_a_pas_ete() {
        let c = corps_de_la_demande(
            "demo",
            Some(6),
            7,
            &["HLP001".into(), "MAJ001".into()],
            &["DOC002".into()],
        );
        assert!(c.contains("**6/8**"));
        assert!(c.contains("**7/8** une fois"));
        assert!(c.contains("`HLP001`"));
        assert!(c.contains("n'a pas ete touche"));
        assert!(c.contains("`DOC002`"));
        assert!(c.contains("fermer la demande suffit"));
    }

    #[test]
    fn sans_niveau_connu_le_corps_reste_lisible() {
        let c = corps_de_la_demande("demo", None, 7, &["HLP001".into()], &[]);
        assert!(!c.contains("Niveau au catalogue"));
        assert!(!c.contains("n'a pas ete touche"));
        assert!(c.contains("`HLP001`"));
    }

    #[test]
    fn un_niveau_deja_atteint_ne_promet_rien() {
        let c = corps_de_la_demande("demo", Some(8), 7, &["MAJ001".into()], &[]);
        assert!(c.contains("**8/8**"));
        assert!(!c.contains("une fois ces points"));
    }
}
