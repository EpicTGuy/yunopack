//! Des adresses qui designent un projet sans etre celles de son depot.
//!
//! La liste de souhaits de YunoHost pointe parfois l'image Docker plutot que
//! le code : `hub.docker.com/r/johannesjo/super-productivity`. La description
//! de l'image cite son depot, trois fois dans ce cas.

use std::collections::HashMap;

/// Le depot que designe une adresse Docker Hub, d'apres la description de
/// l'image : le lien de forge le plus cite. `None` si l'adresse n'est pas une
/// image Docker Hub, ou si la description ne cite aucun depot.
pub async fn depot_d_une_image(url: &str) -> Option<String> {
    let chemin = url
        .split("hub.docker.com/")
        .nth(1)?
        .trim_start_matches("r/")
        .trim_start_matches("_/")
        .trim_end_matches('/');
    let mut morceaux = chemin.split('/');
    let (espace, nom) = match (morceaux.next(), morceaux.next()) {
        (Some(e), Some(n)) => (e, n),
        (Some(n), None) => ("library", n),
        _ => return None,
    };
    let api = format!("https://hub.docker.com/v2/repositories/{espace}/{nom}/");
    let corps: serde_json::Value = crate::client_http()
        .ok()?
        .get(&api)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let texte = format!(
        "{} {}",
        corps["description"].as_str().unwrap_or(""),
        corps["full_description"].as_str().unwrap_or("")
    );
    lien_le_plus_cite(&texte)
}

/// Le lien de depot (GitHub, GitLab, Codeberg) le plus frequent d'un texte.
fn lien_le_plus_cite(texte: &str) -> Option<String> {
    let motif = regex::Regex::new(
        r"https?://(?:github\.com|gitlab\.com|codeberg\.org)/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+",
    )
    .ok()?;
    let mut compte: HashMap<String, usize> = HashMap::new();
    for m in motif.find_iter(texte) {
        let lien = m
            .as_str()
            .trim_end_matches(".git")
            .trim_end_matches('.')
            .to_string();
        *compte.entry(lien).or_default() += 1;
    }
    compte
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(l, _)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_depot_le_plus_cite_par_la_description_l_emporte() {
        let t = "See https://github.com/johannesjo/super-productivity and \
                 https://github.com/johannesjo/super-productivity/issues, built with \
                 https://github.com/electron/electron. https://github.com/johannesjo/super-productivity.";
        assert_eq!(
            lien_le_plus_cite(t).as_deref(),
            Some("https://github.com/johannesjo/super-productivity")
        );
        assert_eq!(lien_le_plus_cite("rien ici"), None);
    }
}
