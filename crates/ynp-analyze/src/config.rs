//! Variables de configuration de l'application.
//!
//! Un `.env.example` dit ce que l'application attend pour demarrer. Classer
//! chaque variable par role — port, adresse de base, identifiants — est ce qui
//! permet de cabler la configuration sur les valeurs YunoHost (`$port`,
//! `$domain`, `$db_pwd`) sans rien deviner.
//!
//! Les sources sont lues de la plus sure a la moins sure, et la premiere qui
//! nomme une variable l'emporte. Chacune retient d'ou elle vient : une
//! proposition faite a l'humain vaut surtout par sa provenance.

use crate::knowledge;
use ynp_core::facts::{BuildRecipe, ComposeFacts, ConfigFacts, ConfigRole, ConfigVar};
use ynp_core::tree::RepoTree;

/// Fichiers d'exemple reconnus par leur chemin exact, par ordre de preference.
const CANDIDATES: &[&str] = &[
    ".env.example",
    ".env.sample",
    ".env.template",
    ".env.dist",
    "env.example",
    ".env.defaults",
    "example.env",
    // Kutt : `.example.env`, ou il fixe son JWT_SECRET. Sans lui, l'outil
    // n'avait pour source que le tableau du README, qui ne fixe rien.
    ".example.env",
    "config/.env.example",
];

/// Terminaisons d'un fichier d'exemple au format `.env`.
///
/// Une liste de chemins exacts ne suffit pas : beaucoup de projets prefixent le
/// fichier du nom de l'application. gotify publie `gotify-server.env.example`,
/// qui documente son port et sa base — deux champs que l'outil declarait
/// indeterminables faute de regarder ce fichier.
const TERMINAISONS: &[&str] = &[
    ".env.example",
    ".env.sample",
    ".env.template",
    ".env.dist",
    ".env.defaults",
    ".env.local.example",
    ".example.env",
];

pub fn detect(tree: &RepoTree, compose: Option<&ComposeFacts>) -> ConfigFacts {
    detect_avec_build(tree, compose, None)
}

pub fn detect_avec_build(
    tree: &RepoTree,
    compose: Option<&ComposeFacts>,
    build: Option<&BuildRecipe>,
) -> ConfigFacts {
    let mut facts = ConfigFacts::default();

    if let Some((path, content)) = exemple(tree) {
        facts.variables = parse_dotenv(content, &path);
        facts.example_file = Some(path);
    } else {
        facts.fichier_d_exemple = fichier_d_exemple(tree);
    }

    // Le compose complete : il porte souvent les variables reellement
    // necessaires au demarrage, la ou le .env.example est incomplet.
    if let Some(c) = compose {
        let provenance = c.path.clone();
        for svc in c.services.iter().filter(|s| s.is_app) {
            for (name, value) in &svc.environment {
                ajouter(&mut facts, name, non_empty(value), &provenance);
            }
        }
    }

    // Les `ENV` de l'etape finale du Dockerfile sont ce que l'image fixe
    // reellement au demarrage. Source moins riche que le compose — pas de
    // commentaires — mais parfois la seule.
    if let Some(b) = build {
        let provenance = b.dockerfile_path.clone();
        for (name, value) in &b.env {
            ajouter(&mut facts, name, non_empty(value), &provenance);
        }
    }

    // Dernier recours : les blocs de code du README. C'est la que la plupart
    // des projets montrent comment on lance leur application, et donc sous
    // quel nom elle attend son port ou sa base — quand ils n'ont ni fichier
    // d'exemple, ni compose, ni `ENV` dans leur Dockerfile.
    if let Some((chemin, texte)) = tree.first_text(&["README.md", "readme.md", "README"]) {
        // Un bloc de code montre un deploiement : il fixe ses variables comme
        // un fichier d'exemple. Un tableau documente toutes les options
        // possibles : il ne fixe rien. La provenance le dit, puisqu'elle est
        // montree a l'utilisateur.
        let trouvees = variables_des_blocs_de_code(texte)
            .into_iter()
            .map(|(n, v)| (n, v, false, format!("{chemin}, bloc de code")))
            .chain(
                variables_des_tableaux(texte)
                    .into_iter()
                    .map(|(n, v)| (n, v, true, format!("{chemin}, tableau"))),
            );
        for (nom, valeur, facultative, provenance) in trouvees {
            // Un bloc de code est une source faible : on n'en retient que ce
            // dont on connait deja le sens. Le reste irait grossir la
            // configuration du paquet de variables sans role — `VERSION`,
            // `PWD`, le nom d'une image — au lieu de l'eclairer.
            if knowledge::get().role_of(&nom) == ConfigRole::Other {
                continue;
            }
            if facultative {
                ajouter_facultative(&mut facts, &nom, valeur, &provenance);
            } else {
                ajouter(&mut facts, &nom, valeur, &provenance);
            }
        }
    }

    facts
}

/// Les variables qu'un cadre de configuration lit sous un prefixe, d'apres
/// les cles du fichier de configuration d'exemple.
///
/// continuwuity se configure par un fichier TOML (`server_name`, `port`,
/// `database_path` dans `[global]`), que son code fusionne avec
/// l'environnement sous `CONTINUWUITY_` : `CONTINUWUITY_SERVER_NAME` fixe
/// son nom de domaine sans fichier a ecrire. Seules les cles dont le role est
/// connu sont retenues.
pub fn ajouter_par_prefixe(facts: &mut ConfigFacts, tree: &RepoTree, depot: &str) {
    let norme = |s: &str| s.to_uppercase().replace(['-', '.', '_'], "");
    let prefixes: Vec<&String> = tree.prefixes_env().iter().collect();
    let prefixe = match prefixes.as_slice() {
        [] => return,
        [seul] => (*seul).clone(),
        plusieurs => match plusieurs.iter().find(|p| norme(p) == norme(depot)) {
            Some(p) => (*p).clone(),
            None => return,
        },
    };
    let Some((chemin, texte)) = exemple_de_configuration(tree) else {
        return;
    };
    for (cle, valeur, commentee) in cles_de_premier_niveau(&chemin, texte) {
        let nom = format!("{prefixe}{}", cle.to_ascii_uppercase());
        // Le port, prefixe du nom de l'application, est reconnu ensuite.
        if knowledge::get().role_of(&nom) == ConfigRole::Other && !nom.ends_with("_PORT") {
            continue;
        }
        let provenance = format!("{chemin}, sous {prefixe}");
        // Une cle commentee documente une option, elle ne la fixe pas.
        if commentee {
            ajouter_facultative(facts, &nom, valeur, &provenance);
        } else {
            ajouter(facts, &nom, valeur, &provenance);
        }
    }
}

/// Un fichier de configuration d'exemple dont le pendant reel n'est pas dans
/// le depot : `config/config.example.toml` donne `config/config.toml`,
/// `settings.yml.sample` donne `settings.yml`. Pas un fichier de deploiement
/// (compose, Helm), ni un fichier ou YunoHost verrait des jetons a remplacer.
pub fn fichier_d_exemple(tree: &RepoTree) -> Option<ynp_core::facts::FichierDExemple> {
    const MARQUES: &[&str] = &[
        ".example",
        ".sample",
        ".dist",
        "-example",
        "_example",
        ".template",
    ];
    const EXTENSIONS: &[&str] = &[".toml", ".yaml", ".yml", ".json", ".ini", ".conf"];
    const ECARTES: &[&str] = &[
        "compose", "docker", "helm", "values", "k8s", "kube", "nginx", "caddy", "apache",
    ];
    let jeton = regex::Regex::new(r"__[A-Z0-9_]+__").expect("motif valide");
    let mut candidats: Vec<(String, String)> = tree
        .paths()
        .into_iter()
        .filter(|p| p.matches('/').count() <= 2)
        .filter(|p| {
            let bas = p.to_lowercase();
            ![
                "docs/",
                "doc/",
                "examples/",
                "example/",
                "test/",
                "tests/",
                ".github/",
                "charts/",
                "deploy/",
            ]
            .iter()
            .any(|d| bas.starts_with(d) || bas.contains(&format!("/{d}")))
        })
        .filter_map(|p| {
            let (dossier, nom) = p.rsplit_once('/').map_or(("", p.as_str()), |(d, n)| (d, n));
            let bas = nom.to_lowercase();
            if ECARTES.iter().any(|e| bas.contains(e)) {
                return None;
            }
            // `nom.example.ext` ou `nom.ext.example`.
            let cible = MARQUES.iter().find_map(|m| {
                EXTENSIONS.iter().find_map(|e| {
                    if bas.ends_with(&format!("{m}{e}")) {
                        Some(format!("{}{e}", &nom[..nom.len() - m.len() - e.len()]))
                    } else if bas.ends_with(&format!("{e}{m}")) {
                        Some(nom[..nom.len() - m.len()].to_string())
                    } else {
                        None
                    }
                })
            })?;
            let cible = if dossier.is_empty() {
                cible
            } else {
                format!("{dossier}/{cible}")
            };
            Some((p.clone(), cible))
        })
        .filter(|(_, cible)| !tree.has(cible))
        .collect();
    candidats.sort_by_key(|(p, _)| (p.matches('/').count(), p.len()));
    let (exemple, cible) = candidats.into_iter().next()?;
    let texte = tree.text(&exemple)?;
    if texte.len() > 64 * 1024 || jeton.is_match(texte) {
        return None;
    }
    Some(ynp_core::facts::FichierDExemple {
        exemple,
        cible,
        texte: texte.to_string(),
    })
}

/// Le fichier de configuration d'exemple : `conduwuit-example.toml`,
/// `config.example.yaml`. Le plus proche de la racine l'emporte.
fn exemple_de_configuration(tree: &RepoTree) -> Option<(String, &str)> {
    let mut candidats: Vec<String> = tree
        .paths()
        .into_iter()
        .filter(|p| {
            let l = p.to_lowercase();
            let nom = l.rsplit('/').next().unwrap_or(&l).to_string();
            p.matches('/').count() <= 1
                && (nom.contains("example") || nom.contains("sample"))
                && (l.ends_with(".toml") || l.ends_with(".yaml") || l.ends_with(".yml"))
        })
        .collect();
    candidats.sort_by_key(|p| (p.matches('/').count(), p.len()));
    let p = candidats.into_iter().next()?;
    let t = tree.text(&p)?;
    Some((p, t))
}

/// Les cles de premier niveau d'un TOML ou d'un YAML, avec leur valeur
/// d'exemple. En TOML, a defaut de cles hors de toute table, celles de la
/// premiere table simple (`[global]`), que Figment lit sans prefixe de table.
fn cles_de_premier_niveau(chemin: &str, texte: &str) -> Vec<(String, Option<String>, bool)> {
    let cle = |l: &str, sep: char| -> Option<(String, Option<String>, bool)> {
        let commentee = l.starts_with('#');
        let l = l.trim_start_matches('#').trim_start();
        let (k, v) = l.split_once(sep)?;
        let k = k.trim();
        if k.is_empty()
            || !k.starts_with(|c: char| c.is_ascii_lowercase())
            || !k
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return None;
        }
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        let v =
            (!v.is_empty() && !v.starts_with('[') && !v.starts_with('{')).then(|| v.to_string());
        Some((k.to_string(), v, commentee))
    };
    let mut out: Vec<(String, Option<String>, bool)> = Vec::new();
    let mut vu = std::collections::BTreeSet::new();
    if chemin.ends_with(".toml") {
        let mut table: Option<String> = None;
        let mut premiere: Option<String> = None;
        let (mut hors_table, mut dans_premiere) = (Vec::new(), Vec::new());
        for ligne in texte.lines() {
            let l = ligne.trim();
            if let Some(t) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                table = Some(t.trim().to_string());
                if premiere.is_none() && !t.contains('.') && !t.starts_with('[') {
                    premiere = table.clone();
                }
                continue;
            }
            if let Some(c) = cle(l, '=') {
                match &table {
                    None => hors_table.push(c),
                    Some(t) if Some(t) == premiere.as_ref() => dans_premiere.push(c),
                    _ => {}
                }
            }
        }
        let choisies = if hors_table.is_empty() {
            dans_premiere
        } else {
            hors_table
        };
        for c in choisies {
            if vu.insert(c.0.clone()) {
                out.push(c);
            }
        }
    } else {
        for ligne in texte.lines() {
            if ligne.starts_with(' ') || ligne.starts_with('\t') {
                continue;
            }
            if let Some(c) = cle(ligne, ':') {
                if vu.insert(c.0.clone()) {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// Ajoute les variables que le code de l'application lit.
///
/// Source la plus sure sur le fond — c'est ce que l'application lit
/// reellement — mais la derniere consultee : un fichier d'exemple ou un
/// compose disent en plus la valeur attendue, et un nom declare la-bas
/// l'emporte toujours. Comme pour le README, on ne retient que les noms dont
/// le role est connu, ou prefixes du nom du depot — que la regle suivante
/// sait reconnaitre ; le reste irait grossir la configuration du paquet.
pub fn ajouter_les_lectures(facts: &mut ConfigFacts, tree: &RepoTree, depot: &str) {
    let prefixe = format!("{}_", depot.to_uppercase().replace(['-', '.'], "_"));
    for (nom, chemin) in tree.lectures() {
        let connu = knowledge::get().role_of(nom) != ConfigRole::Other;
        let defaut = tree
            .defauts_lus()
            .get(nom)
            .filter(|v| !v.is_empty())
            .cloned();
        // Une adresse d'ecoute comme valeur par defaut (`:8000`) designe le
        // port de l'application, quel que soit son prefixe.
        let ecoute = defaut.as_deref().is_some_and(est_une_adresse_d_ecoute);
        if !connu && !ecoute && !nom.starts_with(&prefixe) {
            continue;
        }
        ajouter_facultative(facts, nom, defaut, &format!("lu par le code, {chemin}"));
    }
}

/// `:8000`, `0.0.0.0:8080`, `127.0.0.1:3000`, `[::]:80` : une adresse ou
/// ecouter.
pub fn est_une_adresse_d_ecoute(v: &str) -> bool {
    let Some((hote, port)) = v.rsplit_once(':') else {
        return false;
    };
    !port.is_empty()
        && port.len() <= 5
        && port.chars().all(|c| c.is_ascii_digit())
        && matches!(
            hote,
            "" | "0.0.0.0" | "127.0.0.1" | "localhost" | "[::]" | "::" | "*"
        )
}

/// Reconnait le port d'une application dans une variable prefixee de son nom.
///
/// Beaucoup d'applications rangent leurs reglages sous leur propre prefixe :
/// `TIMETAGGER_BIND`, `GITEA_HTTP_PORT`. La table des noms connus ne peut pas
/// les contenir tous, et la simple terminaison `_PORT` ne suffit pas :
/// `DB_PORT`, `SMTP_PORT`, `REDIS_PORT` designent le port d'un autre service.
/// Le prefixe du depot, lui, leve l'ambiguite — c'est l'application qui parle
/// de son propre port.
///
/// On n'intervient que si aucune variable n'a deja le role : une declaration
/// explicite l'emporte toujours.
pub fn reconnaitre_le_port_prefixe(facts: &mut ConfigFacts, depot: &str) {
    if facts.get(ConfigRole::Port).is_some() {
        return;
    }
    let prefixe = depot.to_uppercase().replace(['-', '.'], "_");
    const SUFFIXES: &[&str] = &[
        "_PORT",
        "_HTTP_PORT",
        "_LISTEN_PORT",
        "_SERVER_PORT",
        "_BIND",
        "_BIND_ADDR",
        "_BIND_ADDRESS",
        "_LISTEN",
        "_LISTEN_ADDR",
        "_LISTEN_ADDRESS",
        "_ADDR",
    ];
    for v in facts.variables.iter_mut() {
        let Some(reste) = v.name.strip_prefix(&prefixe) else {
            continue;
        };
        if SUFFIXES.contains(&reste) {
            v.role = ConfigRole::Port;
            return;
        }
    }
    // Faute de prefixe reconnu : la seule variable d'ecoute dont la valeur
    // par defaut est une adresse. counter.dev ecoute sur WEBSTATS_BIND,
    // `:8000` par defaut.
    let ecoutes: Vec<usize> = facts
        .variables
        .iter()
        .enumerate()
        .filter(|(_, v)| {
            v.role == ConfigRole::Other
                && v.default.as_deref().is_some_and(est_une_adresse_d_ecoute)
                && ["BIND", "ADDR", "ADDRESS", "LISTEN"]
                    .iter()
                    .any(|s| v.name.ends_with(s))
        })
        .map(|(i, _)| i)
        .collect();
    if let [i] = ecoutes.as_slice() {
        facts.variables[*i].role = ConfigRole::Port;
    }
}

/// Les affectations de variables que portent les blocs de code d'un document.
///
/// Un README est de la prose, que l'outil s'interdit d'interpreter. Ses blocs
/// de code, eux, ne le sont pas : `docker run -e PORT=8191 …` enonce un nom de
/// variable aussi nettement qu'un `.env.example`. C'est le champ qui bloquait
/// le plus souvent — le nom sous lequel une application attend son port ou sa
/// base — et beaucoup de projets ne le declarent nulle part ailleurs.
///
/// Trois formes, toutes issues de ce qu'on colle dans un terminal ou dans un
/// compose : `-e NOM=valeur`, `NOM=valeur` en debut de ligne, et `- NOM=valeur`
/// des listes d'environnement.
pub fn variables_des_blocs_de_code(texte: &str) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut dans_un_bloc = false;

    for ligne in texte.lines() {
        let l = ligne.trim();
        if l.starts_with("```") || l.starts_with("~~~") {
            dans_un_bloc = !dans_un_bloc;
            continue;
        }
        if !dans_un_bloc {
            continue;
        }

        let mut mots = l.split_whitespace().peekable();
        while let Some(mot) = mots.next() {
            // `-e NOM=valeur` et `--env NOM=valeur` : le nom peut suivre le
            // drapeau ou lui etre accole.
            let brut = match mot {
                "-e" | "--env" => match mots.next() {
                    Some(suivant) => suivant,
                    None => break,
                },
                _ => mot.trim_start_matches('-').trim_start_matches("export "),
            };
            let Some((nom, valeur)) = brut.split_once('=') else {
                continue;
            };
            if !is_env_name(nom) {
                continue;
            }
            if out.iter().any(|(n, _)| n == nom) {
                continue;
            }
            out.push((nom.to_string(), non_empty(valeur)));
        }
    }
    out
}

/// Les variables qu'un tableau de documentation declare.
///
/// C'est la forme la plus repandue des sections « variables d'environnement »
/// d'un README : une ligne par reglage, son nom, sa valeur par defaut, et ce
/// qu'il fait. FlareSolverr y annonce son `PORT`, que rien d'autre dans son
/// depot ne nomme — ni fichier d'exemple, ni compose, ni `ENV` de Dockerfile.
///
/// Un tableau n'est pas de la prose : ses cellules sont delimitees, et la
/// premiere porte le nom. On ne retient d'ailleurs, plus loin, que les noms
/// dont on connait deja le sens, ce qui ecarte les tableaux qui listent tout
/// autre chose.
pub fn variables_des_tableaux(texte: &str) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut dans_un_bloc = false;

    for ligne in texte.lines() {
        let l = ligne.trim();
        if l.starts_with("```") || l.starts_with("~~~") {
            dans_un_bloc = !dans_un_bloc;
            continue;
        }
        // Un tableau dessine dans un bloc de code est un exemple d'affichage,
        // pas une declaration.
        if dans_un_bloc || !l.starts_with('|') {
            continue;
        }

        let cellules: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
        let Some(nom) = cellules.first() else {
            continue;
        };
        // Les noms sont parfois mis en valeur : `| `PORT` |`, `| **PORT** |`.
        let nom = nom.trim_matches('`').trim_matches('*').trim();
        if !is_env_name(nom) || out.iter().any(|(n, _): &(String, _)| n == nom) {
            continue;
        }

        // La deuxieme cellule porte la valeur par defaut quand elle en est
        // une : un mot, pas une phrase d'explication.
        let defaut = cellules
            .get(1)
            .map(|c| c.trim_matches('`').trim_matches('*').trim())
            .filter(|c| !c.is_empty() && !c.contains(' ') && c.len() <= 40)
            .and_then(non_empty);
        out.push((nom.to_string(), defaut));
    }
    out
}

/// Le fichier d'exemple : chemin connu d'abord, puis terminaison reconnue.
///
/// A terminaison egale, le chemin le plus court gagne : `app.env.example` a la
/// racine est plus representatif que `tests/fixtures/x.env.example`.
fn exemple(tree: &RepoTree) -> Option<(String, &str)> {
    if let Some((p, c)) = tree.first_text(CANDIDATES) {
        return Some((p, c));
    }
    let mut trouves: Vec<String> = tree
        .paths()
        .into_iter()
        .filter(|p| {
            let bas = p.to_lowercase();
            TERMINAISONS.iter().any(|t| bas.ends_with(t))
        })
        .collect();
    trouves.sort_by_key(|p| (p.matches('/').count(), p.len(), p.clone()));
    if let Some(p) = trouves.into_iter().next() {
        let c = tree.text(&p)?;
        return Some((p, c));
    }
    // Un `.env` versionne tient lieu d'exemple : c'est la convention de
    // Symfony, qui y range ses valeurs par defaut (novosga, domain-watchdog :
    // `DATABASE_URL="postgresql://…?serverVersion=16&charset=utf8"`). Le
    // fichier de configuration du paquet le remplace : il doit tout en
    // reprendre.
    tree.first_text(&[".env"]).filter(|(_, c)| {
        c.lines()
            .any(|l| l.contains('=') && !l.trim_start().starts_with('#'))
    })
}

/// Ajoute une variable si aucune source plus sure ne l'a deja nommee.
fn ajouter(facts: &mut ConfigFacts, name: &str, valeur: Option<String>, source: &str) {
    if facts.variables.iter().any(|v| v.name == name) {
        return;
    }
    facts.variables.push(make_var(name, valeur, source));
}

/// Ajoute une variable que le projet mentionne sans la fixer.
fn ajouter_facultative(facts: &mut ConfigFacts, name: &str, valeur: Option<String>, source: &str) {
    if facts.variables.iter().any(|v| v.name == name) {
        return;
    }
    let mut v = make_var(name, valeur, source);
    v.facultative = true;
    facts.variables.push(v);
}

/// Analyse un fichier au format `.env`.
///
/// Les lignes commentees sont lues elles aussi : un `# PORT=3000` documente la
/// valeur par defaut attendue, information qu'il serait dommage de perdre.
/// Elles ne sont retenues que si la variable n'est pas deja definie.
fn parse_dotenv(content: &str, source: &str) -> Vec<ConfigVar> {
    let mut out: Vec<ConfigVar> = Vec::new();
    let mut commented: Vec<ConfigVar> = Vec::new();
    // Les commentaires qui precedent une affectation : ils disent souvent
    // comment tirer un secret (`openssl rand -base64 32`, chez benevoles).
    let mut notes: Vec<&str> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            notes.clear();
            continue;
        }
        let (is_comment, body) = match trimmed.strip_prefix('#') {
            Some(rest) => (true, rest.trim()),
            None => (false, trimmed),
        };

        let Some((name, value)) = body.split_once('=') else {
            if is_comment {
                notes.push(body);
            }
            continue;
        };
        let name = name.trim().trim_start_matches("export ").trim();
        if name.is_empty() || !is_env_name(name) {
            if is_comment {
                notes.push(body);
            }
            continue;
        }

        let mut var = make_var(name, non_empty(value.trim()), source);
        if var.secret {
            var.format_secret = notes
                .iter()
                .copied()
                .chain(std::iter::once(value))
                .find_map(format_de_secret);
        }
        notes.clear();
        if is_comment {
            var.facultative = true;
            commented.push(var);
        } else if !out.iter().any(|v| v.name == var.name) {
            out.push(var);
        }
    }

    for var in commented {
        if !out.iter().any(|v| v.name == var.name) {
            out.push(var);
        }
    }
    out
}

/// `openssl rand -base64 32` → `base64:32` ; `openssl rand -hex 16` → `hex:16`.
fn format_de_secret(texte: &str) -> Option<String> {
    let mots: Vec<&str> = texte.split_whitespace().collect();
    mots.windows(4).find_map(|m| match m {
        ["openssl", "rand", f @ ("-base64" | "-hex"), n] => {
            let n: String = n.chars().take_while(|c| c.is_ascii_digit()).collect();
            let n: u32 = n.parse().ok().filter(|n| (8..=128).contains(n))?;
            Some(format!("{}:{n}", f.trim_start_matches('-')))
        }
        _ => None,
    })
}

fn make_var(name: &str, default: Option<String>, source: &str) -> ConfigVar {
    let k = knowledge::get();
    ConfigVar {
        role: k.role_of(name),
        secret: k.is_secret(name),
        // Ne jamais reprendre la valeur d'un secret : celle d'un fichier
        // d'exemple est publique par construction.
        default: if k.is_secret(name) { None } else { default },
        name: name.to_string(),
        source: source.to_string(),
        facultative: false,
        format_secret: None,
    }
}

fn non_empty(v: &str) -> Option<String> {
    let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Un nom de variable d'environnement : majuscules, chiffres, tirets bas.
/// Ce filtre ecarte les fragments de phrase d'un commentaire en prose.
fn is_env_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.chars().any(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::ConfigRole;

    fn var(nom: &str, defaut: Option<&str>) -> ConfigVar {
        make_var(nom, defaut.map(String::from), "test")
    }

    #[test]
    fn un_prefixe_d_environnement_fait_lire_les_cles_du_fichier_d_exemple() {
        // Cas de continuwuity : Figment et un TOML documente.
        let mut tree = RepoTree::new();
        tree.insert(
            "src/core/config/mod.rs",
            "let f = Figment::new()\n.merge(Env::prefixed(\"CONDUWUIT_\").global())\n\
             .merge(Env::prefixed(\"CONTINUWUITY_\").global());\n",
        );
        tree.insert(
            "conduwuit-example.toml",
            "[global]\n#server_name =\n#port = 8008\n#database_path =\n#turn_secret =\n\
             #new_user_displayname_suffix = \"x\"\n[global.tls]\n#certs =\n",
        );
        let mut facts = ConfigFacts::default();
        ajouter_par_prefixe(&mut facts, &tree, "continuwuity");
        let noms: Vec<&str> = facts.variables.iter().map(|v| v.name.as_str()).collect();
        assert!(noms.contains(&"CONTINUWUITY_SERVER_NAME"));
        assert!(noms.contains(&"CONTINUWUITY_PORT"));
        assert!(noms.contains(&"CONTINUWUITY_DATABASE_PATH"));
        assert!(!noms.contains(&"CONTINUWUITY_NEW_USER_DISPLAYNAME_SUFFIX"));
        assert!(!noms.iter().any(|n| n.contains("CERTS")));
    }

    #[test]
    fn le_commentaire_d_un_secret_donne_son_format() {
        // Cas de benevoles.
        let v = parse_dotenv(
            "# 32 octets en base64 : openssl rand -base64 32. Ne jamais la perdre\n\
             TOKEN_ENCRYPTION_KEY=\"\"\n\nCRON_SECRET=\"\"\n\
             SESSION_SECRET= # openssl rand -hex 32\n",
            ".env.example",
        );
        let f = |n: &str| {
            v.iter()
                .find(|x| x.name == n)
                .unwrap()
                .format_secret
                .clone()
        };
        assert_eq!(f("TOKEN_ENCRYPTION_KEY").as_deref(), Some("base64:32"));
        assert_eq!(f("CRON_SECRET"), None);
        assert_eq!(f("SESSION_SECRET").as_deref(), Some("hex:32"));
    }

    #[test]
    fn le_code_dit_sous_quel_nom_l_application_attend_son_port() {
        // Ni fichier d'exemple, ni compose, ni README : seul le code le dit.
        let arbre = RepoTree::from_pairs([
            ("src/server.js", "app.listen(process.env.PORT || 3000)"),
            ("test/server.test.js", "process.env.TEST_PORT"),
        ]);
        let mut f = ConfigFacts::default();
        ajouter_les_lectures(&mut f, &arbre, "monappli");
        let v = f.get(ConfigRole::Port).unwrap();
        assert_eq!(v.name, "PORT");
        assert!(v.source.contains("src/server.js"), "{}", v.source);
    }

    #[test]
    fn une_declaration_explicite_passe_devant_le_code() {
        let arbre = RepoTree::from_pairs([("main.go", "os.Getenv(\"PORT\")")]);
        let mut f = ConfigFacts {
            variables: vec![var("HTTP_PORT", Some("8080"))],
            ..Default::default()
        };
        ajouter_les_lectures(&mut f, &arbre, "x");
        assert_eq!(f.get(ConfigRole::Port).unwrap().name, "HTTP_PORT");
    }

    #[test]
    fn le_prefixe_du_depot_designe_le_port_de_l_application() {
        // Cas de timetagger : `TIMETAGGER_BIND=0.0.0.0:80`.
        let mut f = ConfigFacts {
            variables: vec![
                var("TIMETAGGER_DATADIR", None),
                var("TIMETAGGER_BIND", Some("0.0.0.0:80")),
            ],
            ..Default::default()
        };
        reconnaitre_le_port_prefixe(&mut f, "timetagger");
        assert_eq!(f.get(ConfigRole::Port).unwrap().name, "TIMETAGGER_BIND");
    }

    #[test]
    fn le_port_d_un_autre_service_n_est_pas_celui_de_l_application() {
        let mut f = ConfigFacts {
            variables: vec![var("DB_PORT", Some("5432")), var("SMTP_PORT", Some("25"))],
            ..Default::default()
        };
        reconnaitre_le_port_prefixe(&mut f, "monappli");
        assert!(f.get(ConfigRole::Port).is_none());
    }

    #[test]
    fn un_tableau_de_documentation_nomme_les_variables() {
        // Cas de FlareSolverr : son PORT n'est declare nulle part ailleurs
        // que dans le tableau des variables d'environnement du README.
        let md = "## Environment variables\n\n                  | Name | Default | Description |\n                  |------|---------|-------------|\n                  | `LOG_LEVEL` | info | Verbosity of the logs. |\n                  | PORT | 8191 | Listening port. You don't need to change this. |\n";
        let v = variables_des_tableaux(md);
        assert_eq!(
            v.iter().find(|(n, _)| n == "PORT").unwrap().1.as_deref(),
            Some("8191")
        );
        assert!(v.iter().any(|(n, _)| n == "LOG_LEVEL"), "{v:?}");
    }

    #[test]
    fn une_phrase_en_deuxieme_cellule_n_est_pas_une_valeur_par_defaut() {
        let md = "| PORT | none, the app picks one | ... |\n";
        assert_eq!(variables_des_tableaux(md)[0].1, None);
    }

    #[test]
    fn un_tableau_dessine_dans_un_bloc_de_code_est_un_affichage() {
        let md = "```\n| PORT | 8191 |\n```\n";
        assert!(variables_des_tableaux(md).is_empty());
    }

    #[test]
    fn un_docker_run_de_readme_nomme_les_variables() {
        // Cas de FlareSolverr : ni .env.example, ni ENV dans le Dockerfile.
        // Son README montre pourtant la commande de lancement.
        let md = "Lancez-le ainsi :\n\n                  ```bash\n                  docker run -d --name flaresolverr -e LOG_LEVEL=info \\\n                    -e PORT=8191 -p 8191:8191 flaresolverr:latest\n                  ```\n";
        let v = variables_des_blocs_de_code(md);
        assert_eq!(
            v.iter().find(|(n, _)| n == "PORT").unwrap().1.as_deref(),
            Some("8191")
        );
        assert!(v.iter().any(|(n, _)| n == "LOG_LEVEL"));
    }

    #[test]
    fn une_liste_d_environnement_de_compose_est_lue() {
        let md = "```yaml\nservices:\n  app:\n    environment:\n                        - DATABASE_URL=postgres://u:p@db/app\n      - PORT=3000\n```\n";
        let v = variables_des_blocs_de_code(md);
        assert!(v.iter().any(|(n, _)| n == "DATABASE_URL"), "{v:?}");
        assert!(v.iter().any(|(n, _)| n == "PORT"), "{v:?}");
    }

    #[test]
    fn la_prose_hors_bloc_est_ignoree() {
        // Tout l'interet de se limiter aux blocs : la phrase dit la meme
        // chose, mais l'interpreter serait deviner.
        let md = "Definissez PORT=8191 dans votre environnement.\n";
        assert!(variables_des_blocs_de_code(md).is_empty());
    }

    #[test]
    fn un_bloc_referme_arrete_la_lecture() {
        let md = "```\nPORT=1\n```\n\nPuis OTHER_VAR=2 en prose.\n";
        let v = variables_des_blocs_de_code(md);
        assert_eq!(v.len(), 1, "{v:?}");
    }

    const ENV: &str = r#"
# Configuration de l'application
PORT=3000
DATABASE_URL=postgresql://user:pass@localhost:5432/app
NEXTAUTH_SECRET=changeme-please
export LOG_LEVEL=info

# Optionnel : decommenter pour activer
# SMTP_HOST=smtp.example.com

ceci est une phrase = pas une variable
"#;

    fn vars(tree: &RepoTree) -> Vec<ConfigVar> {
        detect(tree, None).variables
    }

    fn tree() -> RepoTree {
        RepoTree::from_pairs([(".env.example", ENV)])
    }

    #[test]
    fn les_variables_sont_lues_avec_leur_role() {
        let v = vars(&tree());
        let by = |n: &str| v.iter().find(|x| x.name == n).cloned().unwrap();

        assert_eq!(by("PORT").role, ConfigRole::Port);
        assert_eq!(by("PORT").default.as_deref(), Some("3000"));
        assert_eq!(by("DATABASE_URL").role, ConfigRole::DatabaseUrl);
        assert_eq!(by("LOG_LEVEL").role, ConfigRole::LogLevel);
    }

    #[test]
    fn la_valeur_d_un_secret_n_est_jamais_reprise() {
        // Celle d'un fichier d'exemple est publique par construction.
        let v = vars(&tree());
        let secret = v.iter().find(|x| x.name == "NEXTAUTH_SECRET").unwrap();
        assert!(secret.secret);
        assert_eq!(
            secret.default, None,
            "la valeur d'exemple ne doit pas fuiter"
        );
    }

    #[test]
    fn une_variable_commentee_documente_quand_meme_un_defaut() {
        let v = vars(&tree());
        assert!(v.iter().any(|x| x.name == "SMTP_HOST"));
    }

    #[test]
    fn la_prose_d_un_commentaire_n_est_pas_prise_pour_une_variable() {
        let v = vars(&tree());
        assert!(
            v.iter().all(|x| is_env_name(&x.name)),
            "variables retenues : {:?}",
            v.iter().map(|x| &x.name).collect::<Vec<_>>()
        );
        assert!(!v.iter().any(|x| x.name.contains("phrase")));
    }

    #[test]
    fn le_prefixe_export_est_retire() {
        let v = vars(&tree());
        assert!(v.iter().any(|x| x.name == "LOG_LEVEL"));
        assert!(!v.iter().any(|x| x.name.contains("export")));
    }

    #[test]
    fn le_compose_complete_les_variables_absentes_du_fichier_d_exemple() {
        let compose_yml = r#"
services:
  app:
    build: .
    environment:
      - PORT=8080
      - EXTRA_SETTING=oui
"#;
        let compose = ynp_dockerfile::parse_compose("c.yml", compose_yml, None).unwrap();
        let facts = detect(&tree(), Some(&compose));

        // PORT existe deja dans le .env.example : sa valeur ne doit pas etre ecrasee.
        let port = facts.variables.iter().find(|v| v.name == "PORT").unwrap();
        assert_eq!(port.default.as_deref(), Some("3000"));
        assert!(facts.variables.iter().any(|v| v.name == "EXTRA_SETTING"));
    }

    #[test]
    fn un_depot_sans_fichier_d_exemple_ne_produit_rien() {
        let f = detect(&RepoTree::from_pairs([("README.md", "x")]), None);
        assert!(f.example_file.is_none());
        assert!(f.variables.is_empty());
    }
}

#[cfg(test)]
mod readme_fixe_ou_documente {
    use super::*;

    #[test]
    fn un_bloc_de_code_fixe_un_tableau_documente() {
        // Cas de Kutt : `docker run -e JWT_SECRET=…` montre un deploiement.
        let arbre = RepoTree::from_pairs([(
            "README.md",
            "```sh\ndocker run -e JWT_SECRET=changeme kutt/kutt\n```\n\n\
             | Variable | Default | Description |\n|---|---|---|\n\
             | MAIL_PASSWORD | - | Optional SMTP password |\n",
        )]);
        let f = detect(&arbre, None);
        let v = |n: &str| {
            f.variables
                .iter()
                .find(|v| v.name == n)
                .unwrap()
                .facultative
        };
        assert!(!v("JWT_SECRET"), "un bloc de code fixe sa variable");
        assert!(v("MAIL_PASSWORD"), "un tableau ne fait que documenter");
    }
}
