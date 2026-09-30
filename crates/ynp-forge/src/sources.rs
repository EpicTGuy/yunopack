//! Choix de la source a telecharger, et calcul de sa somme de controle.
//!
//! Le manifest exige un `sha256` : c'est a la fois un controle d'integrite et
//! une protection contre une archive amont modifiee apres coup. Le choix de la
//! reference determine aussi la strategie de mise a jour automatique que
//! l'infrastructure YunoHost appliquera ensuite.

use sha2::{Digest, Sha256};
use ynp_core::facts::{Forge, Release, SourceRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChoice {
    /// Tag, branche ou commit a archiver.
    pub reference: String,
    pub url: String,
    /// Valeur de `autoupdate.strategy` dans le manifest.
    pub strategy: String,
    /// Version amont, sans le `v` initial. Absente pour une strategie par commit,
    /// ou la version devient la date.
    pub version: Option<String>,
    pub kind: SourceKind,
    /// Valeur de `autoupdate.version_regex` quand la version est prefixee
    /// (`rt-5.0.5`, `relay-v0.2.1`) : sans elle, l'autoupdater de YunoHost
    /// ne reconnait aucune version dans ces tags.
    pub version_regex: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Release publiee : le cas le plus confortable, seul a fournir un lien de
    /// changelog dans les propositions de mise a jour.
    Release,
    Tag,
    /// Ni release ni tag : la version sera la date du commit. Packageable, mais
    /// sans point de repere pour l'utilisateur.
    Commit,
}

/// Choisit la reference a packager.
///
/// L'ordre de preference suit la qualite du repere offert a l'administrateur
/// qui verra arriver les mises a jour.
pub fn choose(source: &SourceRef, releases: &[Release], tags: &[String]) -> SourceChoice {
    choose_avec(source, releases, tags, crate::github::archive_url)
}

/// Meme choix, avec la fabrique d'URL d'archive de la forge concernee.
///
/// Chaque forge a sa propre forme d'URL d'archive ; la passer en parametre
/// evite que ce module connaisse les trois, alors que sa seule affaire est
/// l'ordre de preference entre release, tag et commit.
pub fn choose_avec(
    source: &SourceRef,
    releases: &[Release],
    tags: &[String],
    archive_url: impl Fn(&SourceRef, &str) -> String,
) -> SourceChoice {
    let forge = source.forge.autoupdate_slug();

    if let Some(r) = releases
        .iter()
        .find(|r| !r.prerelease && is_version_like(&r.tag))
    {
        return SourceChoice {
            url: archive_url(source, &r.tag),
            version: Some(strip_v(&r.tag)),
            reference: r.tag.clone(),
            strategy: format!("latest_{forge}_release"),
            kind: SourceKind::Release,
            version_regex: None,
        };
    }

    if let Some(tag) = latest_version_tag(tags) {
        return SourceChoice {
            url: archive_url(source, &tag),
            version: Some(strip_v(&tag)),
            reference: tag,
            strategy: format!("latest_{forge}_tag"),
            kind: SourceKind::Tag,
            version_regex: None,
        };
    }

    // Des versions publiees sous un prefixe : `rt-5.0.5` chez Request
    // Tracker, `RackTables-0.22.0`. Sans cette regle, l'outil packageait le
    // dernier commit de la branche de developpement, faute de tag reconnu.
    if let Some(cle) = famille_dominante(tags) {
        if let Some(c) = choisir_dans_la_famille(source, tags, &cle, &archive_url) {
            return c;
        }
    }

    let reference = source
        .default_branch
        .clone()
        .unwrap_or_else(|| "main".into());
    SourceChoice {
        url: archive_url(source, &reference),
        reference,
        strategy: format!("latest_{forge}_commit"),
        version: None,
        kind: SourceKind::Commit,
        version_regex: None,
    }
}

/// Famille d'un tag et sa version : `relay-v0.2.1` → (`relay-v`, `0.2.1`),
/// `server_release/1.4/1.4.7` → (`server_release/#.#/`, `1.4.7`). Les
/// nombres du prefixe deviennent `#`, pour regrouper les branches d'une
/// meme famille. Un tag de version ordinaire n'a pas de famille.
pub fn famille(tag: &str) -> Option<(String, String)> {
    const SEPARATEURS: &[char] = &['-', '_', '/', '@'];
    for (i, _) in tag.char_indices() {
        let (prefixe, reste) = tag.split_at(i);
        if prefixe.is_empty() || !reste.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let avant_v = prefixe.strip_suffix('v').unwrap_or(prefixe);
        if !avant_v.ends_with(SEPARATEURS) {
            continue;
        }
        if !is_version_like(reste) || !reste.contains('.') {
            continue;
        }
        // Une famille de pre-versions n'est pas une source stable.
        // Mot a mot : `src-` ou `arcade-` ne sont pas des pre-versions.
        let l = prefixe.to_lowercase();
        if l.split(|c: char| !c.is_ascii_alphabetic()).any(|m| {
            matches!(
                m,
                "rc" | "beta"
                    | "alpha"
                    | "pre"
                    | "preview"
                    | "nightly"
                    | "snapshot"
                    | "dev"
                    | "test"
            )
        }) {
            return None;
        }
        let mut cle = String::new();
        let mut dans_un_nombre = false;
        for c in prefixe.chars() {
            if c.is_ascii_digit() {
                if !dans_un_nombre {
                    cle.push('#');
                }
                dans_un_nombre = true;
            } else {
                cle.push(c);
                dans_un_nombre = false;
            }
        }
        return Some((cle, reste.to_string()));
    }
    None
}

/// La famille qui porte clairement les versions : au moins deux tags, et
/// quatre sur cinq des tags des familles.
///
/// Plusieurs familles de tailles voisines designent plusieurs logiciels.
/// LessPass versionne ainsi son extension, son site, son application mobile
/// et sa ligne de commande : prendre la plus nombreuse donnait la version
/// mobile. Sans autre indice, on ne devine pas.
fn famille_dominante(tags: &[String]) -> Option<String> {
    let mut compte: std::collections::BTreeMap<String, usize> = Default::default();
    for t in tags {
        if let Some((cle, _)) = famille(t) {
            *compte.entry(cle).or_default() += 1;
        }
    }
    // Un tag isole n'est pas une famille : il ne compte pas dans le total.
    let total: usize = compte.values().filter(|n| **n >= 2).sum();
    compte
        .into_iter()
        .filter(|(_, n)| *n >= 2 && *n * 5 >= total * 4)
        .max_by_key(|(_, n)| *n)
        .map(|(cle, _)| cle)
}

/// L'expression que l'autoupdater de YunoHost applique au tag : elle doit
/// capturer la version seule.
pub fn version_regex(cle: &str) -> String {
    let mut out = String::from("^");
    for c in cle.chars() {
        match c {
            '#' => out.push_str("[0-9]+"),
            c if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '@') => out.push(c),
            c => {
                out.push('\\');
                out.push(c);
            }
        }
    }
    out.push_str("(.*)$");
    out
}

/// La version la plus recente d'une famille de tags.
pub fn choisir_dans_la_famille(
    source: &SourceRef,
    tags: &[String],
    cle: &str,
    archive_url: impl Fn(&SourceRef, &str) -> String,
) -> Option<SourceChoice> {
    let (tag, version) = tags
        .iter()
        .filter_map(|t| {
            famille(t)
                .filter(|(c, _)| c == cle)
                .map(|(_, v)| (t.clone(), v))
        })
        .max_by(|a, b| version_key(&a.1).cmp(&version_key(&b.1)))?;
    let forge = source.forge.autoupdate_slug();
    Some(SourceChoice {
        url: archive_url(source, &tag),
        version: Some(version),
        reference: tag,
        strategy: format!("latest_{forge}_tag"),
        kind: SourceKind::Tag,
        version_regex: Some(version_regex(cle)),
    })
}

/// Un tag exploitable ressemble a `x.y.z` ou `vx.y.z`, sans marqueur de
/// pre-version. Meme regle que l'autoupdater de YunoHost, pour que nos choix
/// et les siens ne divergent pas au premier cycle de mise a jour.
pub fn is_version_like(tag: &str) -> bool {
    const PRE: &[&str] = &[
        "rc", "beta", "alpha", "pre", "nightly", "snapshot", "dev", "test",
    ];
    let lower = tag.to_lowercase();
    if PRE.iter().any(|p| lower.contains(p)) {
        return false;
    }
    let core = lower.trim_start_matches('v');
    let mut parts = core.split('.');
    let first = parts.next().unwrap_or_default();
    !first.is_empty()
        && first.chars().all(|c| c.is_ascii_digit())
        && core.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn latest_version_tag(tags: &[String]) -> Option<String> {
    tags.iter()
        .filter(|t| is_version_like(t))
        .max_by(|a, b| version_key(a).cmp(&version_key(b)))
        .cloned()
}

/// Cle de tri numerique : `v1.10.0` doit passer apres `v1.9.0`, ce qu'un tri
/// lexicographique ferait a l'envers.
fn version_key(tag: &str) -> Vec<u64> {
    tag.trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

fn strip_v(tag: &str) -> String {
    tag.trim_start_matches('v').to_string()
}

/// Somme de controle de l'archive, telle qu'elle figurera dans le manifest.
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Vrai si la forge sait appliquer une strategie de mise a jour automatique.
pub fn supports_autoupdate(forge: Forge) -> bool {
    matches!(
        forge,
        Forge::GitHub | Forge::GitLab | Forge::Gitea | Forge::Forgejo
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceRef {
        SourceRef {
            forge: Forge::GitHub,
            owner: "acme".into(),
            repo: "widget".into(),
            url: "https://github.com/acme/widget".into(),
            default_branch: Some("main".into()),
            commit: None,
        }
    }

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag: tag.into(),
            prerelease,
            ..Default::default()
        }
    }

    #[test]
    fn une_famille_prefixee_se_reconnait_et_se_suit() {
        assert_eq!(famille("rt-5.0.5"), Some(("rt-".into(), "5.0.5".into())));
        assert_eq!(
            famille("relay-v0.2.1"),
            Some(("relay-v".into(), "0.2.1".into()))
        );
        assert_eq!(
            famille("server_release/1.4/1.4.7"),
            Some(("server_release/#.#/".into(), "1.4.7".into()))
        );
        assert_eq!(
            famille("RackTables-0.22.0"),
            Some(("RackTables-".into(), "0.22.0".into()))
        );
        assert_eq!(famille("v1.2.3"), None);
        assert_eq!(famille("nightly-1.2.3"), None);
        assert_eq!(famille("src-1.2.3"), Some(("src-".into(), "1.2.3".into())));
        assert_eq!(version_regex("relay-v"), "^relay-v(.*)$");
        assert_eq!(
            version_regex("server_release/#.#/"),
            "^server_release/[0-9]+\\.[0-9]+/(.*)$"
        );
    }

    #[test]
    fn sans_tag_ordinaire_la_famille_prefixee_evite_la_branche_de_developpement() {
        // Cas de Request Tracker : 51 versions « rt-x.y.z », aucune release.
        let tags: Vec<String> = ["rt-4.4.7", "rt-5.0.5", "rt-5.0.10", "tmp-1.0"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = choose(&source(), &[], &tags);
        assert_eq!(c.reference, "rt-5.0.10");
        assert_eq!(c.version.as_deref(), Some("5.0.10"));
        assert_eq!(c.version_regex.as_deref(), Some("^rt-(.*)$"));
        assert_eq!(c.strategy, "latest_github_tag");
    }

    #[test]
    fn plusieurs_familles_voisines_ne_se_departagent_pas_au_nombre() {
        // Cas de LessPass : extension, site, mobile, ligne de commande.
        let tags: Vec<String> = [
            "mobile-v9.8.7",
            "mobile-v9.8.6",
            "mobile-v9.8.5",
            "cli-v9.1.0",
            "cli-v9.0.9",
            "web-extension-v9.2.0",
            "web-extension-v9.1.9",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(choose(&source(), &[], &tags).kind, SourceKind::Commit);
    }

    #[test]
    fn une_release_stable_est_preferee_a_tout_le_reste() {
        let c = choose(&source(), &[release("v1.2.3", false)], &["v9.9.9".into()]);

        assert_eq!(c.kind, SourceKind::Release);
        assert_eq!(c.reference, "v1.2.3");
        assert_eq!(c.version.as_deref(), Some("1.2.3"));
        assert_eq!(c.strategy, "latest_github_release");
        assert!(c.url.ends_with("/archive/v1.2.3.tar.gz"));
    }

    #[test]
    fn une_preversion_n_est_pas_retenue() {
        let releases = [release("v2.0.0-rc1", true), release("v1.9.0", false)];
        let c = choose(&source(), &releases, &[]);
        assert_eq!(c.reference, "v1.9.0");
    }

    #[test]
    fn a_defaut_de_release_le_tag_de_version_le_plus_recent_gagne() {
        let tags = ["v1.9.0".into(), "v1.10.0".into(), "v1.2.0".into()];
        let c = choose(&source(), &[], &tags);

        assert_eq!(c.kind, SourceKind::Tag);
        // Un tri lexicographique aurait choisi v1.9.0.
        assert_eq!(c.reference, "v1.10.0");
        assert_eq!(c.strategy, "latest_github_tag");
    }

    #[test]
    fn sans_release_ni_tag_on_retombe_sur_la_branche_par_defaut() {
        let c = choose(&source(), &[], &[]);

        assert_eq!(c.kind, SourceKind::Commit);
        assert_eq!(c.reference, "main");
        assert_eq!(c.strategy, "latest_github_commit");
        // La version deviendra la date du commit : il n'y en a pas a annoncer.
        assert_eq!(c.version, None);
    }

    #[test]
    fn les_tags_fantaisistes_sont_ecartes() {
        assert!(is_version_like("v1.2.3"));
        assert!(is_version_like("1.2"));
        assert!(!is_version_like("v2.0.0-beta1"));
        assert!(!is_version_like("nightly"));
        assert!(!is_version_like("release-2024"));
        assert!(!is_version_like("latest"));
    }

    #[test]
    fn la_somme_de_controle_est_celle_du_manifest() {
        // Valeur de reference de sha256sum sur une chaine vide.
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(sha256(b"yunopack").len(), 64);
    }
}
