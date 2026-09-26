//! Regles legales et de vitalite : ce qui se verifie sans rien telecharger.
//!
//! Elles sont evaluees en premier parce qu'elles sont les moins couteuses.
//! Refuser pour une question de licence avant de depenser trente minutes de
//! CPU est le principal interet d'un pipeline a portes successives.

use crate::Rule;
use ynp_analyze::health::{self, MaintenanceRisk};
use ynp_core::facts::RepoFacts;
use ynp_core::finding::{Evidence, Finding, Severity};

/// Identifiants SPDX de licences libres les plus repandues.
///
/// Liste volontairement non exhaustive : une licence absente produit un constat
/// « a verifier a la main », pas un refus definitif. Refuser une licence libre
/// par meconnaissance serait pire que demander une verification.
const LIBRES: &[&str] = &[
    "AGPL-3.0",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "GPL-2.0",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "LGPL-2.1",
    "LGPL-3.0",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MIT",
    "MIT-0",
    "ISC",
    "Apache-2.0",
    "MPL-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSD-4-Clause",
    "Unlicense",
    "CC0-1.0",
    "WTFPL",
    "Zlib",
    "EUPL-1.2",
    "artistic-2.0",
];

pub struct NonLibre;

impl Rule for NonLibre {
    fn id(&self) -> &'static str {
        "LIC001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        match facts.meta.license_spdx.as_deref() {
            Some(spdx) if LIBRES.iter().any(|l| l.eq_ignore_ascii_case(spdx)) => None,

            Some(spdx) => Some(
                Finding::new(
                    self.id(),
                    Severity::Major,
                    format!("Licence « {spdx} » a verifier"),
                )
                .arg("licence", spdx)
                .detail(
                    "Cette licence n'est pas dans la liste des licences libres courantes. \
                         Le catalogue YunoHost accepte au cas par cas des licences ethiques qui \
                         ne sont pas strictement libres.",
                )
                .remediation(
                    "Verifier la licence a la main, et la politique du catalogue \
                         (docs/yunohost/90-policy.md) si une publication officielle est visee.",
                )
                .evidence(Evidence::file("LICENSE")),
            ),

            None => Some(
                Finding::new(self.id(), Severity::Blocker, "Licence non identifiable")
                    .detail(
                        "La forge ne rend aucun identifiant SPDX. Sans licence connue, ni le \
                         champ `upstream.license` du manifest ni l'admissibilite au catalogue \
                         ne peuvent etre remplis.",
                    )
                    .remediation(
                        "Ouvrir le fichier LICENSE du depot. S'il porte bien une licence libre \
                         que la forge n'a pas su reconnaitre, renseigner `upstream.license` \
                         a la main dans appspec.toml.",
                    ),
            ),
        }
    }
}

pub struct Abandonne;

impl Rule for Abandonne {
    fn id(&self) -> &'static str {
        "MAINT001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        let today = crate::today();
        match health::assess(&facts.meta, &today) {
            MaintenanceRisk::Healthy | MaintenanceRisk::Unknown => None,

            MaintenanceRisk::Archived => Some(
                Finding::new(self.id(), Severity::Major, "Depot archive par son auteur")
                    .detail(
                        "Un depot archive ne recevra plus de correctif, y compris de securite. \
                         Le paquet deviendrait une dette pour qui l'installe.",
                    )
                    .remediation("Chercher une fourche maintenue avant de packager ce depot."),
            ),

            MaintenanceRisk::Stale => Some(
                Finding::new(self.id(), Severity::Minor, "Depot sans activite recente")
                    .detail(format!(
                        "Aucun commit depuis plus de {} mois. Le niveau 8 du catalogue exige \
                         une maintenance effective.",
                        health::INACTIVITY_DAYS / 30
                    ))
                    .detail_evidence(facts.meta.pushed_at.clone()),
            ),
        }
    }
}

/// Petit confort d'ecriture : attacher la date du dernier push comme preuve.
trait WithPushDate {
    fn detail_evidence(self, pushed_at: Option<String>) -> Finding;
}

impl WithPushDate for Finding {
    fn detail_evidence(mut self, pushed_at: Option<String>) -> Finding {
        if let Some(d) = pushed_at {
            self.evidence.push(Evidence {
                file: "(metadonnees de la forge)".into(),
                line: None,
                excerpt: Some(format!("dernier push : {d}")),
            });
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::depot_sain;

    #[test]
    fn une_licence_libre_ne_declenche_rien() {
        for spdx in ["AGPL-3.0", "MIT", "Apache-2.0", "mit"] {
            let mut f = depot_sain();
            f.meta.license_spdx = Some(spdx.into());
            assert!(NonLibre.check(&f).is_none(), "{spdx} devrait passer");
        }
    }

    #[test]
    fn une_licence_absente_bloque() {
        let mut f = depot_sain();
        f.meta.license_spdx = None;
        let finding = NonLibre.check(&f).unwrap();

        assert_eq!(finding.severity, Severity::Blocker);
        assert!(finding.remediation.is_some());
    }

    #[test]
    fn une_licence_inconnue_demande_une_verification_sans_refuser() {
        // Refuser une licence libre par meconnaissance serait pire que
        // demander une verification.
        let mut f = depot_sain();
        f.meta.license_spdx = Some("BSL-1.1".into());
        let finding = NonLibre.check(&f).unwrap();

        assert_eq!(finding.severity, Severity::Major);
        assert!(finding.title.contains("BSL-1.1"));
    }

    #[test]
    fn un_depot_archive_est_signale_sans_etre_refuse() {
        // Une fourche maintenue peut exister : le refus serait premature.
        let mut f = depot_sain();
        f.meta.archived = true;
        let finding = Abandonne.check(&f).unwrap();

        assert_eq!(finding.severity, Severity::Major);
        assert!(finding.remediation.as_deref().unwrap().contains("fourche"));
    }

    #[test]
    fn un_depot_actif_ne_declenche_pas_la_regle_d_abandon() {
        assert!(Abandonne.check(&depot_sain()).is_none());
    }

    #[test]
    fn une_date_de_push_inconnue_ne_conclut_rien() {
        let mut f = depot_sain();
        f.meta.pushed_at = None;
        assert!(Abandonne.check(&f).is_none());
    }
}
