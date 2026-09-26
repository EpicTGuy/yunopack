//! Deroule le pipeline pour un travail, en rendant compte a chaque etape.
//!
//! La logique metier n'est pas dupliquee ici : ce module orchestre les memes
//! crates que le CLI. Une divergence entre l'interface web et la ligne de
//! commande serait une source d'incomprehension sans contrepartie.
//!
//! Le pipeline est coupe en deux au point ou une decision humaine peut etre
//! necessaire. La premiere moitie va de l'URL a la specification ; la seconde
//! rend et verifie le paquet. Entre les deux, le travail attend — et ce qu'il
//! attend est ecrit dans `appspec.toml`, si bien qu'une reponse donnee par le
//! formulaire et une reponse donnee par `yunopack repondre` aboutissent
//! exactement au meme fichier.

use crate::travaux::Registre;
use std::path::{Path, PathBuf};

/// Executee dans une tache de fond ; ne rend rien, tout passe par le registre.
pub async fn executer(registre: Registre, id: String, url: String, racine: PathBuf) {
    if let Err(e) = tenter(&registre, &id, &url, &racine).await {
        registre.conclure(&id, false, &enchainement(e.as_ref()));
    }
}

/// Reprend un travail suspendu, une fois sa specification completee.
pub async fn reprendre(registre: Registre, id: String, travail: PathBuf) {
    if let Err(e) = finir(&registre, &id, &travail) {
        registre.conclure(&id, false, &enchainement(e.as_ref()));
    }
}

/// Le message d'une erreur, suivi de ses causes.
///
/// La cause profonde porte souvent l'information utile (quota de la forge,
/// depot prive) : la perdre obligerait a relancer pour rien.
fn enchainement(e: &dyn std::error::Error) -> String {
    let mut message = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        message.push_str(&format!(" — {cause}"));
        source = cause.source();
    }
    message
}

/// Emplacement de la specification d'un travail. Un seul endroit la nomme.
pub fn chemin_appspec(travail: &Path) -> PathBuf {
    travail.join("appspec.toml")
}

async fn tenter(registre: &Registre, id: &str, url: &str, racine: &Path) -> anyhow::Result<()> {
    let travail = racine.join(id);
    std::fs::create_dir_all(&travail)?;

    registre.avancer(id, "analyse", "recuperation du depot…");
    let recupere = ynp_forge::fetch(url).await?;
    let mut faits = ynp_analyze::analyze(recupere.forge, &recupere.tree);
    faits.selection = Some(recupere.selection.clone());
    std::fs::write(
        travail.join("facts.json"),
        serde_json::to_string_pretty(&faits)?,
    )?;

    let techno = faits.stack.primary.to_string();
    registre.avancer(
        id,
        "analyse",
        &format!(
            "{} fichiers · {techno} · source {}",
            recupere.tree.len(),
            recupere.choice.reference
        ),
    );

    registre.avancer(id, "faisabilite", "application des regles…");
    let (faisabilite, portes) =
        ynp_rules::gates::evaluate(&faits, ynp_core::DEFAULT_FEASIBILITY_THRESHOLD);
    std::fs::write(
        travail.join("report.json"),
        serde_json::to_string_pretty(&faisabilite)?,
    )?;

    if !portes.all_passed() {
        let raisons: Vec<String> = faisabilite
            .blockers()
            .map(|f| format!("{} : {}", f.id, f.title))
            .collect();
        registre.conclure(
            id,
            false,
            &format!("non faisable — {}", raisons.join(" ; ")),
        );
        return Ok(());
    }
    registre.avancer(
        id,
        "faisabilite",
        &format!(
            "{} — score {}/100",
            faisabilite.verdict.label(),
            faisabilite.score
        ),
    );

    registre.avancer(id, "specification", "arbitrages…");
    let spec = ynp_spec::build(&faits)?;
    std::fs::write(chemin_appspec(&travail), toml::to_string_pretty(&spec)?)?;

    let mut arbitrages = spec.arbitrages();
    // Chaque question pointe vers les fichiers du depot ou la reponse a des
    // chances de se trouver. Sans cela, « chercher dans la documentation »
    // oblige a aller fouiller soi-meme.
    ynp_spec::ajouter_les_pistes(&mut arbitrages, &faits, &recupere.tree);
    if !arbitrages.is_empty() {
        // Ce n'est pas un echec : l'outil a etabli tout ce que le depot permet
        // d'etablir. Le reste demande une decision, qu'on va chercher.
        registre.demander(id, arbitrages);
        return Ok(());
    }

    finir(registre, id, &travail)
}

/// De la specification completee au paquet verifie.
fn finir(registre: &Registre, id: &str, travail: &Path) -> anyhow::Result<()> {
    let chemin = chemin_appspec(travail);
    let spec: ynp_core::AppSpec = toml::from_str(&std::fs::read_to_string(&chemin)?)?;

    let restants = spec.arbitrages();
    if !restants.is_empty() {
        registre.demander(id, restants);
        return Ok(());
    }

    registre.avancer(id, "generation", "rendu du paquet…");
    let genere = ynp_gen::generate(&spec, travail)?;
    registre.avancer(
        id,
        "generation",
        &format!("{} fichiers ecrits", genere.fichiers.len()),
    );

    registre.avancer(id, "verification", "conformite statique…");
    let constats = ynp_verify::verify(&genere.racine, &spec)?;
    if ynp_verify::gate(&constats).blocks_pipeline() {
        let bloquants: Vec<String> = constats
            .iter()
            .filter(|f| f.severity == ynp_core::Severity::Blocker)
            .map(|f| format!("{} : {}", f.id, f.title))
            .collect();
        registre.conclure(
            id,
            false,
            &format!("paquet non conforme — {}", bloquants.join(" ; ")),
        );
        return Ok(());
    }

    registre.conclure_avec(
        id,
        true,
        &format!("paquet pret : {}", genere.racine.display()),
        genere
            .racine
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
    );
    Ok(())
}
