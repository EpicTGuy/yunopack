//! Regles de faisabilite.
//!
//! C'est ici que se joue la promesse du projet : **refuser explicitement plutot
//! que produire un paquet devine**. Un refus argumente est un livrable, pas un
//! echec — il epargne a l'utilisateur des heures passees sur une application
//! qui ne pouvait pas aboutir.
//!
//! Chaque regle porte un identifiant stable, une severite, une preuve et une
//! remediation. Une regle sans remediation actionnable n'a pas sa place dans le
//! catalogue : un rapport qu'on ne peut pas suivre fait perdre du temps.

pub mod gates;
pub(crate) mod licence;
pub(crate) mod runtime;
pub(crate) mod sources;

use ynp_core::facts::RepoFacts;
use ynp_core::finding::{Feasibility, Finding};

/// Un constat a poser sur un depot.
pub trait Rule {
    /// Identifiant stable, documente dans docs/30-REGLES-FAISABILITE.md.
    fn id(&self) -> &'static str;
    /// Rend un constat, ou rien si la regle ne s'applique pas.
    fn check(&self, facts: &RepoFacts) -> Option<Finding>;
}

/// Toutes les regles, dans l'ordre du catalogue.
pub fn catalog() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(licence::NonLibre),
        Box::new(licence::Abandonne),
        Box::new(sources::AucuneSource),
        Box::new(sources::PasDeReperePourLesMisesAJour),
        Box::new(runtime::ConteneurRequis),
        Box::new(runtime::KubernetesSeulement),
        Box::new(runtime::BaseNonSupportee),
        Box::new(runtime::LangageNonProvisionnable),
        Box::new(runtime::PythonHorsBookworm),
        Box::new(runtime::BuildGourmand),
        Box::new(runtime::RecetteIntrouvable),
        Box::new(runtime::PortPrivilegie),
        Box::new(runtime::StackInconnue),
        Box::new(runtime::BaseDetectee),
        Box::new(runtime::PaquetsAlpineATraduire),
        Box::new(runtime::ModulesNatifs),
    ]
}

/// Applique le catalogue et rend le verdict.
pub fn assess(facts: &RepoFacts, threshold: u8) -> Feasibility {
    let findings = catalog().iter().filter_map(|r| r.check(facts)).collect();
    Feasibility::from_findings(findings, threshold)
}

/// Date de reference pour les regles qui dependent du temps.
///
/// Passee explicitement plutot que lue de l'horloge : un test qui depend de
/// l'heure courante devient faux tout seul au bout de quelques mois.
pub fn today() -> String {
    // Sans dependance a une bibliotheque de dates : la seule regle concernee
    // compare un seuil de deux ans, ou l'heure exacte est sans objet.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| from_epoch_days((d.as_secs() / 86_400) as i64))
        .unwrap_or_else(|_| "1970-01-01".into())
}

/// Inverse de la formule de Howard Hinnant utilisee dans `ynp_analyze::health`.
fn from_epoch_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::finding::{Severity, Verdict};

    /// Un depot sain : rien ne doit le bloquer.
    pub(crate) fn depot_sain() -> RepoFacts {
        use ynp_core::facts::*;
        RepoFacts {
            schema_version: 1,
            source: SourceRef {
                forge: Forge::GitHub,
                owner: "acme".into(),
                repo: "widget".into(),
                url: "https://github.com/acme/widget".into(),
                default_branch: Some("main".into()),
                commit: None,
            },
            meta: RepoMeta {
                license_spdx: Some("AGPL-3.0".into()),
                pushed_at: Some("2026-09-01".into()),
                ..Default::default()
            },
            releases: vec![Release {
                tag: "v1.2.3".into(),
                ..Default::default()
            }],
            stack: StackFacts {
                primary: Technology::NodeJs,
                runtime_version: Some("20".into()),
                ..Default::default()
            },
            build: Some(BuildRecipe {
                dockerfile_path: "Dockerfile".into(),
                expose: vec![3000],
                // Une recette de construction reelle : sans elle, BUILD002 se
                // declenche a juste titre.
                build_steps: vec!["npm ci --omit=dev".into()],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn un_depot_sain_est_declare_faisable() {
        let r = assess(&depot_sain(), 60);
        assert_eq!(r.verdict, Verdict::Feasible, "constats : {:?}", r.findings);
        assert_eq!(r.blockers().count(), 0);
    }

    #[test]
    fn chaque_regle_du_catalogue_a_un_identifiant_unique() {
        let cat = catalog();
        let mut ids: Vec<&str> = cat.iter().map(|r| r.id()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total, "identifiants en double dans le catalogue");
    }

    #[test]
    fn toute_regle_au_dela_d_informative_propose_une_remediation() {
        // Un constat qu'on ne peut pas suivre fait perdre du temps a son lecteur.
        use ynp_core::facts::*;
        let mut facts = depot_sain();
        // Depot volontairement problematique, pour declencher un maximum de regles.
        facts.meta.license_spdx = None;
        facts.meta.archived = true;
        facts.releases.clear();
        facts.stack = StackFacts::default();
        facts.services = ServiceFacts {
            unsupported: vec!["elasticsearch".into()],
            requires_container_runtime: true,
            ..Default::default()
        };

        for f in assess(&facts, 60).findings {
            if f.severity != Severity::Info {
                assert!(
                    f.remediation.is_some(),
                    "la regle {} ne dit pas quoi faire",
                    f.id
                );
            }
        }
    }

    #[test]
    fn la_date_du_jour_est_bien_formee() {
        let d = today();
        assert_eq!(d.len(), 10, "{d}");
        assert_eq!(d.matches('-').count(), 2);
        assert!(d.starts_with("20"));
    }

    #[test]
    fn la_conversion_de_jours_en_date_est_juste() {
        assert_eq!(from_epoch_days(0), "1970-01-01");
        assert_eq!(from_epoch_days(19_723), "2024-01-01");
        // 2024 est bissextile : le 60e jour de l'annee est le 29 fevrier.
        assert_eq!(from_epoch_days(19_723 + 59), "2024-02-29");
    }
}
