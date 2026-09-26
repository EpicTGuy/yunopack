//! Traduction des constats du moteur d'analyse.
//!
//! Les regles rendent leur texte en francais, au plus pres de l'endroit ou
//! elles decident. Les traduire dans le code obligerait a y faire passer une
//! langue, et a tenir trois versions de chaque phrase au milieu de la logique.
//!
//! Elles sont donc traduites en donnee, par identifiant de regle. La severite
//! sert a distinguer les formes d'une meme regle : c'est ce qui les separe
//! deja, il n'y a pas a inventer un discriminant de plus.
//!
//! Le francais n'est pas dans le fichier : il vient de la regle. Une langue
//! sans traduction retombe donc sur lui, ce qui vaut mieux qu'une cle affichee
//! telle quelle.

use crate::finding::Finding;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

const TRADUCTIONS: &str = include_str!("../../../assets/i18n/regles.toml");

#[derive(Debug, Deserialize, Default)]
struct Textes {
    #[serde(default)]
    titre: Option<String>,
    #[serde(default)]
    remede: Option<String>,
}

type Table = BTreeMap<String, BTreeMap<String, Textes>>;

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| toml::from_str(TRADUCTIONS).expect("assets/i18n/regles.toml invalide"))
}

/// Le titre et le remede d'un constat, dans la langue demandee.
///
/// Rend le texte francais du constat pour `fr`, pour une langue inconnue, ou
/// quand la traduction manque.
pub fn traduire(f: &Finding, langue: &str) -> (String, Option<String>) {
    let francais = || (f.title.clone(), f.remediation.clone());
    if langue == "fr" {
        return francais();
    }
    let cle = format!("{}-{}", f.id, severite_slug(f));
    let Some(textes) = table().get(&cle).and_then(|l| l.get(langue)) else {
        return francais();
    };
    (
        textes
            .titre
            .as_ref()
            .map_or_else(|| f.title.clone(), |t| substituer(t, &f.args)),
        textes.remede.clone().or_else(|| f.remediation.clone()),
    )
}

/// `{nom}` prend la valeur que la regle a nommee.
///
/// Un marqueur sans valeur reste tel quel : l'effacer supprimerait une
/// information sans que personne s'en aperçoive.
fn substituer(gabarit: &str, args: &BTreeMap<String, String>) -> String {
    let mut out = gabarit.to_string();
    for (nom, valeur) in args {
        out = out.replace(&format!("{{{nom}}}"), valeur);
    }
    out
}

fn severite_slug(f: &Finding) -> &'static str {
    use crate::finding::Severity;
    match f.severity {
        Severity::Blocker => "blocker",
        Severity::Major => "major",
        Severity::Minor => "minor",
        Severity::Info => "info",
    }
}

/// Les langues pour lesquelles au moins une traduction existe.
pub fn langues() -> Vec<&'static str> {
    let mut v: Vec<&str> = table()
        .values()
        .flat_map(|l| l.keys().map(String::as_str))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::Severity;

    fn constat(id: &str, s: Severity, titre: &str) -> Finding {
        Finding::new(id, s, titre)
    }

    #[test]
    fn le_fichier_de_traduction_est_valide() {
        // Il est incorpore au binaire : une faute de frappe doit se voir ici,
        // pas au premier affichage chez quelqu'un.
        assert!(!table().is_empty());
        assert_eq!(langues(), vec!["en", "es"]);
    }

    #[test]
    fn le_francais_vient_de_la_regle_pas_du_fichier() {
        let f = constat("SRC001", Severity::Blocker, "Aucune archive telechargeable");
        assert_eq!(traduire(&f, "fr").0, "Aucune archive telechargeable");
    }

    #[test]
    fn une_langue_traduite_rend_son_texte() {
        let f = constat("SRC001", Severity::Blocker, "Aucune archive telechargeable")
            .remediation("texte francais");
        let (titre, remede) = traduire(&f, "en");
        assert_eq!(titre, "No downloadable archive");
        assert!(remede.unwrap().contains("stable archive"));
        assert_eq!(traduire(&f, "es").0, "Ningún archivo descargable");
    }

    #[test]
    fn la_severite_distingue_les_formes_d_une_meme_regle() {
        let bloquant = constat("LIC001", Severity::Blocker, "Licence non identifiable");
        let majeur = constat("LIC001", Severity::Major, "Licence « X » a verifier");
        assert_eq!(traduire(&bloquant, "en").0, "Licence cannot be identified");
        assert!(traduire(&majeur, "en").0.contains("to be checked"));
    }

    #[test]
    fn les_valeurs_nommees_reprennent_leur_place() {
        let f = constat("DB002", Severity::Blocker, "Service sans equivalent : x")
            .arg("services", "meilisearch, clickhouse");
        assert_eq!(
            traduire(&f, "en").0,
            "Service with no YunoHost equivalent: meilisearch, clickhouse"
        );
    }

    #[test]
    fn un_marqueur_sans_valeur_reste_visible() {
        // L'effacer supprimerait une information sans que personne le voie.
        let f = constat("DB002", Severity::Blocker, "x");
        assert!(traduire(&f, "en").0.contains("{services}"));
    }

    #[test]
    fn une_langue_inconnue_retombe_sur_le_francais() {
        let f = constat("SRC001", Severity::Blocker, "Aucune archive telechargeable");
        assert_eq!(traduire(&f, "de").0, "Aucune archive telechargeable");
        assert_eq!(traduire(&f, "").0, "Aucune archive telechargeable");
    }

    #[test]
    fn une_regle_sans_traduction_retombe_sur_le_francais() {
        let f = constat("XXX999", Severity::Major, "Regle inventee").remediation("a faire");
        let (titre, remede) = traduire(&f, "en");
        assert_eq!(titre, "Regle inventee");
        assert_eq!(remede.as_deref(), Some("a faire"));
    }

    #[test]
    fn un_constat_traduit_sans_remede_garde_celui_de_la_regle() {
        // `MAINT001-minor` n'a pas de remede traduit : celui de la regle vaut
        // mieux qu'aucun.
        let f = constat("MAINT001", Severity::Minor, "Depot sans activite recente")
            .remediation("remede francais");
        let (titre, remede) = traduire(&f, "en");
        assert_eq!(titre, "No recent activity in the repository");
        assert_eq!(remede.as_deref(), Some("remede francais"));
    }
}
