//! Restitution lisible des faits en terminal.
//!
//! Le rapport humain et le JSON portent la meme information : le premier sert a
//! decider, le second a enchainer. Aucun des deux n'est un resume de l'autre.

use ynp_core::facts::{Database, RepoFacts, Technology};

pub fn facts(f: &RepoFacts) -> String {
    let mut out = String::new();

    out.push_str(&format!("\n{}/{}\n", f.source.owner, f.source.repo));
    out.push_str(&format!("  {}\n\n", f.source.url));

    line(
        &mut out,
        "licence",
        f.meta
            .license_spdx
            .clone()
            .unwrap_or_else(|| "non identifiee".into()),
    );
    line(
        &mut out,
        "activite",
        match (&f.meta.pushed_at, f.meta.archived) {
            (_, true) => "depot ARCHIVE".to_string(),
            (Some(d), _) => format!("dernier push {}", &d[..10.min(d.len())]),
            (None, _) => "inconnue".into(),
        },
    );
    line(&mut out, "etoiles", f.meta.stars.to_string());

    out.push('\n');
    line(
        &mut out,
        "technologie",
        tech(f.stack.primary, f.stack.runtime_version.as_deref()),
    );
    if !f.stack.package_managers.is_empty() {
        line(
            &mut out,
            "gestionnaires",
            f.stack.package_managers.join(", "),
        );
    }
    if let Some(b) = &f.stack.build_script {
        line(&mut out, "build declare", b.clone());
    }
    line(
        &mut out,
        "base de donnees",
        database(f.services.database, f.services.database_evidence.as_deref()),
    );
    if f.services.needs_redis {
        line(&mut out, "cache", "redis".into());
    }

    out.push('\n');
    match &f.build {
        None => line(
            &mut out,
            "Dockerfile",
            "absent — la detection s'appuie sur les seuls fichiers de projet".into(),
        ),
        Some(b) => {
            line(&mut out, "Dockerfile", b.dockerfile_path.clone());
            line(
                &mut out,
                "  etapes",
                format!("{} ({} de build)", b.stages.len(), b.build_steps.len()),
            );
            if !b.apt_packages.is_empty() {
                line(&mut out, "  apt", liste(&b.apt_packages));
            }
            if !b.apk_packages.is_empty() {
                line(
                    &mut out,
                    "  apk",
                    format!("{} (a traduire vers deb)", liste(&b.apk_packages)),
                );
            }
            if !b.expose.is_empty() {
                line(
                    &mut out,
                    "  port",
                    b.expose
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if let Some(cmd) = b.start_command() {
                line(&mut out, "  demarrage", cmd);
            }
        }
    }

    if let Some(c) = &f.compose {
        let apps: Vec<&str> = c
            .services
            .iter()
            .filter(|s| s.is_app)
            .map(|s| s.name.as_str())
            .collect();
        out.push('\n');
        line(&mut out, "compose", c.path.clone());
        line(
            &mut out,
            "  service app",
            if apps.is_empty() {
                "aucun identifie".into()
            } else {
                apps.join(", ")
            },
        );
    }

    if !f.config.variables.is_empty() {
        out.push('\n');
        line(
            &mut out,
            "configuration",
            f.config
                .example_file
                .clone()
                .unwrap_or_else(|| "compose".into()),
        );
        // Le nombre brut peut etre enorme (335 pour linkwarden, une serie par
        // fournisseur OIDC). Ce qui compte est le nombre de variables dont le
        // role est reconnu : ce sont les seules a cabler sur YunoHost.
        let reconnues = f
            .config
            .variables
            .iter()
            .filter(|v| v.role != ynp_core::facts::ConfigRole::Other)
            .count();
        line(
            &mut out,
            "  variables",
            format!(
                "{} dont {reconnues} avec un role reconnu, {} secrets",
                f.config.variables.len(),
                f.config.variables.iter().filter(|v| v.secret).count()
            ),
        );
    }

    if !f.stack.native_deps.is_empty() {
        out.push('\n');
        line(
            &mut out,
            "compilation",
            format!("modules natifs : {}", liste(&f.stack.native_deps)),
        );
    }
    if !f.services.unsupported.is_empty() {
        line(
            &mut out,
            "sans equivalent",
            f.services.unsupported.join(", "),
        );
    }
    if f.services.requires_container_runtime {
        line(
            &mut out,
            "ATTENTION",
            "distribue uniquement sous forme d'image".into(),
        );
    }

    out
}

/// Une ligne du rapport : libelle aligne, puis valeur.
fn line(out: &mut String, label: &str, value: String) {
    out.push_str(&format!("  {label:<16}{value}\n"));
}

fn tech(t: Technology, version: Option<&str>) -> String {
    let name = match t {
        Technology::Php => "PHP",
        Technology::NodeJs => "Node.js",
        Technology::Python => "Python",
        Technology::Go => "Go",
        Technology::Ruby => "Ruby",
        Technology::Rust => "Rust",
        Technology::Java => "Java",
        Technology::Static => "fichiers statiques",
        Technology::Unknown => return "non identifiee".into(),
    };
    match version {
        Some(v) => format!("{name} {v}"),
        None => name.to_string(),
    }
}

fn database(db: Database, evidence: Option<&str>) -> String {
    let name = match db {
        Database::None => return "aucune".into(),
        Database::MySql => "MySQL/MariaDB",
        Database::PostgreSql => "PostgreSQL",
        Database::Sqlite => "SQLite (rien a provisionner)",
        Database::MongoDb => "MongoDB",
        Database::Unsupported => "non supportee",
    };
    match evidence {
        Some(e) => format!("{name}  [{e}]"),
        None => name.to_string(),
    }
}

/// Tronque une liste longue plutot que d'inonder le terminal.
fn liste(items: &[String]) -> String {
    const MAX: usize = 6;
    if items.len() <= MAX {
        return items.join(", ");
    }
    format!("{}, … (+{})", items[..MAX].join(", "), items.len() - MAX)
}

// --- Rapport de faisabilite ---

use ynp_core::finding::{Feasibility, Finding, Severity, Verdict};
use ynp_core::gate::GateReport;
use ynp_core::spec::Arbitrage;

pub fn feasibility(f: &Feasibility, gates: &GateReport) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "\n  {}   —   score {}/100\n",
        f.verdict.label(),
        f.score
    ));

    if f.findings.is_empty() {
        out.push_str("\n  Aucun constat.\n");
    }

    let mut severite_courante = None;
    for finding in &f.findings {
        if severite_courante != Some(finding.severity) {
            out.push_str(&format!("\n  ── {} ──\n", finding.severity.label()));
            severite_courante = Some(finding.severity);
        }
        out.push_str(&constat(finding));
    }

    out.push_str("\n  ── portes ──\n");
    for r in &gates.results {
        let etat = match &r.outcome {
            ynp_core::gate::GateOutcome::Pass => "ok".to_string(),
            ynp_core::gate::GateOutcome::Fail { findings } => {
                format!(
                    "ECHEC ({})",
                    findings
                        .iter()
                        .map(|f| f.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            ynp_core::gate::GateOutcome::Skipped { reason } => format!("sautee — {reason}"),
        };
        out.push_str(&format!(
            "  {:<6}{:<26}{etat}\n",
            r.gate.code(),
            r.gate.label()
        ));
    }

    // Le verdict seul ne dit pas quoi faire : on l'explicite.
    out.push_str(match f.verdict {
        Verdict::Feasible => "\n  Prochaine etape : yunopack plan\n",
        Verdict::FeasibleWithWork => {
            "\n  Packageable, mais l'appspec demandera des arbitrages.\n  \
             Prochaine etape : yunopack plan\n"
        }
        Verdict::NotFeasible => {
            "\n  Le pipeline s'arrete ici. Lever les blocages ci-dessus, ou renoncer.\n"
        }
    });

    out
}

fn constat(f: &Finding) -> String {
    let mut out = format!("\n  [{}] {}\n", f.id, f.title);

    if !f.detail.is_empty() {
        for ligne in enrouler(&f.detail, 74) {
            out.push_str(&format!("        {ligne}\n"));
        }
    }
    for e in &f.evidence {
        let ou = match (e.line, &e.excerpt) {
            (Some(l), Some(x)) => format!("{}:{l} — {x}", e.file),
            (None, Some(x)) => format!("{} — {x}", e.file),
            _ => e.file.clone(),
        };
        out.push_str(&format!("        preuve : {ou}\n"));
    }
    if let Some(r) = &f.remediation {
        for (i, ligne) in enrouler(r, 72).into_iter().enumerate() {
            out.push_str(&format!(
                "        {} {ligne}\n",
                if i == 0 { "→" } else { " " }
            ));
        }
    }
    out
}

/// Enroule un texte sans couper les mots : un rapport illisible en terminal
/// n'est pas lu.
fn enrouler(texte: &str, largeur: usize) -> Vec<String> {
    let mut lignes = Vec::new();
    let mut courante = String::new();

    for mot in texte.split_whitespace() {
        if !courante.is_empty() && courante.chars().count() + 1 + mot.chars().count() > largeur {
            lignes.push(std::mem::take(&mut courante));
        }
        if !courante.is_empty() {
            courante.push(' ');
        }
        courante.push_str(mot);
    }
    if !courante.is_empty() {
        lignes.push(courante);
    }
    lignes
}

/// Source retenue, et binaires preconstruits s'il y en a.
pub fn selection(s: &ynp_core::facts::SourceSelection) -> String {
    let mut out = String::new();
    out.push('\n');

    if s.evite_la_compilation() {
        line(
            &mut out,
            "source",
            format!("binaires publies ({})", s.reference),
        );
        for a in &s.prebuilt {
            out.push_str(&format!("    {:<12}{}\n", a.arch, a.name));
        }
        // C'est l'information qui compte pour une petite instance.
        line(&mut out, "", "rien a compiler sur la machine cible".into());
    } else {
        line(&mut out, "source", s.url.clone());
        line(&mut out, "sha256", s.sha256.clone());
    }
    line(&mut out, "autoupdate", s.strategy.clone());
    out
}

/// Resume de la specification, en insistant sur ce qui reste a completer.
pub fn spec(s: &ynp_core::AppSpec) -> String {
    let mut out = String::new();
    out.push('\n');

    line(&mut out, "identifiant", s.app.id.clone());
    line(&mut out, "nom", s.app.name.clone());
    line(&mut out, "version", s.app.version.to_string());
    line(&mut out, "licence", s.upstream.license.to_string());
    line(
        &mut out,
        "architectures",
        match &s.integration.architectures {
            ynp_core::spec::Architectures::All => "all".to_string(),
            ynp_core::spec::Architectures::Only(v) => v.join(", "),
        },
    );

    out.push('\n');
    line(&mut out, "technologie", s.runtime.technology.to_string());
    line(&mut out, "demarrage", s.runtime.execstart.to_string());
    if !s.runtime.build_steps.is_empty() {
        line(
            &mut out,
            "construction",
            format!("{} etape(s)", s.runtime.build_steps.len()),
        );
        for e in &s.runtime.build_steps {
            out.push_str(&format!("      · {e}\n"));
        }
    }

    out.push('\n');
    let mut briques: Vec<&str> = Vec::new();
    for (actif, nom) in [
        (s.features.nginx, "nginx"),
        (s.features.systemd, "systemd"),
        (s.features.phpfpm, "php-fpm"),
        (s.features.logrotate, "logrotate"),
        (s.features.change_url, "change_url"),
    ] {
        if actif {
            briques.push(nom);
        }
    }
    line(&mut out, "briques", briques.join(", "));
    if let Some(t) = s.resources.database.manifest_type() {
        line(&mut out, "base", t.to_string());
    }
    if !s.resources.apt_packages.is_empty() {
        line(
            &mut out,
            "dependances apt",
            s.resources.apt_packages.join(", "),
        );
    }

    let arbitrages = s.arbitrages();
    if arbitrages.is_empty() {
        out.push_str("\n  Specification complete : rien a completer.\n");
    } else {
        out.push_str(&format!(
            "\n  ── {} champ(s) a completer ──\n",
            arbitrages.len()
        ));
        out.push_str(&a_completer(&arbitrages));
    }
    out
}

/// Les arbitrages ouverts, avec leurs propositions numerotees.
///
/// Le numero n'est pas decoratif : `yunopack repondre <champ>=@2` le reprend,
/// ce qui evite de recopier une valeur a la main.
pub fn a_completer(arbitrages: &[Arbitrage]) -> String {
    let mut out = String::new();
    for a in arbitrages {
        out.push_str(&format!("\n  {}\n", a.champ));
        for l in enrouler(&a.raison, 72) {
            out.push_str(&format!("      {l}\n"));
        }
        if !a.ou_chercher.is_empty() {
            for l in enrouler(&format!("chercher dans : {}", a.ou_chercher.join(", ")), 72) {
                out.push_str(&format!("      {l}\n"));
            }
        }
        if !a.choix.is_empty() {
            out.push_str(&format!(
                "\n      valeurs acceptees : {}\n",
                a.choix.join(", ")
            ));
        }
        if !a.candidats.is_empty() {
            out.push_str("\n      propositions :\n");
            for (i, c) in a.candidats.iter().enumerate() {
                let valeur = c.value.replace('\n', " ⏎ ");
                out.push_str(&format!("      @{:<2} {valeur}\n", i + 1));
                out.push_str(&format!("          {}\n", c.why));
            }
        }
    }
    out
}

/// Resultat de la verification statique.
pub fn verification(racine: &std::path::Path, constats: &[Finding]) -> String {
    let mut out = format!("\n  {}\n", racine.display());

    if constats.is_empty() {
        out.push_str("\n  Paquet conforme : aucun constat.\n");
        return out;
    }

    let mut severite_courante = None;
    for f in constats {
        if severite_courante != Some(f.severity) {
            out.push_str(&format!("\n  ── {} ──\n", f.severity.label()));
            severite_courante = Some(f.severity);
        }
        out.push_str(&constat(f));
    }

    let bloquants = constats
        .iter()
        .filter(|f| f.severity == Severity::Blocker)
        .count();
    out.push_str(&if bloquants == 0 {
        "\n  Aucun blocage : le paquet peut etre teste.\n".to_string()
    } else {
        format!("\n  {bloquants} blocage(s) : le paquet ne doit pas etre installe en l'etat.\n")
    });
    out
}

/// Deroule d'une campagne de validation dynamique.
pub fn campagne(r: &ynp_runner::Rapport) -> String {
    let mut out = format!("\n  {} sur {}\n  {}\n\n", r.app, r.hote, r.url);

    for e in &r.etapes {
        let marque = if e.reussie { "ok" } else { "ECHEC" };
        out.push_str(&format!("  {:<28} {marque:<6} {}s\n", e.nom, e.duree_s));
        if !e.reussie && !e.detail.is_empty() {
            for ligne in e.detail.lines().take(12) {
                out.push_str(&format!("        {ligne}\n"));
            }
        }
    }

    out.push_str(match r.premiere_erreur() {
        None => {
            "\n  Gate G3 franchie : le paquet s'installe, fonctionne et se retire proprement.\n"
        }
        Some(_) => "\n  Gate G3 en echec.\n",
    });
    out
}

/// Resultat de l'examen d'un paquet deja publie.
pub fn examen(a: &ynp_verify::audit::Audit) -> String {
    let mut out = format!("\n  {}\n", if a.app.is_empty() { "paquet" } else { &a.app });
    match a.niveau_actuel {
        Some(n) => line(&mut out, "niveau actuel", format!("{n}/8")),
        None => line(&mut out, "niveau actuel", "inconnu".to_string()),
    }
    line(
        &mut out,
        "atteignable",
        format!("{}/8", a.niveau_atteignable()),
    );

    if a.rien_a_signaler() {
        out.push_str(
            "\n  Rien a signaler : ce paquet respecte tout ce que nous savons verifier.\n",
        );
        return out;
    }

    out.push_str(&format!("\n  ── {} point(s) ──\n", a.constats.len()));
    for c in &a.constats {
        let bloque = match c.bloque_le_niveau {
            Some(n) => format!(" — bloque le niveau {n}"),
            None => String::new(),
        };
        let repare = if c.reparable { " — corrigeable" } else { "" };
        out.push_str(&format!("\n  {}{bloque}{repare}\n", c.id));
        for l in enrouler(&c.quoi, 72) {
            out.push_str(&format!("      {l}\n"));
        }
        for l in enrouler(&c.remede, 72) {
            out.push_str(&format!("      {l}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_enroulement_ne_coupe_pas_les_mots() {
        let l = enrouler(
            "un texte assez long pour devoir etre reparti sur plusieurs lignes",
            20,
        );
        assert!(l.len() > 2);
        assert!(l.iter().all(|x| x.chars().count() <= 20), "{l:?}");
        assert_eq!(
            l.join(" "),
            "un texte assez long pour devoir etre reparti sur plusieurs lignes"
        );
    }

    #[test]
    fn un_mot_plus_long_que_la_largeur_n_est_pas_perdu() {
        let l = enrouler("court anticonstitutionnellement", 10);
        assert_eq!(l.join(" "), "court anticonstitutionnellement");
    }
}
