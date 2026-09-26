//! Constats produits par le moteur de regles et par la verification.
//!
//! Un [`Finding`] est toujours actionnable : il dit ce qui ne va pas, sur quelle
//! preuve, et ce qu'il faut faire. Un rapport sans remediation est un rapport
//! qui fait perdre du temps a celui qui le lit.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Rend le packaging impossible. Un seul suffit a rendre le verdict negatif.
    Blocker,
    /// Packageable, mais au prix d'un travail manuel significatif.
    Major,
    /// A corriger, sans remettre en cause la faisabilite.
    Minor,
    /// Information utile pour remplir l'AppSpec, pas un defaut.
    Info,
}

impl Severity {
    /// Penalite appliquee au score de faisabilite (0-100).
    pub fn penalty(self) -> u32 {
        match self {
            Severity::Blocker => 100,
            Severity::Major => 20,
            Severity::Minor => 5,
            Severity::Info => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Severity::Blocker => "BLOQUANT",
            Severity::Major => "MAJEUR",
            Severity::Minor => "MINEUR",
            Severity::Info => "INFO",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// D'ou vient le constat, pour que l'utilisateur puisse verifier lui-meme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

impl Evidence {
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            file: path.into(),
            line: None,
            excerpt: None,
        }
    }

    pub fn at(path: impl Into<String>, line: u32, excerpt: impl Into<String>) -> Self {
        Self {
            file: path.into(),
            line: Some(line),
            excerpt: Some(excerpt.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Identifiant stable, ex. `LIC001`. Documente dans docs/30-REGLES-FAISABILITE.md.
    pub id: String,
    pub severity: Severity,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Ce qu'il faut faire. Obligatoire des que la severite depasse Info.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    /// Valeurs que le titre insere, nommees.
    ///
    /// Le titre est rendu en francais par la regle elle-meme ; une traduction
    /// a besoin des memes valeurs pour construire sa propre phrase, et ne peut
    /// pas les retrouver dans le texte deja forme.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub args: std::collections::BTreeMap<String, String>,
}

impl Finding {
    pub fn new(id: impl Into<String>, severity: Severity, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            severity,
            title: title.into(),
            detail: String::new(),
            remediation: None,
            evidence: Vec::new(),
            args: std::collections::BTreeMap::new(),
        }
    }

    /// Nomme une valeur que le titre insere, pour qu'une traduction puisse la
    /// replacer dans sa propre phrase.
    pub fn arg(mut self, nom: &str, valeur: impl Into<String>) -> Self {
        self.args.insert(nom.to_string(), valeur.into());
        self
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }

    pub fn remediation(mut self, r: impl Into<String>) -> Self {
        self.remediation = Some(r.into());
        self
    }

    pub fn evidence(mut self, e: Evidence) -> Self {
        self.evidence.push(e);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Aucun blocage, peu de travail manuel attendu.
    Feasible,
    /// Packageable, mais l'AppSpec demandera des arbitrages humains.
    FeasibleWithWork,
    /// Au moins un blocage. Le pipeline s'arrete a la gate G1.
    NotFeasible,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Feasible => "FAISABLE",
            Verdict::FeasibleWithWork => "FAISABLE AVEC TRAVAIL",
            Verdict::NotFeasible => "NON FAISABLE",
        }
    }
}

/// Sortie de `yunopack assess`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feasibility {
    pub verdict: Verdict,
    /// 0-100. Purement indicatif : c'est l'absence de bloquant qui decide.
    pub score: u8,
    pub findings: Vec<Finding>,
}

impl Feasibility {
    /// Calcule verdict et score a partir des constats.
    ///
    /// Le seuil distingue « faisable » de « faisable avec travail » ; il ne
    /// transforme jamais un bloquant en simple avertissement.
    pub fn from_findings(mut findings: Vec<Finding>, threshold: u8) -> Self {
        findings.sort_by(|a, b| a.severity.cmp(&b.severity).then_with(|| a.id.cmp(&b.id)));

        let has_blocker = findings.iter().any(|f| f.severity == Severity::Blocker);
        let penalty: u32 = findings.iter().map(|f| f.severity.penalty()).sum();
        let score = 100u32.saturating_sub(penalty).min(100) as u8;

        let verdict = if has_blocker {
            Verdict::NotFeasible
        } else if score >= threshold {
            Verdict::Feasible
        } else {
            Verdict::FeasibleWithWork
        };

        Self {
            verdict,
            score,
            findings,
        }
    }

    pub fn blockers(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Blocker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, s: Severity) -> Finding {
        Finding::new(id, s, "t")
    }

    #[test]
    fn un_seul_bloquant_suffit_a_refuser() {
        let r =
            Feasibility::from_findings(vec![f("A", Severity::Info), f("B", Severity::Blocker)], 60);
        assert_eq!(r.verdict, Verdict::NotFeasible);
        assert_eq!(r.blockers().count(), 1);
    }

    #[test]
    fn les_majeurs_degradent_sans_refuser() {
        let r =
            Feasibility::from_findings(vec![f("A", Severity::Major), f("B", Severity::Major)], 60);
        assert_eq!(r.score, 60);
        assert_eq!(r.verdict, Verdict::Feasible);

        let r = Feasibility::from_findings(vec![f("A", Severity::Major); 3], 60);
        assert_eq!(r.score, 40);
        assert_eq!(r.verdict, Verdict::FeasibleWithWork);
    }

    #[test]
    fn un_depot_sans_constat_est_faisable() {
        let r = Feasibility::from_findings(vec![], 60);
        assert_eq!(r.score, 100);
        assert_eq!(r.verdict, Verdict::Feasible);
    }

    #[test]
    fn les_constats_sortent_tries_du_plus_grave_au_moins_grave() {
        let r = Feasibility::from_findings(
            vec![
                f("Z", Severity::Info),
                f("A", Severity::Blocker),
                f("M", Severity::Minor),
            ],
            60,
        );
        let ids: Vec<_> = r.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "M", "Z"]);
    }
}
