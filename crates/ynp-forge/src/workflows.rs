//! Ce que les workflows d'integration continue disent des versions.
//!
//! Un depot peut heberger plusieurs logiciels versionnes separement. Buzz y
//! range son application de bureau (tags `v*`, avec releases GitHub) et son
//! serveur (tags `relay-v*`, sans release). Seul le serveur interesse le
//! paquet, et rien dans les tags eux-memes ne le designe. Le workflow qui
//! construit l'image Docker, si : il se declenche sur la famille de tags du
//! serveur, puisque c'est lui qu'il publie.

use ynp_core::tree::RepoTree;

/// Prefixes des tags qui declenchent la construction d'une image Docker.
///
/// Seuls les motifs a prefixe litteral comptent (`relay-v*.*.*` donne
/// `relay-v`) ; `v*` ou `[0-9]*` designent la famille ordinaire.
pub fn prefixes_de_l_image(tree: &RepoTree) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for chemin in tree.paths() {
        if !chemin.starts_with(".github/workflows/") {
            continue;
        }
        let Some(texte) = tree.text(&chemin) else {
            continue;
        };
        if !construit_une_image(texte) {
            continue;
        }
        for motif in tags_declencheurs(texte) {
            if let Some(p) = prefixe_litteral(&motif) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn construit_une_image(texte: &str) -> bool {
    let t = texte.to_ascii_lowercase();
    t.contains("docker/build-push-action")
        || t.contains("docker buildx build")
        || t.contains("docker build ")
}

/// Motifs de `on.push.tags`.
fn tags_declencheurs(texte: &str) -> Vec<String> {
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(texte) else {
        return Vec::new();
    };
    // YAML 1.1 lit la cle `on` comme le booleen vrai.
    let on = doc
        .get("on")
        .or_else(|| doc.get(serde_yaml::Value::Bool(true)));
    let Some(tags) = on.and_then(|o| o.get("push")).and_then(|p| p.get("tags")) else {
        return Vec::new();
    };
    match tags {
        serde_yaml::Value::Sequence(s) => s
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        serde_yaml::Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// `relay-v*.*.*` → `relay-v` ; `v*` et `*` → rien.
fn prefixe_litteral(motif: &str) -> Option<String> {
    let fin = motif
        .find(|c: char| matches!(c, '*' | '?' | '[' | '+') || c.is_ascii_digit())
        .unwrap_or(motif.len());
    let p = &motif[..fin];
    let sans_v = p.strip_suffix('v').unwrap_or(p);
    (!sans_v.is_empty()).then(|| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_workflow_docker_de_buzz_designe_la_famille_du_relais() {
        let wf = r#"
name: Docker image
on:
  push:
    branches: [main]
    tags:
      - 'relay-v*.*.*'
  pull_request:
jobs:
  build:
    runs-on: ubuntu-24.04
    steps:
      - uses: docker/build-push-action@v6
"#;
        let release = r#"
on:
  push:
    tags:
      - 'v[0-9]*'
jobs:
  x:
    steps:
      - run: gh release create
"#;
        let tree = RepoTree::from_pairs([
            (".github/workflows/docker.yml", wf),
            (".github/workflows/release.yml", release),
        ]);
        assert_eq!(prefixes_de_l_image(&tree), vec!["relay-v".to_string()]);
    }

    #[test]
    fn la_famille_ordinaire_ne_donne_pas_de_prefixe() {
        assert_eq!(prefixe_litteral("v*"), None);
        assert_eq!(prefixe_litteral("*"), None);
        assert_eq!(prefixe_litteral("[0-9]+.*"), None);
        assert_eq!(prefixe_litteral("server-*"), Some("server-".into()));
    }
}
