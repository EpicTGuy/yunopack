//! Depot du paquet sur une forge Forgejo.
//!
//! La creation du depot passe par l'API et demande un jeton ; le push passe
//! par SSH et n'en demande pas. Les deux sont separes pour que l'absence de
//! jeton n'empeche pas de publier : on cree alors le depot a la main une fois,
//! et tout le reste fonctionne.

use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("git {commande} a echoue : {detail}")]
    Git { commande: String, detail: String },
    #[error("reseau vers la forge : {0}")]
    Http(#[from] reqwest::Error),
    #[error("la forge a repondu {code} : {corps}")]
    Refus { code: u16, corps: String },
}

/// Ou publier.
#[derive(Debug, Clone)]
pub struct Forge {
    /// Racine HTTP de l'instance, ex. `https://git.exemple.fr`.
    pub url: String,
    pub proprietaire: String,
    /// Alias SSH servant au push, declare dans `~/.ssh/config`.
    pub alias_ssh: String,
    /// Jeton d'API. Sans lui, la creation du depot est laissee a l'utilisateur.
    pub jeton: Option<String>,
}

impl Forge {
    /// Lit la configuration depuis l'environnement.
    ///
    /// Le jeton n'est jamais ecrit dans un fichier du projet : il vient de
    /// l'environnement ou il n'est pas la.
    pub fn depuis_environnement(alias_ssh: &str) -> Option<Self> {
        Some(Self {
            url: std::env::var("FORGEJO_URL")
                .ok()?
                .trim_end_matches('/')
                .to_string(),
            proprietaire: std::env::var("FORGEJO_OWNER").ok()?,
            alias_ssh: alias_ssh.to_string(),
            jeton: std::env::var("FORGEJO_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
        })
    }

    pub fn url_https(&self, depot: &str) -> String {
        format!("{}/{}/{depot}", self.url, self.proprietaire)
    }

    pub fn url_ssh(&self, depot: &str) -> String {
        format!("{}:{}/{depot}.git", self.alias_ssh, self.proprietaire)
    }

    /// URL de push portant le jeton, pour une machine sans cle SSH.
    ///
    /// Le service qui heberge l'interface web tourne sous un utilisateur
    /// systeme sans trousseau : lui en installer un pour publier serait lui
    /// donner un acces permanent a la forge. Le jeton, lui, se revoque et se
    /// limite a un depot.
    pub fn url_push_https(&self, depot: &str) -> Option<String> {
        let jeton = self.jeton.as_ref()?;
        let sans_schema = self.url.split_once("://")?.1;
        let schema = self.url.split_once("://")?.0;
        Some(format!(
            "{schema}://{}:{jeton}@{sans_schema}/{}/{depot}.git",
            self.proprietaire, self.proprietaire
        ))
    }

    /// Cree le depot s'il n'existe pas deja.
    ///
    /// Rend `false` quand il existait : republier n'est pas une erreur, c'est
    /// le cas courant.
    pub async fn creer_depot(&self, nom: &str, description: &str) -> Result<bool, ForgeError> {
        let Some(jeton) = &self.jeton else {
            return Ok(false);
        };

        let reponse = reqwest::Client::new()
            .post(format!("{}/api/v1/user/repos", self.url))
            .bearer_auth(jeton)
            .json(&json!({
                "name": nom,
                "description": description,
                "private": false,
                "auto_init": false,
            }))
            .send()
            .await?;

        match reponse.status().as_u16() {
            200 | 201 => Ok(true),
            // 409 : le depot existe deja, ce qui est le cas nominal d'une
            // republication.
            409 => Ok(false),
            code => Err(ForgeError::Refus {
                code,
                corps: reponse.text().await.unwrap_or_default(),
            }),
        }
    }
}

/// Prepare le depot local et l'envoie sur la forge.
///
/// Le depot est (re)cree a chaque publication plutot que mis a jour : le
/// paquet est integralement regenere, un historique local n'aurait aucun sens.
pub fn pousser(
    paquet: &std::path::Path,
    distant: &str,
    branche: &str,
    message: &str,
) -> Result<String, ForgeError> {
    let git = |args: &[&str]| -> Result<String, ForgeError> {
        // Les variables d'un git appelant (un hook de commit fixe
        // GIT_INDEX_FILE) designeraient son depot, pas celui du paquet.
        let sortie = std::process::Command::new("git")
            .args(args)
            .current_dir(paquet)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .output()
            .map_err(|e| ForgeError::Git {
                commande: args.join(" "),
                detail: e.to_string(),
            })?;

        if sortie.status.success() {
            Ok(String::from_utf8_lossy(&sortie.stdout).trim().to_string())
        } else {
            Err(ForgeError::Git {
                commande: args.join(" "),
                detail: String::from_utf8_lossy(&sortie.stderr).trim().to_string(),
            })
        }
    };

    if !paquet.join(".git").exists() {
        git(&["init", "-q", "-b", branche])?;
    }
    // L'identite est locale au depot : on ne touche pas a la configuration
    // globale de la machine.
    git(&["config", "user.name", "yunopack"])?;
    git(&["config", "user.email", "yunopack@localhost"])?;

    git(&["add", "-A"])?;
    // Un commit sans changement echoue ; ce n'est pas une erreur ici.
    let _ = git(&["commit", "-q", "-m", message]);

    let _ = git(&["remote", "remove", "origin"]);
    git(&["remote", "add", "origin", distant])?;
    let pousse = git(&["push", "-q", "--force", "origin", branche]);
    // Une URL de push peut porter un jeton ; le laisser dans .git/config le
    // rendrait lisible a quiconque recupere le paquet. On l'efface, que le
    // push ait reussi ou non.
    let _ = git(&["remote", "remove", "origin"]);
    pousse?;

    git(&["rev-parse", "HEAD"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forge_jeton(jeton: Option<&str>) -> Forge {
        Forge {
            url: "https://git.exemple.fr".into(),
            proprietaire: "utilisateur".into(),
            alias_ssh: "forgejo".into(),
            jeton: jeton.map(str::to_string),
        }
    }

    #[test]
    fn l_url_de_push_https_porte_le_jeton() {
        assert_eq!(
            forge_jeton(Some("abc123"))
                .url_push_https("app_ynh")
                .as_deref(),
            Some("https://utilisateur:abc123@git.exemple.fr/utilisateur/app_ynh.git")
        );
    }

    #[test]
    fn sans_jeton_il_n_y_a_pas_d_url_https() {
        // Pousser sans jeton demanderait un mot de passe interactif : mieux
        // vaut dire qu'on ne peut pas que bloquer sur une invite invisible.
        assert!(forge_jeton(None).url_push_https("app_ynh").is_none());
    }

    fn forge() -> Forge {
        Forge {
            url: "https://git.exemple.fr".into(),
            proprietaire: "utilisateur".into(),
            alias_ssh: "forgejo".into(),
            jeton: None,
        }
    }

    #[test]
    fn les_deux_formes_d_url_sont_construites_correctement() {
        let f = forge();
        assert_eq!(
            f.url_https("demo_ynh"),
            "https://git.exemple.fr/utilisateur/demo_ynh"
        );
        assert_eq!(f.url_ssh("demo_ynh"), "forgejo:utilisateur/demo_ynh.git");
    }

    #[test]
    fn une_barre_finale_dans_l_url_ne_se_duplique_pas() {
        std::env::set_var("FORGEJO_URL", "https://git.exemple.fr/");
        std::env::set_var("FORGEJO_OWNER", "utilisateur");
        let f = Forge::depuis_environnement("forgejo").unwrap();
        assert_eq!(f.url_https("x"), "https://git.exemple.fr/utilisateur/x");
        std::env::remove_var("FORGEJO_URL");
        std::env::remove_var("FORGEJO_OWNER");
    }

    #[tokio::test]
    async fn sans_jeton_la_creation_est_laissee_a_l_utilisateur() {
        // L'absence de jeton ne doit pas empecher de publier : le push passe
        // par SSH, seul le premier `create` demande l'API.
        assert!(!forge().creer_depot("demo_ynh", "").await.unwrap());
    }

    #[test]
    fn un_pousser_sans_depot_git_initialise_le_depot() {
        let paquet = std::env::temp_dir().join("yunopack-git-test");
        let _ = std::fs::remove_dir_all(&paquet);
        std::fs::create_dir_all(&paquet).unwrap();
        std::fs::write(paquet.join("manifest.toml"), "id = \"demo\"\n").unwrap();

        // Le push echouera faute de distant joignable, mais l'initialisation
        // et le commit doivent avoir eu lieu.
        let _ = pousser(&paquet, "/inexistant", "main", "test");
        assert!(paquet.join(".git").exists());
    }
}
