//! `yunopack` — packager d'applications pour YunoHost.
//!
//! Chaque sous-commande correspond a un etage du pipeline et ecrit son artefact,
//! ce qui permet de reprendre le travail en cours de route ou de corriger la
//! main a l'etage de decision.

mod eval;
mod report;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "yunopack",
    version,
    about = "Colle une URL de depot, recupere une application YunoHost installable",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Repertoire de travail ou sont ecrits les artefacts.
    #[arg(long, short = 'o', global = true, default_value = ".yunopack")]
    out: PathBuf,

    /// Sortie JSON au lieu du rapport lisible.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Collecte ce qui est dans le depot, sans interpretation.
    Analyze {
        /// URL du depot, sous n'importe quelle forme.
        url: String,
    },

    /// Decide si le depot est packageable, et dit pourquoi si ce n'est pas le cas.
    Assess {
        /// URL du depot. A defaut, le facts.json deja produit par `analyze` est relu.
        url: Option<String>,

        /// Score en deca duquel le verdict devient « faisable avec travail ».
        #[arg(long, default_value_t = ynp_core::DEFAULT_FEASIBILITY_THRESHOLD)]
        seuil: u8,
    },

    /// Produit l'appspec.toml : le seul document ou une decision se prend.
    Plan {
        /// URL du depot. A defaut, le facts.json deja produit est relu.
        url: Option<String>,

        /// Produit la specification meme s'il y reste des champs a completer.
        #[arg(long)]
        force: bool,
    },

    /// Repond aux champs que `plan` n'a pas pu determiner.
    ///
    /// Sans argument, liste les champs ouverts avec leurs propositions.
    Repondre {
        /// `champ=valeur`, ou `champ=@n` pour reprendre la proposition n.
        /// Repetable.
        reponses: Vec<String>,
    },

    /// Examine un paquet deja au catalogue et ecrit ce qui se corrige seul.
    Ameliorer {
        /// Identifiant de l'application au catalogue, ou URL de son paquet.
        app: String,

        /// Ne rien ecrire : dire seulement ce qui serait change.
        #[arg(long)]
        simuler: bool,
    },

    /// Rend le paquet a partir de l'appspec.toml.
    Generate {
        /// Genere malgre des champs non completes, qui apparaitront en FIXME.
        #[arg(long)]
        force: bool,
    },

    /// Verifie la conformite statique du paquet genere.
    Verify {
        /// Repertoire du paquet. A defaut, celui produit par `generate`.
        chemin: Option<PathBuf>,
    },

    /// Installe reellement le paquet sur un hote YunoHost et verifie qu'il tourne.
    Test {
        /// Alias SSH de la machine de test, tel que declare dans ~/.ssh/config.
        ///
        /// Aucune valeur par defaut : cette machine va installer un paquet non
        /// relu, dont les scripts tournent en root. Elle doit etre designee,
        /// jamais supposee.
        #[arg(long, env = "YUNOPACK_HOTE_TEST")]
        host: String,

        /// Domaine d'installation. A defaut, le domaine principal de l'hote.
        #[arg(long)]
        domaine: Option<String>,

        /// Sauter la sauvegarde et la restauration, plus lentes.
        #[arg(long)]
        rapide: bool,
    },

    /// Publie le paquet sur la forge et l'inscrit au catalogue.
    Publish {
        /// Alias SSH de la forge, declare dans ~/.ssh/config.
        #[arg(long, default_value = "forgejo")]
        forge: String,

        /// Fichier de catalogue a mettre a jour.
        #[arg(long, default_value = "catalogue/apps.json")]
        catalogue: PathBuf,

        /// Ne rien envoyer : montrer ce qui serait fait.
        #[arg(long)]
        simuler: bool,

        /// Preparer aussi une contribution au catalogue officiel de YunoHost.
        #[arg(long)]
        officiel: bool,

        /// URL d'un depot deja publie, a cataloguer sans le repousser.
        #[arg(long)]
        depot: Option<String>,
    },

    /// Enchaine tout le pipeline, de l'URL au paquet publie.
    Run {
        /// URL du depot.
        url: String,

        /// Hote de test pour la gate G3. Omis, l'installation reelle est sautee.
        #[arg(long)]
        host: Option<String>,

        /// Poursuivre malgre une porte en echec. Reserve a la mise au point.
        #[arg(long)]
        force: bool,
    },

    /// Compare les paquets produits a ceux du catalogue officiel.
    Eval {
        /// Fichier decrivant le corpus d'evaluation.
        #[arg(long, default_value = "tests/corpus.toml")]
        corpus: PathBuf,

        /// N'evaluer qu'une application du corpus.
        #[arg(long)]
        seulement: Option<String>,
    },

    /// Liste ce que la communaute YunoHost aimerait voir package.
    Wishlist {
        /// Ne montrer que ce que l'outil sait analyser aujourd'hui.
        #[arg(long)]
        analysables: bool,

        /// Filtrer sur un terme, dans le nom ou la description.
        #[arg(long)]
        cherche: Option<String>,

        /// Nombre d'entrees affichees.
        #[arg(long, default_value_t = 30)]
        limite: usize,
    },

    /// Cherche des alternatives auto-hebergeables, et dit lesquelles restent a packager.
    Alternatives {
        /// Nom du logiciel dont on cherche des alternatives.
        terme: String,

        /// Inclure celles qui sont deja au catalogue YunoHost.
        #[arg(long)]
        tout: bool,

        /// Nombre d'entrees affichees.
        #[arg(long, default_value_t = 20)]
        limite: usize,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nErreur : {e}");
            // Les causes profondes portent souvent l'information utile
            // (quota d'API, depot prive), il serait dommage de les perdre.
            let mut source = e.source();
            while let Some(cause) = source {
                eprintln!("  dû à : {cause}");
                source = cause.source();
            }
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Analyze { url } => analyze(url, &cli).await.map(|_| ()),
        Command::Assess { url, seuil } => assess(url.as_deref(), *seuil, &cli).await.map(|_| ()),
        Command::Plan { url, force } => plan(url.as_deref(), *force, &cli).await.map(|_| ()),
        Command::Repondre { reponses } => repondre(reponses, &cli),
        Command::Ameliorer { app, simuler } => ameliorer(app, *simuler, &cli).await,
        Command::Generate { force } => generate(*force, &cli),
        Command::Verify { chemin } => verify(chemin.as_deref(), &cli),
        Command::Test {
            host,
            domaine,
            rapide,
        } => test(host, domaine.clone(), *rapide, &cli).await.map(|_| ()),
        Command::Publish {
            forge,
            catalogue,
            simuler,
            officiel,
            depot,
        } => {
            publish(
                forge,
                catalogue,
                *simuler,
                *officiel,
                depot.as_deref(),
                &cli,
            )
            .await
        }
        Command::Run { url, host, force } => run_pipeline(url, host.as_deref(), *force, &cli).await,
        Command::Eval { corpus, seulement } => evaluer(corpus, seulement.as_deref(), &cli).await,
        Command::Wishlist {
            analysables,
            cherche,
            limite,
        } => wishlist(*analysables, cherche.as_deref(), *limite).await,
        Command::Alternatives {
            terme,
            tout,
            limite,
        } => alternatives(terme, *tout, *limite).await,
    }
}

async fn analyze(url: &str, cli: &Cli) -> anyhow::Result<ynp_core::facts::RepoFacts> {
    if !cli.json {
        eprintln!("Recuperation de {url} …");
    }
    let fetched = ynp_forge::fetch(url).await?;

    if !cli.json {
        eprintln!(
            "  {} fichiers, source retenue : {} ({})",
            fetched.tree.len(),
            fetched.choice.reference,
            strategie(&fetched.choice),
        );
    }

    let mut facts = ynp_analyze::analyze(fetched.forge, &fetched.tree);
    facts.selection = Some(fetched.selection.clone());

    std::fs::create_dir_all(&cli.out)?;
    let path = cli.out.join("facts.json");
    std::fs::write(&path, serde_json::to_string_pretty(&facts)?)?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&facts)?);
    } else {
        print!("{}", report::facts(&facts));
        print!("{}", report::selection(&fetched.selection));
        println!("\nFaits ecrits dans {}", path.display());
    }
    Ok(facts)
}

/// Verdict de faisabilite, avec les portes G0 et G1.
///
/// Le code de sortie designe la porte en echec, pour qu'un script appelant
/// sache *ou* ca a casse sans analyser la sortie.
async fn assess(
    url: Option<&str>,
    seuil: u8,
    cli: &Cli,
) -> anyhow::Result<ynp_core::facts::RepoFacts> {
    let facts = match url {
        Some(u) => analyze(u, cli).await?,
        None => {
            let path = cli.out.join("facts.json");
            let text = std::fs::read_to_string(&path).map_err(|e| {
                anyhow::anyhow!(
                    "{} illisible ({e}) — lancer d'abord `yunopack analyze <url>`",
                    path.display()
                )
            })?;
            serde_json::from_str(&text)?
        }
    };

    let (feasibility, gates) = ynp_rules::gates::evaluate(&facts, seuil);

    std::fs::create_dir_all(&cli.out)?;
    std::fs::write(
        cli.out.join("report.json"),
        serde_json::to_string_pretty(&feasibility)?,
    )?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&feasibility)?);
    } else {
        print!("{}", report::feasibility(&feasibility, &gates));
    }

    let code = gates.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(facts)
}

/// Produit `appspec.toml`, le document de decision.
///
/// C'est ici que le pipeline rend la main : ce que l'analyse n'a pas su
/// deduire y figure en clair, et c'est a un humain ou a un agent de le
/// completer avant de generer le paquet.
async fn plan(url: Option<&str>, force: bool, cli: &Cli) -> anyhow::Result<ynp_core::AppSpec> {
    let facts = assess(url, ynp_core::DEFAULT_FEASIBILITY_THRESHOLD, cli).await?;
    let spec = ynp_spec::build(&facts)?;

    std::fs::create_dir_all(&cli.out)?;
    let path = cli.out.join("appspec.toml");
    std::fs::write(&path, toml::to_string_pretty(&spec)?)?;

    let manquants = spec.unresolved();
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&spec)?);
    } else {
        print!("{}", report::spec(&spec));
        println!("\nSpecification ecrite dans {}", path.display());
    }

    if !manquants.is_empty() && !force {
        eprintln!(
            "\n{} champ(s) restent a completer.\n\
             `yunopack repondre` les liste avec leurs propositions ;\n\
             editer {} directement reste possible.",
            manquants.len(),
            path.display()
        );
        std::process::exit(20);
    }
    Ok(spec)
}

/// Examine un paquet publie et ecrit ce qui se corrige mecaniquement.
///
/// Sept cents applications sont deja au catalogue et une bonne part n'atteint
/// pas le niveau maximal. Les refaire de zero serait du gachis : elles
/// marchent, quelqu'un s'en occupe. Reste a savoir ce qui leur manque.
async fn ameliorer(app: &str, simuler: bool, cli: &Cli) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("yunopack/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let catalogue = ynp_forge::catalogue::Catalogue::charger(&client).await?;

    let (depot, niveau, id) = match catalogue.trouver(app) {
        Some(p) => (p.depot_paquet.clone(), p.niveau, p.id.clone()),
        // Une URL directe permet d'examiner un paquet qui n'est pas encore au
        // catalogue — c'est le cas de tous ceux qu'on vient de produire.
        None if app.starts_with("http") => (app.to_string(), None, String::new()),
        None => anyhow::bail!("« {app} » n'est ni au catalogue ni une URL de depot"),
    };

    println!("\n  Recuperation de {depot} …");
    let recupere = ynp_forge::fetch(&depot).await?;

    let dossier = cli.out.join("ameliorations").join(if id.is_empty() {
        recupere.forge.source.repo.clone()
    } else {
        id.clone()
    });
    let _ = std::fs::remove_dir_all(&dossier);
    for chemin in recupere.tree.paths() {
        if chemin.contains("..") || chemin.starts_with('/') {
            continue;
        }
        let cible = dossier.join(&chemin);
        if let Some(d) = cible.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(cible, recupere.tree.text(&chemin).unwrap_or_default())?;
    }

    let examen = ynp_verify::audit::examiner(&dossier, niveau);
    print!("{}", report::examen(&examen));

    if examen.rien_a_signaler() {
        return Ok(());
    }
    if simuler {
        println!("  Simulation : rien n'a ete ecrit.\n");
        return Ok(());
    }

    let correctif = ynp_verify::correctif::appliquer(&dossier, &examen)?;
    if correctif.rien() {
        // Un constat marque corrigeable ne l'est pas toujours dans ce
        // paquet-ci : une source inhabituelle, une section absente. Le dire
        // vaut mieux qu'un « rien a faire » qui contredit l'examen.
        if examen.constats.iter().any(|c| c.reparable) {
            println!(
                "  Les points corrigeables ne l'etaient pas dans ce paquet :\n  \
                 sa forme sort de ce que l'outil sait reecrire sans risque.\n"
            );
        } else {
            println!("  Rien ne se corrige mecaniquement ici.\n");
        }
        return Ok(());
    }
    println!("  Corrige dans {} :", dossier.display());
    for f in &correctif.fichiers {
        println!("    {f}");
    }
    println!(
        "\n  Relire, puis proposer le correctif au mainteneur.\n           L'interface web ouvre la demande d'integration.\n"
    );
    Ok(())
}

/// Repond aux champs ouverts sans passer par l'editeur.
///
/// Editer le TOML a la main reste possible et reste la reference ; mais pour
/// une ou deux lignes, ouvrir un editeur, retrouver le champ et respecter la
/// syntaxe est une friction sans contrepartie. Ici la reponse tient sur une
/// ligne, et `@n` reprend une proposition sans la recopier.
fn repondre(reponses: &[String], cli: &Cli) -> anyhow::Result<()> {
    let chemin = cli.out.join("appspec.toml");
    let texte = std::fs::read_to_string(&chemin).map_err(|e| {
        anyhow::anyhow!(
            "{} illisible ({e}) — lancer d'abord `yunopack plan <url>`",
            chemin.display()
        )
    })?;
    let mut spec: ynp_core::AppSpec = toml::from_str(&texte)?;

    if reponses.is_empty() {
        let arbitrages = spec.arbitrages();
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&arbitrages)?);
            return Ok(());
        }
        if arbitrages.is_empty() {
            println!("\n  Rien a completer : la specification est complete.\n");
            return Ok(());
        }
        println!("\n  ── {} champ(s) a completer ──", arbitrages.len());
        print!("{}", report::a_completer(&arbitrages));
        println!(
            "\n  Repondre avec : yunopack repondre <champ>='<valeur>'\n\
             \x20 ou, pour reprendre une proposition : yunopack repondre <champ>=@1\n"
        );
        return Ok(());
    }

    let arbitrages = spec.arbitrages();
    let mut appliquees = Vec::new();
    for entree in reponses {
        let (champ, brut) = entree.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("« {entree} » n'a pas la forme attendue « champ=valeur »")
        })?;
        let valeur = resoudre_raccourci(champ, brut, &arbitrages)?;
        spec.repondre(champ, &valeur)?;
        appliquees.push((champ.to_string(), valeur));
    }

    std::fs::write(&chemin, toml::to_string_pretty(&spec)?)?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&spec)?);
        return Ok(());
    }
    for (champ, valeur) in &appliquees {
        println!("  {champ} = {}", valeur.replace('\n', " ⏎ "));
    }
    let restants = spec.arbitrages();
    if restants.is_empty() {
        println!("\n  Specification complete — `yunopack generate` peut suivre.\n");
    } else {
        println!("\n  {} champ(s) restent a completer :", restants.len());
        for a in &restants {
            println!("    {}", a.champ);
        }
        println!();
    }
    Ok(())
}

/// Traduit `@n` en la n-ieme proposition du champ. Tout le reste est litteral.
fn resoudre_raccourci(
    champ: &str,
    brut: &str,
    arbitrages: &[ynp_core::spec::Arbitrage],
) -> anyhow::Result<String> {
    let Some(rang) = brut.strip_prefix('@') else {
        return Ok(brut.to_string());
    };
    let n: usize = rang
        .parse()
        .map_err(|_| anyhow::anyhow!("« @{rang} » n'est pas un numero de proposition"))?;

    let a = arbitrages
        .iter()
        .find(|a| a.champ == champ)
        .ok_or_else(|| anyhow::anyhow!("{champ} n'est pas un champ ouvert"))?;
    a.candidats
        .get(n.wrapping_sub(1))
        .map(|c| c.value.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{champ} n'a que {} proposition(s), @{n} n'existe pas",
                a.candidats.len()
            )
        })
}

/// Rend le paquet. Aucune decision ici : tout a ete tranche dans l'appspec.
fn generate(force: bool, cli: &Cli) -> anyhow::Result<()> {
    let chemin = cli.out.join("appspec.toml");
    let texte = std::fs::read_to_string(&chemin).map_err(|e| {
        anyhow::anyhow!(
            "{} illisible ({e}) — lancer d'abord `yunopack plan <url>`",
            chemin.display()
        )
    })?;
    let spec: ynp_core::AppSpec = toml::from_str(&texte)?;

    let manquants = spec.unresolved();
    if !manquants.is_empty() && !force {
        eprintln!(
            "Specification incomplete : {} champ(s) a renseigner.",
            manquants.len()
        );
        for (champ, _) in &manquants {
            eprintln!("  {champ}");
        }
        eprintln!(
            "\n`yunopack repondre` les liste avec leurs propositions, ou les\n\
             completer dans {} ; --force genere malgre tout\n\
             un paquet portant des marqueurs FIXME.",
            chemin.display()
        );
        std::process::exit(20);
    }

    let genere = ynp_gen::generate(&spec, &cli.out)?;

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "racine": genere.racine,
                "fichiers": genere.fichiers,
                "a_completer": genere.a_completer,
            })
        );
    } else {
        println!("\nPaquet genere dans {}\n", genere.racine.display());
        for f in &genere.fichiers {
            println!("  {f}");
        }
        if genere.a_completer.is_empty() {
            println!("\nAucun marqueur a completer.");
        } else {
            println!(
                "\n{} marqueur(s) FIXME deposes — `yunopack verify` refusera le paquet\n\
                 tant qu'ils subsistent.",
                genere.a_completer.len()
            );
        }
    }
    Ok(())
}

fn strategie(c: &ynp_forge::SourceChoice) -> &'static str {
    match c.kind {
        ynp_forge::SourceKind::Release => "release",
        ynp_forge::SourceKind::Tag => "tag",
        ynp_forge::SourceKind::Commit => "commit — ni release ni tag exploitable",
    }
}

/// Gate G2 : conformite statique.
fn verify(chemin: Option<&std::path::Path>, cli: &Cli) -> anyhow::Result<()> {
    // Un paquet genere emporte son `.appspec.toml` : c'est ce qui permet de le
    // reverifier tel quel, sans avoir rejoue `plan` dans le repertoire de
    // travail. On ne se rabat dessus que si le repertoire de travail n'en a
    // pas, pour qu'une session en cours garde la main.
    let spec_path = match chemin {
        Some(c) if !cli.out.join("appspec.toml").is_file() && c.join(".appspec.toml").is_file() => {
            c.join(".appspec.toml")
        }
        _ => cli.out.join("appspec.toml"),
    };
    let spec: ynp_core::AppSpec = toml::from_str(
        &std::fs::read_to_string(&spec_path)
            .map_err(|e| anyhow::anyhow!("{} illisible : {e}", spec_path.display()))?,
    )?;

    let racine = match chemin {
        Some(c) => c.to_path_buf(),
        None => cli.out.join(format!("{}_ynh", spec.app.id)),
    };
    if !racine.is_dir() {
        anyhow::bail!(
            "{} introuvable — lancer d'abord `yunopack generate`",
            racine.display()
        );
    }

    let mut constats = ynp_verify::verify(&racine, &spec)?;
    constats.extend(syntaxe_bash(&racine)?);

    let porte = ynp_verify::gate(&constats);
    // `verify` peut etre lance seul sur un paquet deja ecrit ailleurs : le
    // repertoire de travail n'existe alors pas encore.
    std::fs::create_dir_all(&cli.out)?;
    std::fs::write(
        cli.out.join("lint.json"),
        serde_json::to_string_pretty(&constats)?,
    )?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&constats)?);
    } else {
        print!("{}", report::verification(&racine, &constats));
    }

    if porte.blocks_pipeline() {
        std::process::exit(12);
    }
    Ok(())
}

/// `bash -n` sur chaque script : une erreur de syntaxe se voit sans installer.
fn syntaxe_bash(racine: &std::path::Path) -> anyhow::Result<Vec<ynp_core::Finding>> {
    use ynp_core::finding::{Evidence, Finding, Severity};

    let mut out = Vec::new();
    let Ok(entrees) = std::fs::read_dir(racine.join("scripts")) else {
        return Ok(out);
    };

    let mut chemins: Vec<_> = entrees.filter_map(Result::ok).map(|e| e.path()).collect();
    chemins.sort();

    for chemin in chemins.iter().filter(|c| c.is_file()) {
        let nom = chemin
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let sortie = std::process::Command::new("bash")
            .arg("-n")
            .arg(chemin)
            .output()?;
        if !sortie.status.success() {
            out.push(
                Finding::new(
                    "BASH001",
                    Severity::Blocker,
                    format!("Erreur de syntaxe : {nom}"),
                )
                .detail(String::from_utf8_lossy(&sortie.stderr).trim().to_string())
                .evidence(Evidence::file(format!("scripts/{nom}"))),
            );
        }
    }
    Ok(out)
}

/// Gate G3 : le paquet s'installe-t-il et fonctionne-t-il vraiment ?
async fn test(
    host: &str,
    domaine: Option<String>,
    rapide: bool,
    cli: &Cli,
) -> anyhow::Result<ynp_runner::Rapport> {
    let spec_path = cli.out.join("appspec.toml");
    let spec: ynp_core::AppSpec = toml::from_str(&std::fs::read_to_string(&spec_path)?)?;
    let racine = cli.out.join(format!("{}_ynh", spec.app.id));
    if !racine.is_dir() {
        anyhow::bail!(
            "{} introuvable — lancer d'abord `yunopack generate`",
            racine.display()
        );
    }

    let cible = ynp_runner::Cible {
        domaine,
        rapide,
        ..ynp_runner::Cible::new(host)
    };
    if !cli.json {
        eprintln!("Campagne sur « {host} » — cela prend quelques minutes.\n");
    }

    let rapport = ynp_runner::run_g3(&racine, &spec, &cible).await?;
    std::fs::write(
        cli.out.join("test.json"),
        serde_json::to_string_pretty(&rapport)?,
    )?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&rapport)?);
    } else {
        print!("{}", report::campagne(&rapport));
    }

    if rapport.gate().blocks_pipeline() {
        std::process::exit(13);
    }
    Ok(rapport)
}

/// Gate G5 : publication sur la forge et inscription au catalogue.
async fn publish(
    alias_forge: &str,
    catalogue: &std::path::Path,
    simuler: bool,
    officiel: bool,
    deja_publie: Option<&str>,
    cli: &Cli,
) -> anyhow::Result<()> {
    let spec: ynp_core::AppSpec =
        toml::from_str(&std::fs::read_to_string(cli.out.join("appspec.toml"))?)?;
    let racine = cli.out.join(format!("{}_ynh", spec.app.id));
    let depot = format!("{}_ynh", spec.app.id);

    // On ne publie pas un paquet dont on n'a pas verifie la conformite :
    // ce qui part sur la forge est installable par d'autres.
    let constats = ynp_verify::verify(&racine, &spec)?;
    if ynp_verify::gate(&constats).blocks_pipeline() {
        anyhow::bail!(
            "le paquet ne passe pas la verification statique — lancer `yunopack verify` \
             et corriger avant de publier"
        );
    }

    // Un paquet deja publie ailleurs n'a pas a etre repousse : on se contente
    // de l'inscrire au catalogue et, si demande, de preparer la contribution.
    if let Some(url) = deja_publie {
        let manifest: serde_json::Value =
            toml::from_str::<toml::Value>(&std::fs::read_to_string(racine.join("manifest.toml"))?)
                .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))?;

        let niveau = niveau_mesure(cli);
        let mut cat = ynp_publish::catalogue::Catalogue::charger(catalogue)?;
        cat.inscrire(&spec.app.id, manifest, url, "main", "HEAD", niveau);
        cat.ecrire(catalogue)?;

        println!("\n  {} inscrit au catalogue depuis {url}", spec.app.id);
        println!("  {} — {} app(s)", catalogue.display(), cat.nombre_d_apps());
        if officiel {
            preparer_contribution(&spec, url, niveau, cli)?;
        }
        return Ok(());
    }

    let Some(forge) = ynp_publish::forge::Forge::depuis_environnement(alias_forge) else {
        anyhow::bail!(
            "configuration de forge absente. Definir FORGEJO_URL et FORGEJO_OWNER \
             (et FORGEJO_TOKEN pour creer le depot automatiquement)."
        );
    };

    if simuler {
        println!("\n  Simulation — rien ne sera envoye.\n");
        println!("  depot     {}", forge.url_https(&depot));
        println!("  push      {}", forge.url_ssh(&depot));
        println!("  catalogue {}", catalogue.display());
        return Ok(());
    }

    let cree = forge
        .creer_depot(&depot, spec.app.description_en.value().map_or("", |v| v))
        .await?;
    let revision = ynp_publish::forge::pousser(
        &racine,
        &forge.url_ssh(&depot),
        "main",
        &format!("{} {}", spec.app.name, spec.app.version),
    )?;

    // Le manifest publie est celui du paquet, pas une reconstruction : le
    // catalogue doit decrire exactement ce qui sera installe.
    let manifest: serde_json::Value =
        toml::from_str::<toml::Value>(&std::fs::read_to_string(racine.join("manifest.toml"))?)
            .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))?;

    let niveau = std::fs::read_to_string(cli.out.join("test.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<ynp_runner::Rapport>(&t).ok())
        .filter(|r| r.reussi())
        // Un cycle G3 complet vaut le niveau 4 : installable, fonctionnel,
        // sauvegardable. Au-dela, seul package_check peut se prononcer.
        .map(|_| 4u8);

    let mut cat = ynp_publish::catalogue::Catalogue::charger(catalogue)?;
    cat.inscrire(
        &spec.app.id,
        manifest,
        &forge.url_https(&depot),
        "main",
        &revision,
        niveau,
    );
    cat.ecrire(catalogue)?;

    println!("\n  {} publie\n", spec.app.id);
    println!("  depot      {}", forge.url_https(&depot));
    println!("  revision   {}", &revision[..12.min(revision.len())]);
    if cree {
        println!("  (depot cree sur la forge)");
    }
    println!(
        "  catalogue  {} — {} app(s)",
        catalogue.display(),
        cat.nombre_d_apps()
    );
    match niveau {
        Some(n) => println!("  niveau     {n} (cycle G3 complet)"),
        None => println!("  niveau     0 — lancer `yunopack test` pour le mesurer"),
    }
    println!(
        "\n  Pour l'installer :  yunohost app install {}",
        forge.url_https(&depot)
    );

    if officiel {
        preparer_contribution(&spec, &forge.url_https(&depot), niveau, cli)?;
    }

    Ok(())
}

/// Enchaine le pipeline complet, en s'arretant a la premiere porte en echec.
async fn run_pipeline(url: &str, host: Option<&str>, force: bool, cli: &Cli) -> anyhow::Result<()> {
    eprintln!("── analyse et faisabilite ──");
    let spec = plan(Some(url), force, cli).await?;

    eprintln!("\n── generation ──");
    generate(force, cli)?;

    eprintln!("\n── verification statique ──");
    verify(None, cli)?;

    match host {
        None => {
            eprintln!(
                "\n  Installation reelle sautee (aucun --host).\n  \
                 Le paquet est pret dans {}",
                cli.out.join(format!("{}_ynh", spec.app.id)).display()
            );
        }
        Some(h) => {
            eprintln!("\n── installation reelle ──");
            test(h, None, false, cli).await?;
        }
    }
    Ok(())
}

/// Mesure la precision des detecteurs contre les paquets officiels.
async fn evaluer(
    corpus: &std::path::Path,
    seulement: Option<&str>,
    cli: &Cli,
) -> anyhow::Result<()> {
    let texte = std::fs::read_to_string(corpus)
        .map_err(|e| anyhow::anyhow!("{} illisible : {e}", corpus.display()))?;
    let corpus: eval::Corpus = toml::from_str(&texte)?;

    let entrees: Vec<_> = corpus
        .app
        .into_iter()
        .filter(|a| match seulement {
            None => true,
            Some(s) => a.nom == s,
        })
        .collect();
    if entrees.is_empty() {
        anyhow::bail!("aucune application ne correspond");
    }

    let mut resultats = Vec::new();
    for entree in &entrees {
        eprintln!("  {} …", entree.nom);
        resultats.push(evaluer_une(entree, cli).await);
    }

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&resultats)?);
        return Ok(());
    }

    for r in &resultats {
        println!("\n  ── {} ── {}", r.app, r.difficulte);
        if let Some(e) = &r.erreur {
            println!("     non evaluee : {e}");
            continue;
        }
        for ecart in r.ecarts.iter().filter(|e| !e.accord) {
            println!(
                "     {:<26} nous « {} »  officiel « {} »",
                ecart.champ, ecart.nous, ecart.officiel
            );
        }
        println!("     {}/{} champs en accord", r.accords(), r.total());
    }
    print!("{}", eval::matrice(&resultats));
    Ok(())
}

async fn evaluer_une(entree: &eval::Entree, cli: &Cli) -> eval::Resultat {
    let echec = |e: String| eval::Resultat {
        app: entree.nom.clone(),
        difficulte: entree.difficulte.clone(),
        erreur: Some(e),
        ecarts: Vec::new(),
    };

    // Notre paquet, produit par le pipeline complet jusqu'a la generation.
    let travail = cli.out.join("eval").join(&entree.nom);
    if let Err(e) = std::fs::create_dir_all(&travail) {
        return echec(e.to_string());
    }

    let recupere = match ynp_forge::fetch(&entree.amont).await {
        Ok(r) => r,
        Err(e) => return echec(e.to_string()),
    };
    let mut faits = ynp_analyze::analyze(recupere.forge, &recupere.tree);
    faits.selection = Some(recupere.selection);

    let spec = match ynp_spec::build(&faits) {
        Ok(s) => s,
        Err(e) => return echec(e.to_string()),
    };
    // Un champ non resolu n'empeche pas la comparaison : on veut justement
    // savoir ce que l'outil deduit, y compris quand il s'arrete.
    let genere = match ynp_gen::generate(&spec, &travail) {
        Ok(g) => g,
        Err(e) => return echec(e.to_string()),
    };

    let notre = match std::fs::read_to_string(genere.racine.join("manifest.toml"))
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
    {
        Some(v) => v,
        None => return echec("manifest genere illisible".into()),
    };

    // Le manifest officiel, tel qu'il est publie.
    let brut = entree
        .paquet
        .replace("https://github.com/", "https://raw.githubusercontent.com/");
    let mut officiel = None;
    for branche in ["master", "main"] {
        let url = format!("{brut}/{branche}/manifest.toml");
        if let Ok(r) = reqwest::get(&url).await {
            if r.status().is_success() {
                if let Ok(t) = r.text().await {
                    officiel = toml::from_str::<toml::Value>(&t).ok();
                    break;
                }
            }
        }
    }
    let Some(officiel) = officiel else {
        return echec("manifest officiel introuvable".into());
    };

    eval::Resultat {
        app: entree.nom.clone(),
        difficulte: entree.difficulte.clone(),
        erreur: None,
        ecarts: eval::comparer(&notre, &officiel),
    }
}

/// Prepare la contribution au catalogue officiel — sans rien envoyer.
///
/// Une pull request vers un projet tiers engage l'utilisateur, pas l'outil.
fn preparer_contribution(
    spec: &ynp_core::AppSpec,
    url: &str,
    niveau: Option<u8>,
    cli: &Cli,
) -> anyhow::Result<()> {
    match ynp_publish::officiel::preparer(spec, url, niveau) {
        Err(e) => println!("\n  Contribution officielle impossible :\n    {e}"),
        Ok(c) => {
            let dossier = cli.out.join("officiel");
            std::fs::create_dir_all(&dossier)?;
            std::fs::write(dossier.join("apps.toml.fragment"), &c.entree)?;
            std::fs::write(dossier.join("pull-request.md"), &c.message)?;

            println!(
                "\n  Contribution officielle preparee dans {}",
                dossier.display()
            );
            println!("  Rien n'a ete envoye : ouvrir la pull request sur");
            println!("  https://github.com/YunoHost/apps est une decision qui vous revient.");
        }
    }
    Ok(())
}

/// Ce que la communaute aimerait voir packager.
///
/// Partir de la liste de souhaits evite la question « que packager ? » : cinq
/// cents demandes y sont deja formulees, avec leur depot amont.
async fn wishlist(analysables: bool, cherche: Option<&str>, limite: usize) -> anyhow::Result<()> {
    eprintln!("Recuperation de la liste de souhaits…");
    let souhaits = ynp_forge::wishlist::recuperer().await?;
    let total = souhaits.len();

    let retenus: Vec<_> = souhaits
        .iter()
        // Un paquet deja en preparation ailleurs ferait doublon.
        .filter(|s| !s.en_cours())
        .filter(|s| !analysables || s.analysable())
        .filter(|s| match cherche {
            None => true,
            Some(t) => {
                let t = t.to_lowercase();
                s.name.to_lowercase().contains(&t) || s.description.to_lowercase().contains(&t)
            }
        })
        .collect();

    println!(
        "\n  {total} demandes en attente, dont {} retenues ici\n",
        retenus.len()
    );
    for s in retenus.iter().take(limite) {
        let marque = if s.analysable() { " " } else { "·" };
        println!(
            "  {marque} {:<26} {}",
            tronquer(&s.name, 26),
            tronquer(&s.description, 60)
        );
        println!("    {:<26} {}", "", s.upstream);
    }
    if retenus.len() > limite {
        println!(
            "\n  … et {} autres (--limite pour en voir plus)",
            retenus.len() - limite
        );
    }

    let non_github = retenus.iter().filter(|s| !s.analysable()).count();
    if non_github > 0 && !analysables {
        println!(
            "\n  · {non_github} sont hors GitHub : l'outil ne sait pas encore les analyser.\n    \
             `--analysables` les masque."
        );
    }
    println!("\n  Pour en traiter une :  yunopack run <depot amont>");
    Ok(())
}

/// Alternatives auto-hebergeables a un logiciel, et ce qui reste a packager.
async fn alternatives(terme: &str, tout: bool, limite: usize) -> anyhow::Result<()> {
    eprintln!("Recuperation du catalogue auto-heberge…");
    let catalogue = ynp_forge::alternatives::Catalogue::charger().await?;

    let trouvees = catalogue.alternatives(terme);
    if trouvees.is_empty() {
        // Le terme ne designe peut-etre aucun logiciel connu : on le cherche
        // alors comme un mot-cle, plutot que de rendre la main sans rien.
        let correspondances = catalogue.chercher(terme);
        if correspondances.is_empty() {
            println!(
                "\n  Rien ne correspond a « {terme} » parmi {} logiciels.",
                catalogue.nombre()
            );
            return Ok(());
        }
        println!("\n  « {terme} » ne designe aucun logiciel connu. Correspondances :\n");
        for l in correspondances.iter().take(limite) {
            println!(
                "  {:<26} {}",
                tronquer(&l.name, 26),
                tronquer(&l.description, 60)
            );
        }
        return Ok(());
    }

    let liste = if tout {
        trouvees.clone()
    } else {
        catalogue.a_packager(&trouvees)
    };
    println!(
        "\n  {} alternative(s) a « {terme} »{}\n",
        liste.len(),
        if tout { "" } else { ", restant a packager" }
    );

    for l in liste.iter().take(limite) {
        let licence = l.licenses.first().map(String::as_str).unwrap_or("?");
        let marque = if l.deja_package {
            "✓"
        } else if !l.analysable() {
            "·"
        } else if !l.licence_libre() {
            "!"
        } else {
            " "
        };
        println!(
            "  {marque} {:<22} {:>6}★  {:<14} {}",
            tronquer(&l.name, 22),
            l.stargazers_count,
            tronquer(licence, 14),
            tronquer(&l.description, 46)
        );
        println!("    {:<22} {}", "", l.source_code_url);
    }

    println!(
        "\n  ✓ deja au catalogue   · forge non prise en charge   ! licence a verifier\n\n  \
         Pour en packager une :  yunopack run <depot source>"
    );
    Ok(())
}

/// Tronque proprement, en signalant la coupe.
fn tronquer(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max - 1).collect::<String>())
}

/// Niveau etabli par la derniere campagne, s'il y en a eu une.
///
/// Un cycle G3 complet vaut le niveau 4 : installable, fonctionnel,
/// sauvegardable. Au-dela, seul `package_check` peut se prononcer.
fn niveau_mesure(cli: &Cli) -> Option<u8> {
    std::fs::read_to_string(cli.out.join("test.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<ynp_runner::Rapport>(&t).ok())
        .filter(ynp_runner::Rapport::reussi)
        .map(|_| 4)
}
