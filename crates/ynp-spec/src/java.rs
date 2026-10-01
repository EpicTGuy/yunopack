//! Java, que YunoHost ne provisionne pas lui-meme.
//!
//! Debian 12 ne fournit que Java 17. Les applications recentes en veulent
//! davantage : booklore et geopulse partent d'`eclipse-temurin:25`, netrunner
//! de Java 21. Le depot apt d'Adoptium, libre, fournit chaque version de
//! Temurin pour Debian 12, en amd64 comme en arm64 : le manifeste le declare
//! comme depot supplementaire de la resource apt.

use ynp_core::facts::{RepoFacts, Technology};
use ynp_core::spec::AptExtra;

/// La version majeure de Java qu'il faut, si l'application en a besoin.
pub fn version(facts: &RepoFacts) -> Option<u32> {
    let lance_java = facts
        .build
        .as_ref()
        .and_then(|b| b.start_command())
        .is_some_and(|c| {
            c.split_whitespace()
                .any(|m| m.rsplit('/').next() == Some("java"))
        });
    if facts.stack.primary != Technology::Java && !lance_java {
        return None;
    }
    let majeure = facts
        .stack
        .runtime_version
        .as_deref()
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v >= 8);
    Some(majeure.unwrap_or(17))
}

/// Il faut compiler : le JDK, pas seulement le JRE.
fn construit(facts: &RepoFacts) -> bool {
    facts.build.as_ref().is_some_and(|b| {
        b.build_steps.iter().any(|e| {
            e.contains("gradle") || e.contains("mvn") || e.contains("javac") || e.contains("sbt ")
        })
    })
}

/// Le paquet Debian, quand Java 17 suffit.
pub fn paquets_debian(facts: &RepoFacts) -> Vec<String> {
    match version(facts) {
        Some(v) if v <= 17 => vec![if construit(facts) {
            "openjdk-17-jdk-headless".to_string()
        } else {
            "openjdk-17-jre-headless".to_string()
        }],
        _ => Vec::new(),
    }
}

/// Le depot Adoptium, quand il faut plus que Java 17.
pub fn depot_temurin(facts: &RepoFacts) -> Option<AptExtra> {
    let v = version(facts).filter(|v| *v > 17)?;
    let sorte = if construit(facts) { "jdk" } else { "jre" };
    Some(AptExtra {
        nom: "temurin".into(),
        repo: "deb https://packages.adoptium.net/artifactory/deb bookworm main".into(),
        key: "https://packages.adoptium.net/artifactory/api/gpg/key/public".into(),
        packages: vec![format!("temurin-{v}-{sorte}")],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::BuildRecipe;

    fn faits(version: Option<&str>, etapes: &[&str]) -> RepoFacts {
        let mut f = RepoFacts::default();
        f.stack.primary = Technology::Java;
        f.stack.runtime_version = version.map(str::to_string);
        f.build = Some(BuildRecipe {
            build_steps: etapes.iter().map(|e| e.to_string()).collect(),
            ..Default::default()
        });
        f
    }

    #[test]
    fn java_17_vient_de_debian_au_dela_d_adoptium() {
        let f = faits(Some("17"), &[]);
        assert_eq!(paquets_debian(&f), vec!["openjdk-17-jre-headless"]);
        assert!(depot_temurin(&f).is_none());

        // Cas de booklore : eclipse-temurin:25, construit avec gradle.
        let f = faits(Some("25"), &["./gradlew bootJar"]);
        assert!(paquets_debian(&f).is_empty());
        assert_eq!(depot_temurin(&f).unwrap().packages, vec!["temurin-25-jdk"]);

        // Netrunner : 21.0, rien a compiler sur place.
        let f = faits(Some("21.0"), &[]);
        assert_eq!(depot_temurin(&f).unwrap().packages, vec!["temurin-21-jre"]);
    }

    #[test]
    fn sans_java_rien() {
        let mut f = faits(None, &[]);
        f.stack.primary = Technology::Go;
        assert_eq!(version(&f), None);
    }
}
