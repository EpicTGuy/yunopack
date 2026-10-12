//! Harnais d'evaluation : notre paquet face au paquet officiel.
//!
//! Il existe plus de sept cents paquets YunoHost ecrits a la main. Les
//! comparer a ce que produit yunopack, champ par champ, est la seule mesure
//! honnete du progres des detecteurs.
//!
//! Un ecart n'est pas forcement une erreur : le paquet officiel peut avoir
//! fait un autre arbitrage, tout aussi defendable. Le harnais ne juge donc
//! pas, il rapproche — et c'est en lisant les ecarts qu'on decide lesquels
//! sont des bugs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Corpus {
    pub app: Vec<Entree>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Entree {
    pub nom: String,
    pub amont: String,
    pub paquet: String,
    #[serde(default)]
    pub difficulte: String,
}

/// Comparaison d'un champ entre les deux paquets.
#[derive(Debug, Clone, Serialize)]
pub struct Ecart {
    pub champ: String,
    pub nous: String,
    pub officiel: String,
    pub accord: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Resultat {
    pub app: String,
    pub difficulte: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erreur: Option<String>,
    pub ecarts: Vec<Ecart>,
}

impl Resultat {
    pub fn accords(&self) -> usize {
        self.ecarts.iter().filter(|e| e.accord).count()
    }

    pub fn total(&self) -> usize {
        self.ecarts.len()
    }
}

/// Champs compares, avec la facon d'en extraire la valeur d'un manifest.
///
/// On compare ce qui a un sens fonctionnel. Le nom affiche ou la description,
/// qui relevent du gout, n'y figurent pas : un ecart n'y voudrait rien dire.
type Extracteur = fn(&toml::Value) -> String;

pub fn champs() -> Vec<(&'static str, Extracteur)> {
    vec![
        ("id", |m| texte(m, "id")),
        // La famille de la licence : `AGPL-3.0`, forme SPDX retiree, vaut
        // `AGPL-3.0-only`, et le choix entre `-only` et `-or-later` ne se lit
        // pas toujours dans le depot — les paquets du catalogue eux-memes
        // tranchent au hasard.
        ("licence", |m| {
            let l = chemin(m, &["upstream", "license"]);
            let l = l.trim();
            l.strip_suffix("-only")
                .or_else(|| l.strip_suffix("-or-later"))
                .unwrap_or(l)
                .to_string()
        }),
        ("architectures", |m| {
            liste_ou_texte(m, &["integration", "architectures"])
        }),
        ("multi_instance", |m| {
            chemin(m, &["integration", "multi_instance"])
        }),
        ("base de donnees", |m| {
            chemin(m, &["resources", "database", "type"])
        }),
        ("binaires par arch", |m| {
            let s = m
                .get("resources")
                .and_then(|r| r.get("sources"))
                .and_then(|s| s.get("main"));
            let multi = s.is_some_and(|s| s.get("amd64").is_some() || s.get("arm64").is_some());
            multi.to_string()
        }),
        ("extraction", |m| {
            let s = m
                .get("resources")
                .and_then(|r| r.get("sources"))
                .and_then(|s| s.get("main"));
            // `extract` absent vaut vrai : c'est le defaut de YunoHost.
            s.and_then(|s| s.get("extract"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
                .to_string()
        }),
        ("strategie de mise a jour", |m| {
            m.get("resources")
                .and_then(|r| r.get("sources"))
                .and_then(|s| s.get("main"))
                .and_then(|s| s.get("autoupdate"))
                .and_then(|a| a.get("strategy"))
                .map(valeur)
                .unwrap_or_else(|| "-".into())
        }),
        ("utilisateur systeme", |m| {
            existe(m, &["resources", "system_user"])
        }),
        ("repertoire de donnees", |m| {
            existe(m, &["resources", "data_dir"])
        }),
        ("port reserve", |m| existe(m, &["resources", "ports"])),
    ]
}

fn texte(m: &toml::Value, cle: &str) -> String {
    m.get(cle).map(valeur).unwrap_or_else(|| "-".into())
}

fn chemin(m: &toml::Value, cles: &[&str]) -> String {
    let mut courant = m;
    for c in cles {
        match courant.get(c) {
            Some(v) => courant = v,
            None => return "-".into(),
        }
    }
    valeur(courant)
}

/// `architectures` accepte une chaine ou une liste : on normalise pour comparer.
fn liste_ou_texte(m: &toml::Value, cles: &[&str]) -> String {
    let brut = chemin(m, cles);
    if brut == "-" {
        return brut;
    }
    let mut parties: Vec<String> = brut
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    parties.sort();
    parties.join(",")
}

fn existe(m: &toml::Value, cles: &[&str]) -> String {
    (chemin(m, cles) != "-").to_string()
}

fn valeur(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Array(a) => a.iter().map(valeur).collect::<Vec<_>>().join(","),
        toml::Value::Table(_) => "(table)".into(),
        autre => autre.to_string(),
    }
}

/// Rapproche deux manifests.
pub fn comparer(nous: &toml::Value, officiel: &toml::Value) -> Vec<Ecart> {
    champs()
        .into_iter()
        .map(|(nom, extraire)| {
            let a = extraire(nous);
            let b = extraire(officiel);
            Ecart {
                champ: nom.to_string(),
                accord: a == b,
                nous: a,
                officiel: b,
            }
        })
        .collect()
}

/// Matrice de precision, par champ et pour l'ensemble.
pub fn matrice(resultats: &[Resultat]) -> String {
    let mut out = String::from("\n  Precision par champ\n\n");

    let noms: Vec<String> = champs().iter().map(|(n, _)| n.to_string()).collect();
    for nom in &noms {
        let concernes: Vec<&Ecart> = resultats
            .iter()
            .flat_map(|r| r.ecarts.iter())
            .filter(|e| &e.champ == nom)
            .collect();
        if concernes.is_empty() {
            continue;
        }
        let bons = concernes.iter().filter(|e| e.accord).count();
        let total = concernes.len();
        let barre = "█".repeat(bons * 10 / total.max(1));
        out.push_str(&format!("  {nom:<26} {bons}/{total}  {barre}\n"));
    }

    let bons: usize = resultats.iter().map(Resultat::accords).sum();
    let total: usize = resultats.iter().map(Resultat::total).sum();
    let echecs = resultats.iter().filter(|r| r.erreur.is_some()).count();

    out.push_str(&format!(
        "\n  Ensemble : {bons}/{total} champs en accord ({:.0}%)\n",
        if total == 0 {
            0.0
        } else {
            bons as f64 * 100.0 / total as f64
        }
    ));
    if echecs > 0 {
        out.push_str(&format!(
            "  {echecs} application(s) n'ont pas pu etre evaluees\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(src: &str) -> toml::Value {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn deux_manifests_identiques_sont_en_accord_partout() {
        let m = manifest(
            r#"id = "demo"
[upstream]
license = "MIT"
[integration]
architectures = "all"
multi_instance = false
[resources.system_user]
"#,
        );
        let ecarts = comparer(&m, &m);
        assert!(ecarts.iter().all(|e| e.accord), "{ecarts:#?}");
    }

    #[test]
    fn l_ordre_des_architectures_ne_cree_pas_de_faux_ecart() {
        let a = manifest("[integration]\narchitectures = [\"amd64\", \"arm64\"]\n");
        let b = manifest("[integration]\narchitectures = [\"arm64\", \"amd64\"]\n");
        let e = comparer(&a, &b);
        let arch = e.iter().find(|x| x.champ == "architectures").unwrap();
        assert!(arch.accord, "{arch:?}");
    }

    #[test]
    fn extract_absent_vaut_vrai_comme_chez_yunohost() {
        // Comparer « absent » a « true » produirait un ecart qui n'existe pas.
        let implicite = manifest("[resources.sources.main]\nurl = \"x\"\n");
        let explicite = manifest("[resources.sources.main]\nurl = \"x\"\nextract = true\n");
        let e = comparer(&implicite, &explicite);
        assert!(e.iter().find(|x| x.champ == "extraction").unwrap().accord);
    }

    #[test]
    fn un_veritable_ecart_est_rapporte_avec_les_deux_valeurs() {
        let a = manifest("[resources.database]\ntype = \"postgresql\"\n");
        let b = manifest("[resources.database]\ntype = \"mysql\"\n");
        let e = comparer(&a, &b);
        let base = e.iter().find(|x| x.champ == "base de donnees").unwrap();

        assert!(!base.accord);
        assert_eq!(base.nous, "postgresql");
        assert_eq!(base.officiel, "mysql");
    }

    #[test]
    fn une_ressource_absente_se_distingue_d_une_ressource_presente() {
        let avec = manifest("[resources.data_dir]\n");
        let sans = manifest("[resources.install_dir]\n");
        let e = comparer(&avec, &sans);
        let d = e
            .iter()
            .find(|x| x.champ == "repertoire de donnees")
            .unwrap();
        assert_eq!((d.nous.as_str(), d.officiel.as_str()), ("true", "false"));
    }

    #[test]
    fn la_matrice_resume_l_ensemble_des_resultats() {
        let r = vec![Resultat {
            app: "demo".into(),
            difficulte: String::new(),
            erreur: None,
            ecarts: vec![
                Ecart {
                    champ: "id".into(),
                    nous: "a".into(),
                    officiel: "a".into(),
                    accord: true,
                },
                Ecart {
                    champ: "licence".into(),
                    nous: "MIT".into(),
                    officiel: "GPL".into(),
                    accord: false,
                },
            ],
        }];
        let m = matrice(&r);
        assert!(m.contains("1/2"), "{m}");
        assert!(m.contains("50%"), "{m}");
    }
}
