//! Dockerfile -> [`BuildRecipe`].
//!
//! Le pari du projet tient dans ce fichier : un Dockerfile est deja une
//! specification de construction lisible par une machine. `FROM` donne le
//! runtime et sa version, `apt-get install` donne les dependances
//! litteralement, `RUN npm ci` donne les etapes de build, `EXPOSE` le port,
//! `CMD` la commande de demarrage. Ce qu'on demanderait a un modele de deviner,
//! on le lit.
//!
//! Le Dockerfile n'est jamais execute ni embarque : une application YunoHost
//! tourne en natif. Il sert de documentation executable de ce que l'upstream
//! fait pour construire son application.

use crate::lexer::lex;
use crate::shell::{is_dynamic, split_commands, tokenize};
use indexmap::IndexMap;
use ynp_core::facts::{BuildRecipe, BuildStage};

/// Analyse un Dockerfile et en extrait la recette de build.
/// Les copies d'un `COPY` ou `ADD`, par chemin dans l'image.
///
/// Les copies entre etapes (`--from=`) et les URL ne viennent pas du depot :
/// elles sont ignorees. Une destination en repertoire recoit chaque source
/// sous son propre nom.
fn parse_copy(args: &str) -> Vec<(String, String)> {
    let mots: Vec<String> = if args.trim_start().starts_with('[') {
        parse_exec_form(args)
    } else {
        args.split_whitespace().map(unquote).collect()
    };
    if mots.iter().any(|m| m.starts_with("--from")) {
        return Vec::new();
    }
    let mots: Vec<&String> = mots.iter().filter(|m| !m.starts_with("--")).collect();
    let Some((dest, sources)) = mots.split_last() else {
        return Vec::new();
    };
    let repertoire = dest.ends_with('/') || sources.len() > 1;
    sources
        .iter()
        .filter(|s| !s.contains("://") && !s.contains('*'))
        .map(|s| {
            let s = s.trim_start_matches("./").to_string();
            let d = if repertoire {
                format!(
                    "{}/{}",
                    dest.trim_end_matches('/'),
                    s.rsplit('/').next().unwrap_or(&s)
                )
            } else {
                dest.to_string()
            };
            (d, s)
        })
        .collect()
}

pub fn parse(path: &str, content: &str) -> BuildRecipe {
    let instructions = lex(content);

    let mut recipe = BuildRecipe {
        dockerfile_path: path.to_string(),
        ..Default::default()
    };
    // Les ARG declares avant le premier FROM sont globaux et substituables
    // dans les lignes FROM : `ARG NODE=20` puis `FROM node:${NODE}`.
    let mut args: IndexMap<String, String> = IndexMap::new();
    let mut stages: Vec<StageAccu> = Vec::new();

    for ins in &instructions {
        match ins.keyword.as_str() {
            "ARG" => {
                if let Some((k, v)) = ins.args.split_once('=') {
                    args.insert(k.trim().to_string(), unquote(v.trim()));
                }
            }
            "FROM" => {
                let mut stage = parse_from(&ins.args, &args);
                resolve_alias(&mut stage, &stages);
                stages.push(stage);
            }
            // Les listes de paquets sont frequemment rangees dans un ARG
            // (`ARG RUNTIME_PACKAGES="curl gosu ..."`) puis referencees dans le
            // RUN. Sans substitution, on perdrait toutes ces dependances.
            "RUN" => collect_run(&expand(&ins.args, &args), &mut recipe),
            "ENV" => {
                for (k, v) in parse_env(&ins.args) {
                    // Une ENV est aussi substituable dans les RUN suivants.
                    args.insert(k.clone(), v.clone());
                    if let Some(s) = stages.last_mut() {
                        s.env.insert(k, v);
                    }
                }
            }
            "EXPOSE" => {
                if let Some(s) = stages.last_mut() {
                    s.expose.extend(parse_expose(&ins.args));
                }
            }
            "WORKDIR" => {
                if let Some(s) = stages.last_mut() {
                    s.workdir = Some(unquote(ins.args.trim()));
                }
            }
            "VOLUME" => {
                if let Some(s) = stages.last_mut() {
                    s.volumes.extend(parse_string_list(&ins.args));
                }
            }
            "COPY" | "ADD" => {
                // Les destinations s'ecrivent souvent avec une variable :
                // `COPY docker/entry.sh $MEALIE_HOME/run.sh`.
                for (dans_l_image, dans_le_depot) in parse_copy(&expand(&ins.args, &args)) {
                    recipe.copies.insert(dans_l_image, dans_le_depot);
                }
            }
            "CMD" => {
                if let Some(s) = stages.last_mut() {
                    s.cmd = Some(parse_exec_form(&ins.args));
                }
            }
            "ENTRYPOINT" => {
                if let Some(s) = stages.last_mut() {
                    s.entrypoint = Some(parse_exec_form(&ins.args));
                }
            }
            _ => {}
        }
    }

    finalize(&mut recipe, stages);
    dedup(&mut recipe);
    recipe
}

/// Une etape peut partir d'une etape precedente (`FROM builder`). Dans ce cas
/// son « image » est un alias local, pas une image reelle : on remonte jusqu'a
/// l'image de base effective, sans quoi la detection de stack conclurait que le
/// runtime est « s6-overlay-base » ou « builder ».
fn resolve_alias(stage: &mut StageAccu, previous: &[StageAccu]) {
    let Some(parent) = previous
        .iter()
        .find(|p| p.stage.alias.as_deref() == Some(stage.stage.image.as_str()))
    else {
        return;
    };
    stage.stage.image = parent.stage.image.clone();
    if stage.stage.tag.is_none() {
        stage.stage.tag = parent.stage.tag.clone();
    }
}

/// Accumulateur pour une etape : ces attributs ne se propagent pas d'une etape
/// a l'autre, sauf quand une etape part explicitement d'une precedente.
#[derive(Debug, Default)]
struct StageAccu {
    stage: BuildStage,
    /// Nom de l'etape precedente dont celle-ci herite, le cas echeant.
    from_alias: Option<String>,
    env: IndexMap<String, String>,
    expose: Vec<u16>,
    workdir: Option<String>,
    volumes: Vec<String>,
    cmd: Option<Vec<String>>,
    entrypoint: Option<Vec<String>>,
}

fn parse_from(args: &str, vars: &IndexMap<String, String>) -> StageAccu {
    let expanded = expand(args, vars);
    let toks = tokenize(&expanded);

    // `FROM --platform=$BUILDPLATFORM node:20 AS builder` : les drapeaux
    // precedent l'image. Les prendre pour l'image faisait passer les etages de
    // construction de gotify pour une image nommee « --platform=... », et
    // perdait avec elles le runtime qu'elles annoncent.
    let image_ref = toks
        .iter()
        .find(|t| !t.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    let alias = toks
        .iter()
        .position(|t| t.eq_ignore_ascii_case("as"))
        .and_then(|i| toks.get(i + 1))
        .cloned();

    // Le `:` d'un tag ne doit pas etre confondu avec celui d'un port dans un
    // registre prive (`registry.io:5000/img`) : on ne coupe qu'apres le
    // dernier `/`.
    let (image, tag) = match image_ref.rfind('/') {
        Some(slash) => match image_ref[slash..].find(':') {
            Some(colon) => {
                let at = slash + colon;
                (
                    image_ref[..at].to_string(),
                    Some(image_ref[at + 1..].to_string()),
                )
            }
            None => (image_ref.clone(), None),
        },
        None => match image_ref.split_once(':') {
            Some((i, t)) => (i.to_string(), Some(t.to_string())),
            None => (image_ref.clone(), None),
        },
    };

    StageAccu {
        stage: BuildStage {
            image: image.clone(),
            tag,
            alias,
        },
        from_alias: Some(image),
        ..Default::default()
    }
}

/// Les paquets et etapes de build sont collectes sur **toutes** les etapes.
///
/// Contrairement a Docker, YunoHost construit l'application sur la machine
/// cible : les dependances de build et de runtime finissent donc toutes dans
/// `[resources.apt]`. La distinction builder/runtime n'a pas d'interet ici.
fn collect_run(args: &str, recipe: &mut BuildRecipe) {
    let commande = strip_run_flags(args);
    // Un bloc `if`, `case`, `for` ou `while` ne se decoupe pas : ses
    // fragments ecartes, son corps devenait inconditionnel. Buzz copie un
    // certificat seulement `if [ -n "${EXTRA_CA_CERTS}" ]` ; decoupee,
    // l'etape copiait toujours un fichier qui n'existe pas. Gardee entiere,
    // elle s'execute par `bash -c` avec son sens d'origine — ici, rien.
    let fragments = split_commands(commande);
    let a_un_bloc = fragments.iter().any(|f| {
        tokenize(f)
            .first()
            .is_some_and(|t| matches!(t.as_str(), "if" | "case" | "for" | "while"))
    });
    if a_un_bloc {
        recipe.build_steps.push(commande.trim().to_string());
        return;
    }
    for cmd in fragments {
        let toks = tokenize(&cmd);
        if toks.is_empty() {
            continue;
        }
        match classify(&toks) {
            RunKind::AptInstall(pkgs) => recipe.apt_packages.extend(pkgs),
            RunKind::ApkAdd(pkgs) => recipe.apk_packages.extend(pkgs),
            RunKind::Noise => {}
            RunKind::BuildStep => recipe.build_steps.push(cmd.clone()),
        }
    }
}

/// Retire les options de `RUN` introduites par BuildKit, qui precedent la
/// commande : `RUN --mount=type=cache,target=/root/.cache npm ci`.
fn strip_run_flags(args: &str) -> &str {
    let mut rest = args.trim_start();
    while let Some(after) = rest.strip_prefix("--") {
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        rest = after[end..].trim_start();
    }
    rest
}

enum RunKind {
    AptInstall(Vec<String>),
    ApkAdd(Vec<String>),
    /// Bruit d'infrastructure sans equivalent cote YunoHost : mise a jour des
    /// index apt, nettoyage de cache, creation d'utilisateur...
    Noise,
    BuildStep,
}

fn classify(toks: &[String]) -> RunKind {
    // Les affectations d'environnement en tete (`DEBIAN_FRONTEND=... apt-get`)
    // et `sudo` ne font pas partie de la commande.
    let start = toks
        .iter()
        .position(|t| !t.contains('=') && t != "sudo" && t != "env")
        .unwrap_or(toks.len());
    let cmd = &toks[start..];
    let Some(bin) = cmd.first().map(|s| s.as_str()) else {
        return RunKind::Noise;
    };
    let bin = bin.rsplit('/').next().unwrap_or(bin);

    // `cargo chef` met en cache les couches d'une image Docker. Hors de
    // Docker, il ne fait que couter une compilation de plus.
    if bin == "cargo"
        && (cmd.get(1).is_some_and(|s| s == "chef")
            || (cmd.get(1).is_some_and(|s| s == "install")
                && cmd.iter().any(|t| t == "cargo-chef")))
    {
        return RunKind::Noise;
    }

    match bin {
        "apt-get" | "apt" | "aptitude" => match subcommand(cmd) {
            Some("install") => RunKind::AptInstall(packages_after(cmd, "install")),
            _ => RunKind::Noise, // update, clean, autoremove...
        },
        "apk" => match subcommand(cmd) {
            Some("add") => RunKind::ApkAdd(packages_after(cmd, "add")),
            _ => RunKind::Noise,
        },
        // Gestion du systeme : YunoHost s'en charge via ses resources.
        "rm"
        | "mkdir"
        | "chown"
        | "chmod"
        | "ln"
        | "useradd"
        | "adduser"
        | "groupadd"
        | "addgroup"
        | "echo"
        | "true"
        | ":"
        | "set"
        | "export"
        | "update-ca-certificates"
        | "dpkg-reconfigure"
        | "locale-gen"
        | "yum"
        | "dnf"
        | "microdnf" => RunKind::Noise,
        // Fragments d'un bloc shell coupe par le decoupage sur && et ; : les
        // conserver deposerait des morceaux de syntaxe dans l'appspec.
        "if" | "then" | "else" | "elif" | "fi" | "case" | "esac" | "for" | "while" | "do"
        | "done" | "{" | "}" | "(" | ")" => RunKind::Noise,
        _ => RunKind::BuildStep,
    }
}

fn subcommand(cmd: &[String]) -> Option<&str> {
    cmd.iter()
        .skip(1)
        .find(|t| !t.starts_with('-'))
        .map(|s| s.as_str())
}

/// Noms de paquets suivant un verbe (`install`, `add`), debarrasses des options
/// et des epinglages de version.
fn packages_after(cmd: &[String], verb: &str) -> Vec<String> {
    let Some(at) = cmd.iter().position(|t| t == verb) else {
        return Vec::new();
    };
    cmd[at + 1..]
        .iter()
        .filter(|t| !t.starts_with('-') && !is_dynamic(t))
        // Un epinglage `foo=1.2.3` ne se transpose pas : on garde le nom.
        .map(|t| t.split('=').next().unwrap_or(t).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn parse_env(args: &str) -> Vec<(String, String)> {
    let toks = tokenize(args);
    if toks.is_empty() {
        return Vec::new();
    }
    // Forme historique `ENV cle valeur avec des espaces`, reconnaissable a
    // l'absence de `=` dans le premier mot.
    if !toks[0].contains('=') {
        let value = toks[1..].join(" ");
        return vec![(toks[0].clone(), value)];
    }
    toks.iter()
        .filter_map(|t| {
            t.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn parse_expose(args: &str) -> Vec<u16> {
    tokenize(args)
        .iter()
        .filter_map(|t| t.split('/').next().unwrap_or(t).parse::<u16>().ok())
        .collect()
}

/// Forme exec (`["node", "server.js"]`) ou forme shell (`node server.js`).
///
/// La forme shell est conservee telle quelle plutot que prefixee de
/// `/bin/sh -c` : c'est la commande utile pour deriver un `ExecStart`.
fn parse_exec_form(args: &str) -> Vec<String> {
    let t = args.trim();
    if t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(t) {
            return v;
        }
    }
    if t.is_empty() {
        Vec::new()
    } else {
        vec![t.to_string()]
    }
}

fn parse_string_list(args: &str) -> Vec<String> {
    let t = args.trim();
    if t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(t) {
            return v;
        }
    }
    tokenize(t)
}

/// Reporte sur la recette les attributs de l'etape de runtime.
///
/// C'est la derniere etape qui compte, pas la premiere : dans un multi-etage,
/// l'image de build (`node:20`) n'est pas l'image de runtime (`nginx:alpine`),
/// et se tromper de cible fausse toute la detection de stack.
fn finalize(recipe: &mut BuildRecipe, stages: Vec<StageAccu>) {
    recipe.stages = stages.iter().map(|s| s.stage.clone()).collect();

    let Some(last) = stages.last() else { return };

    // Une etape qui part d'une etape precedente en herite tout : environnement,
    // ports, commande de demarrage, volumes. Docker fait de meme.
    //
    // Constate sur block/buzz : `EXPOSE 3000 8080 9102` et `ENTRYPOINT` sont
    // declares dans l'etape `runtime-base`, et la derniere etape est un simple
    // `FROM runtime-base AS runtime`. Sans heritage, le Dockerfile principal
    // paraissait n'exposer aucun port ni demarrer quoi que ce soit.
    let parent = last
        .from_alias
        .as_ref()
        .and_then(|a| stages.iter().find(|s| s.stage.alias.as_ref() == Some(a)));

    let mut env = IndexMap::new();
    if let Some(p) = parent {
        env.extend(p.env.clone());
    }
    env.extend(last.env.clone());
    recipe.env = env;

    recipe.expose = if last.expose.is_empty() {
        parent.map(|p| p.expose.clone()).unwrap_or_default()
    } else {
        last.expose.clone()
    };

    recipe.volumes = if last.volumes.is_empty() {
        parent.map(|p| p.volumes.clone()).unwrap_or_default()
    } else {
        last.volumes.clone()
    };
    recipe.cmd = last
        .cmd
        .clone()
        .or_else(|| parent.and_then(|p| p.cmd.clone()));
    recipe.entrypoint = last
        .entrypoint
        .clone()
        .or_else(|| parent.and_then(|p| p.entrypoint.clone()));
    recipe.workdir = last
        .workdir
        .clone()
        .or_else(|| parent.and_then(|p| p.workdir.clone()));
}

fn dedup(recipe: &mut BuildRecipe) {
    for list in [&mut recipe.apt_packages, &mut recipe.apk_packages] {
        let mut seen = std::collections::HashSet::new();
        list.retain(|p| seen.insert(p.clone()));
    }
    let mut seen = std::collections::HashSet::new();
    recipe.expose.retain(|p| seen.insert(*p));
}

/// Substitue les variables dont la valeur est connue statiquement.
///
/// Les variables sans valeur declaree ne sont pas substituees : elles restent
/// sous la forme `$NAME`, ce qui les fait ecarter par [`is_dynamic`]. On
/// prefere ne rien affirmer plutot que d'inventer un nom de paquet vide.
fn expand(s: &str, vars: &IndexMap<String, String>) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut out = s.to_string();
    // Les noms les plus longs d'abord : sans cela, `$APP` ecraserait le prefixe
    // de `$APP_VERSION`.
    let mut keys: Vec<&String> = vars.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in keys {
        let v = &vars[k];
        out = out
            .replace(&format!("${{{k}}}"), v)
            .replace(&format!("${k}"), v);
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '\'').to_string()
}

/// Nom des fichiers reconnus comme Dockerfile, par ordre de preference.
pub const DOCKERFILE_NAMES: &[&str] = &[
    "Dockerfile",
    "dockerfile",
    "docker/Dockerfile",
    "Dockerfile.prod",
    "build/Dockerfile",
];

/// Retrouve le Dockerfile le plus pertinent dans une liste de chemins.
///
/// Les variantes de developpement ou de test sont ecartees : elles decrivent un
/// environnement de travail, pas la construction du produit livre.
pub fn find_dockerfile(tree: &[String]) -> Option<String> {
    let excluded = ["dev", "test", "ci", "docs", "example"];
    let candidate = |p: &&String| {
        let lower = p.to_lowercase();
        lower
            .rsplit('/')
            .next()
            .is_some_and(|f| f.starts_with("dockerfile"))
            && !excluded.iter().any(|e| lower.contains(e))
    };

    DOCKERFILE_NAMES
        .iter()
        .find(|n| tree.iter().any(|p| p == *n))
        .map(|n| n.to_string())
        .or_else(|| {
            // A defaut, le moins profond dans l'arborescence.
            tree.iter()
                .filter(candidate)
                .min_by_key(|p| p.matches('/').count())
                .cloned()
        })
}

/// Choisit le Dockerfile qui decrit reellement l'application.
///
/// Un depot en contient souvent plusieurs : celui du produit, mais aussi ceux
/// qui construisent un paquet `.deb` ou `.rpm`, ou qui montent un environnement
/// de developpement. Constate sur miniflux, qui en compte quatre : retenir
/// `packaging/debian/Dockerfile` faisait passer `devscripts` et `dh-make` pour
/// des dependances de l'application, et `golang:1` pour sa version de runtime.
///
/// On note donc chaque candidat sur ce qui distingue un Dockerfile de runtime :
/// il expose un port, part d'une image de langage, et demarre l'application
/// plutot qu'un script de construction.
pub fn choose(tree: &ynp_core::tree::RepoTree) -> Option<String> {
    let candidates: Vec<String> = tree
        .find_by_name(est_un_dockerfile)
        .into_iter()
        .filter(|p| !est_une_variante_de_developpement(p))
        .collect();
    if candidates.is_empty() {
        return None;
    }

    candidates
        .into_iter()
        .map(|path| {
            let score = tree
                .text(&path)
                .map_or(0, |c| runtime_score(&path, &parse(&path, c)));
            (score, path)
        })
        // A egalite, le moins profond gagne, puis l'ordre alphabetique : le
        // choix doit etre reproductible d'une execution a l'autre.
        .max_by_key(|(score, path)| (*score, -(path.matches('/').count() as i32), path.clone()))
        .map(|(_, path)| path)
}

/// Vrai pour un Dockerfile qui monte un poste de travail, pas l'application.
///
/// `dev.Dockerfile` et `Dockerfile.dev` decrivent l'environnement du
/// developpeur : outils de debogage, sources montees en volume, serveur de
/// rechargement a chaud. AnonAddy n'en a pas d'autre, et le prendre pour la
/// recette de production faisait echouer la traduction de sa commande de
/// demarrage sur un paquet qui, sans lui, sortait complet. Mieux vaut n'avoir
/// aucune recette que la mauvaise : l'outil le dit alors franchement.
fn est_une_variante_de_developpement(chemin: &str) -> bool {
    const VARIANTES: &[&str] = &[
        "dev",
        "develop",
        "development",
        "test",
        "tests",
        "ci",
        "local",
        "debug",
    ];
    let bas = chemin.to_lowercase();
    let nom = bas.rsplit('/').next().unwrap_or(&bas);
    let variante = nom
        .strip_suffix(".dockerfile")
        .or_else(|| nom.strip_prefix("dockerfile."));
    // La variante peut etre composee : `Dockerfile.radicale.dev` est la
    // version de developpement d'un composant. Modoboa voyait ce fichier
    // retenu comme recette de l'application entiere.
    variante.is_some_and(|v| v.split('.').any(|morceau| VARIANTES.contains(&morceau)))
}

fn runtime_score(path: &str, recipe: &BuildRecipe) -> i32 {
    let mut score = 0;

    if path == "Dockerfile" {
        score += 3;
    }
    // Un Dockerfile de conditionnement ne sert pas a faire tourner l'app.
    const CONDITIONNEMENT: &[&str] = &["packaging", "debian", "rpm", "deb", "release", "snap"];
    let lower = path.to_lowercase();
    if CONDITIONNEMENT.iter().any(|k| lower.contains(k)) {
        score -= 4;
    }
    // Configuration d'integration continue, d'hebergeur ou de poste de
    // developpement : ces Dockerfiles assemblent un artefact deja construit
    // ailleurs. Constate sur AFFiNE, dont `.github/deployment/node/Dockerfile`
    // se contente de recopier un `dist/` et ne revele donc aucune recette.
    const HORS_PRODUIT: &[&str] = &[
        ".github",
        ".render",
        ".devcontainer",
        ".gitlab",
        ".circleci",
    ];
    if HORS_PRODUIT.iter().any(|k| lower.contains(k)) {
        score -= 5;
    }
    score -= path.matches('/').count() as i32;

    if !recipe.expose.is_empty() {
        score += 4;
    }
    if recipe
        .runtime_stage()
        .is_some_and(|s| is_runtime_image(&s.image))
    {
        score += 3;
    }
    if let Some(cmd) = recipe.start_command() {
        // `CMD ["/src/packaging/debian/build.sh"]` demarre une construction,
        // pas l'application.
        let looks_like_build = cmd.contains("build") || cmd.ends_with(".sh");
        score += if looks_like_build { -2 } else { 2 };
    }
    score
}

/// Vrai si ce nom de fichier designe reellement un Dockerfile.
///
pub use ynp_core::noms::est_un_dockerfile;

/// Vrai pour les images de base qui portent un runtime applicatif.
fn is_runtime_image(image: &str) -> bool {
    const RUNTIMES: &[&str] = &[
        "node",
        "nodejs",
        "python",
        "php",
        "golang",
        "go",
        "ruby",
        "rust",
        "openjdk",
        "eclipse-temurin",
        "nginx",
        "httpd",
        "caddy",
        "alpine",
        "debian",
        "ubuntu",
        "distroless",
        "scratch",
        "busybox",
    ];
    let short = image.rsplit('/').next().unwrap_or(image).to_lowercase();
    RUNTIMES.contains(&short.as_str())
}
