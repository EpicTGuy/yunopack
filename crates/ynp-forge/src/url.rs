//! Reconnaissance d'une URL de depot.
//!
//! L'utilisateur colle ce qu'il a sous la main : une URL de navigateur, une
//! adresse de clonage, avec ou sans `.git`, avec ou sans branche. Tout doit
//! mener au meme depot.

use ynp_core::facts::{Forge, SourceRef};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UrlError {
    #[error("URL de depot non reconnue : {0}")]
    Unrecognized(String),
    #[error("forge non prise en charge pour le moment : {0}")]
    UnsupportedForge(String),
}

/// Analyse une URL de depot et en tire proprietaire, nom et forge.
pub fn parse(input: &str) -> Result<SourceRef, UrlError> {
    let raw = input.trim();
    let cleaned = raw
        .trim_end_matches('/')
        .trim_start_matches("git@")
        .replace("https://", "")
        .replace("http://", "")
        .replace("ssh://", "")
        // Forme SSH `git@github.com:owner/repo`
        .replacen(".com:", ".com/", 1)
        .replacen(".org:", ".org/", 1);

    // Une page projet de Savannah designe un depot Git que cgit sert.
    if let Some(projet) = cleaned
        .strip_prefix("savannah.gnu.org/projects/")
        .or_else(|| cleaned.strip_prefix("savannah.nongnu.org/projects/"))
    {
        let projet = projet.split('/').next().unwrap_or(projet).to_string();
        if !projet.is_empty() {
            return Ok(SourceRef {
                forge: Forge::Git,
                url: format!("https://git.savannah.gnu.org/cgit/{projet}.git"),
                owner: "savannah".into(),
                repo: projet,
                default_branch: None,
                commit: None,
            });
        }
    }

    // Une page projet de sourcehut (`sr.ht/~u/projet`) designe le depot Git
    // du meme nom sur git.sr.ht.
    let cleaned = match cleaned.strip_prefix("sr.ht/") {
        Some(reste) => format!("git.sr.ht/{reste}"),
        None => cleaned,
    };
    let mut parts = cleaned.split('/').filter(|s| !s.is_empty());
    let host = parts
        .next()
        .ok_or_else(|| UrlError::Unrecognized(raw.into()))?
        .to_lowercase();
    // GitLab imbrique les groupes : `framasoft/framaspace/argos` est un seul
    // projet, pas un projet `framaspace` du groupe `framasoft`. Le separateur
    // `/-/` marque la fin du chemin et le debut de la navigation ; a defaut,
    // les mots-cles de chemin des autres forges jouent le meme role.
    let segments: Vec<&str> = parts
        .take_while(|s| {
            !matches!(
                *s,
                "-" | "tree" | "blob" | "src" | "commit" | "releases" | "archive"
            )
        })
        .collect();

    let (dernier, chemin) = segments
        .split_last()
        .ok_or_else(|| UrlError::Unrecognized(raw.into()))?;
    // Une page de groupe ou d'organisation, sans depot : le projet porte
    // souvent le nom du groupe (`foss.heptapod.net/tryton/` est
    // `tryton/tryton`). S'il n'existe pas, la forge le dira.
    let owner = if chemin.is_empty() {
        dernier.to_string()
    } else {
        chemin.join("/")
    };
    let repo = dernier.trim_end_matches(".git").to_string();

    if owner.is_empty() || repo.is_empty() {
        return Err(UrlError::Unrecognized(raw.into()));
    }

    let forge = match host.as_str() {
        "github.com" | "www.github.com" => Forge::GitHub,
        "gitlab.com" => Forge::GitLab,
        h if h.contains("codeberg") => Forge::Forgejo,
        h if h.contains("forgejo") => Forge::Forgejo,
        h if h.contains("gitea") => Forge::Gitea,
        // Instances GitLab, et Heptapod qui en est une variante.
        h if h.contains("gitlab") || h.contains("heptapod") => Forge::GitLab,
        // sourcehut n'a pas d'API publique sans jeton : le protocole Git
        // suffit (voir `git`).
        "git.sr.ht" | "sr.ht" | "bitbucket.org" => Forge::Git,
        // Instances GitLab connues de la liste de souhaits de YunoHost. Les
        // autres sont reconnues en interrogeant l'instance.
        "framagit.org" | "salsa.debian.org" | "0xacab.org" | "git.laquadrature.net" => {
            Forge::GitLab
        }
        // Une instance auto-hebergee ne se reconnait pas a son nom de domaine :
        // `git.exemple.fr` peut heberger n'importe quelle forge. La deviner
        // serait le contraire de ce que fait cet outil partout ailleurs. C'est
        // `fetch` qui interroge l'instance pour le savoir.
        _ => Forge::Inconnue,
    };

    Ok(SourceRef {
        forge,
        url: format!("https://{host}/{owner}/{repo}"),
        owner,
        repo,
        default_branch: None,
        commit: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toutes_les_formes_d_url_menent_au_meme_depot() {
        let attendu = ("gristlabs", "grist-core");
        for url in [
            "https://github.com/gristlabs/grist-core",
            "https://github.com/gristlabs/grist-core/",
            "https://github.com/gristlabs/grist-core.git",
            "http://github.com/gristlabs/grist-core",
            "github.com/gristlabs/grist-core",
            "git@github.com:gristlabs/grist-core.git",
            "https://github.com/gristlabs/grist-core/tree/main",
        ] {
            let s = parse(url).unwrap_or_else(|e| panic!("{url} : {e}"));
            assert_eq!((s.owner.as_str(), s.repo.as_str()), attendu, "pour {url}");
            assert_eq!(s.url, "https://github.com/gristlabs/grist-core");
        }
    }

    #[test]
    fn les_instances_gitlab_connues_sont_reconnues() {
        for u in [
            "https://gitlab.com/owner/repo",
            "https://framagit.org/owner/repo",
            "https://salsa.debian.org/owner/repo",
            "https://0xacab.org/owner/repo",
        ] {
            assert_eq!(parse(u).unwrap().forge, Forge::GitLab, "{u}");
        }
    }

    #[test]
    fn un_groupe_imbrique_reste_entier() {
        // Ne garder que deux segments designerait un autre projet, ou aucun.
        let s = parse("https://framagit.org/framasoft/framaspace/argos").unwrap();
        assert_eq!(s.owner, "framasoft/framaspace");
        assert_eq!(s.repo, "argos");
    }

    #[test]
    fn la_navigation_gitlab_est_coupee_au_separateur() {
        let s = parse("https://framagit.org/framasoft/framadate/-/tree/main/app").unwrap();
        assert_eq!(
            (s.owner.as_str(), s.repo.as_str()),
            ("framasoft", "framadate")
        );
    }

    #[test]
    fn un_hote_inconnu_reste_a_determiner() {
        // C'est `fetch` qui tranche, apres avoir interroge l'instance.
        assert_eq!(
            parse("https://git.exemple.org/owner/repo").unwrap().forge,
            Forge::Inconnue
        );
    }

    #[test]
    fn une_page_de_groupe_designe_le_projet_du_meme_nom() {
        let s = parse("https://foss.heptapod.net/tryton/").unwrap();
        assert_eq!(s.forge, Forge::GitLab);
        assert_eq!((s.owner.as_str(), s.repo.as_str()), ("tryton", "tryton"));
        assert_eq!(
            parse("https://gitlab.fabcloud.org/a/b").unwrap().forge,
            Forge::GitLab
        );
    }

    #[test]
    fn sourcehut_et_savannah_se_lisent_par_git() {
        let s = parse("https://git.sr.ht/~nicoco/slidge-debian/").unwrap();
        assert_eq!(s.forge, Forge::Git);
        assert_eq!(s.url, "https://git.sr.ht/~nicoco/slidge-debian");
        let s = parse("https://sr.ht/~mlb/linkhut/").unwrap();
        assert_eq!(s.url, "https://git.sr.ht/~mlb/linkhut");
        let s = parse("https://savannah.gnu.org/projects/mediagoblin").unwrap();
        assert_eq!(s.forge, Forge::Git);
        assert_eq!(s.url, "https://git.savannah.gnu.org/cgit/mediagoblin.git");
    }

    #[test]
    fn codeberg_et_les_instances_forgejo_sont_reconnues() {
        let s = parse("https://codeberg.org/forgejo/forgejo").unwrap();
        assert_eq!(s.forge, Forge::Forgejo);
        assert_eq!((s.owner.as_str(), s.repo.as_str()), ("forgejo", "forgejo"));
        assert_eq!(s.url, "https://codeberg.org/forgejo/forgejo");

        assert_eq!(
            parse("https://forgejo.ellis.link/a/b").unwrap().forge,
            Forge::Forgejo
        );
        // Une instance auto-hebergee sous `git.` : le pari le plus courant.
        // Une instance auto-hebergee reste inconnue jusqu'a ce qu'on
        // l'interroge : son nom de domaine n'apprend rien.
        assert_eq!(
            parse("https://git.exemple.fr/a/b").unwrap().forge,
            Forge::Inconnue
        );
        assert_eq!(parse("https://gitea.com/a/b").unwrap().forge, Forge::Gitea);
    }

    #[test]
    fn la_forme_ssh_de_codeberg_est_reconnue_elle_aussi() {
        let s = parse("git@codeberg.org:forgejo/forgejo.git").unwrap();
        assert_eq!((s.owner.as_str(), s.repo.as_str()), ("forgejo", "forgejo"));
    }

    #[test]
    fn une_url_incomplete_est_refusee() {
        // Une page d'organisation se tente comme le projet du meme nom ;
        // c'est la forge qui dira s'il existe.
        let s = parse("https://github.com/gristlabs").unwrap();
        assert_eq!(
            (s.owner.as_str(), s.repo.as_str()),
            ("gristlabs", "gristlabs")
        );
        assert!(parse("").is_err());
        assert!(parse("https://github.com").is_err());
    }
}
