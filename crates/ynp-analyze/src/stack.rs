//! Detection de la technologie et de la version de runtime.
//!
//! Deux sources, par ordre de fiabilite decroissante : l'image de base du
//! Dockerfile — qui dit exactement ce sur quoi l'upstream fait tourner son
//! application — puis les fichiers de projet, qui expriment une contrainte
//! (`>=18`) plutot qu'un choix.

use crate::knowledge;
use serde_json::Value;
use ynp_core::facts::{BuildRecipe, CommandeDeclaree, PisteTechnologie, StackFacts, Technology};
use ynp_core::tree::RepoTree;

pub fn detect(tree: &RepoTree, build: Option<&BuildRecipe>) -> StackFacts {
    let mut facts = StackFacts::default();

    // L'image de runtime prime : c'est un fait, pas une contrainte.
    if let Some(stage) = build.and_then(|b| b.runtime_stage()) {
        if let Some((tech, version)) = from_base_image(&stage.image, stage.tag.as_deref()) {
            facts.primary = tech;
            facts.runtime_version = version;
        }
    }

    // Les fichiers de projet completent, et prennent la main si le Dockerfile
    // n'a rien appris (image `scratch`, base inconnue, absence de Dockerfile).
    let from_files = from_project_files(tree);
    if facts.primary == Technology::Unknown {
        facts.primary = from_files.primary;
    }
    // Un tag d'image peut etre plus vague que le fichier de projet : `golang:1`
    // face a `go 1.26.0`. On garde la version la plus precise des deux.
    facts.runtime_version = more_precise(facts.runtime_version.take(), from_files.runtime_version);
    facts.package_managers = from_files.package_managers;
    facts.has_lockfile = from_files.has_lockfile;
    facts.build_script = from_files.build_script;
    facts.native_deps = native_deps(tree);
    facts.contrainte_php = json(tree, "composer.json")
        .and_then(|c| c.get("require")?.get("php")?.as_str().map(str::to_string));
    if let Some(Value::Object(requis)) =
        json(tree, "composer.json").and_then(|c| c.get("require").cloned())
    {
        facts.dependances_php = requis.keys().cloned().collect();
        facts.extensions_php = requis
            .keys()
            .filter_map(|k| k.strip_prefix("ext-"))
            .map(|e| e.to_lowercase())
            .collect();
    }
    if let Some(b) = build {
        for etape in &b.build_steps {
            if let Some((_, reste)) = etape.split_once("docker-php-ext-install") {
                for mot in reste
                    .split_whitespace()
                    .take_while(|m| *m != "&&" && *m != ";")
                {
                    let mot = mot.to_lowercase();
                    if !mot.starts_with('-') && !facts.extensions_php.contains(&mot) {
                        facts.extensions_php.push(mot);
                    }
                }
            }
        }
    }
    if let Some(texte) = tree.text("pyproject.toml") {
        for section in ["[project.scripts]", "[tool.poetry.scripts]"] {
            facts.scripts_python.extend(cles_de_section(texte, section));
        }
    }
    if let Some(pkg) = json(tree, "package.json") {
        if let Some(Value::Object(scripts)) = pkg.get("scripts") {
            facts.scripts_npm = scripts
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
        }
        for champ in ["dependencies", "devDependencies"] {
            if let Some(Value::Object(d)) = pkg.get(champ) {
                facts.dependances_npm.extend(d.keys().cloned());
            }
        }
    }

    facts.commande_declaree =
        commande_declaree(tree).map(|(commande, ou)| CommandeDeclaree { commande, ou });
    facts.processus_du_procfile = processus_du_procfile(tree);

    // La racine n'a rien appris. Soit elle annonce un langage hors perimetre
    // — et c'est lui qu'il faut nommer — soit on regarde plus bas, pour
    // proposer plutot que de demander les mains vides.
    if facts.primary == Technology::Unknown {
        facts.langage_hors_perimetre = langage_hors_perimetre(tree)
            .or_else(|| {
                build
                    .and_then(|b| b.runtime_stage())
                    .and_then(|s| langage_hors_perimetre_de_l_image(&s.image))
            })
            .map(str::to_string);
        if facts.langage_hors_perimetre.is_none() {
            facts.pistes = pistes_de_technologie(tree)
                .into_iter()
                .map(|(technologie, ou)| PisteTechnologie { technologie, ou })
                .collect();
        }
    }

    facts
}

/// Retient la version la plus precise, c'est-a-dire celle qui porte le plus de
/// composantes. A precision egale, celle de l'image de base fait foi : c'est un
/// fait, quand le fichier de projet n'exprime qu'une contrainte.
fn more_precise(from_image: Option<String>, from_files: Option<String>) -> Option<String> {
    match (from_image, from_files) {
        (Some(image), Some(files)) => {
            let parts = |v: &str| v.matches('.').count();
            if parts(&files) > parts(&image) {
                Some(files)
            } else {
                Some(image)
            }
        }
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}

/// Technologie et version deduites d'une image de base Docker.
fn from_base_image(image: &str, tag: Option<&str>) -> Option<(Technology, Option<String>)> {
    let short = image.rsplit('/').next().unwrap_or(image).to_lowercase();
    let version = tag.and_then(first_version);

    let tech = match short.as_str() {
        "node" | "nodejs" => Technology::NodeJs,
        "python" => Technology::Python,
        "php" | "php-fpm" => Technology::Php,
        "golang" | "go" => Technology::Go,
        "ruby" => Technology::Ruby,
        "rust" => Technology::Rust,
        "openjdk" | "eclipse-temurin" | "amazoncorretto" => Technology::Java,
        "nginx" | "httpd" | "caddy" => Technology::Static,
        // `scratch` et les bases nues ne disent rien de la technologie : c'est
        // aux fichiers de projet de trancher.
        _ => return None,
    };
    Some((tech, version))
}

/// Technologie deduite de la presence de fichiers de projet.
///
/// L'ordre reflete la specificite : un depot qui a un `go.mod` est un projet
/// Go, meme s'il contient aussi un `package.json` pour son frontal.
fn from_project_files(tree: &RepoTree) -> StackFacts {
    let mut f = StackFacts::default();

    if tree.has("go.mod") {
        f.primary = Technology::Go;
        f.runtime_version = go_version(tree);
    } else if tree.has("composer.json") {
        f.primary = Technology::Php;
        f.runtime_version = php_version(tree);
    } else if tree.has("Gemfile") {
        f.primary = Technology::Ruby;
        f.runtime_version = ruby_version(tree);
    } else if tree.has("Cargo.toml") {
        f.primary = Technology::Rust;
    } else if tree.has("pyproject.toml") || tree.has("requirements.txt") || tree.has("setup.py") {
        f.primary = Technology::Python;
        f.runtime_version = python_version(tree);
    // Un `package.json` a la racine est le marqueur le plus faible de tous :
    // c'est souvent la chaine d'assets d'une application ecrite ailleurs.
    // Koha, en Perl, s'annoncait ainsi comme un projet Node.js. Quand la
    // racine declare par ailleurs un langage que YunoHost ne provisionne pas,
    // c'est lui l'application.
    } else if tree.has("package.json") && langage_hors_perimetre(tree).is_none() {
        f.primary = Technology::NodeJs;
        f.runtime_version = node_version(tree);
    } else if tree.has("pom.xml") || tree.has("build.gradle") || tree.has("build.gradle.kts") {
        f.primary = Technology::Java;
    } else if tree.has("index.html") {
        f.primary = Technology::Static;
    }

    // Un depot Go ou Python peut aussi embarquer un frontal Node : le
    // gestionnaire de paquets et l'etape de build restent utiles a connaitre.
    if tree.has("package.json") {
        if f.runtime_version.is_none() && f.primary == Technology::NodeJs {
            f.runtime_version = node_version(tree);
        }
        f.build_script = build_script(tree);
    }
    f.package_managers = package_managers(tree);
    f.has_lockfile = !f.package_managers.is_empty();
    f
}

/// Un langage que le depot annonce a sa racine et que YunoHost ne sait pas
/// provisionner, s'il y en a un.
///
/// Sans cela, un projet Elixir ou .NET voit son frontal JavaScript propose
/// comme « le langage de l'application » : une proposition datee de sa source,
/// et pourtant fausse. Mieux vaut nommer le langage reel et dire qu'il sort du
/// modele que de designer un composant secondaire.
pub fn langage_hors_perimetre(tree: &RepoTree) -> Option<&'static str> {
    const ANNONCES: &[(&str, &str)] = &[
        ("mix.exs", "Elixir"),
        ("cpanfile", "Perl"),
        ("Makefile.PL", "Perl"),
        ("Build.PL", "Perl"),
        ("build.sbt", "Scala"),
        ("pubspec.yaml", "Dart"),
        ("deno.json", "Deno"),
        ("deno.jsonc", "Deno"),
        ("Package.swift", "Swift"),
        ("stack.yaml", "Haskell"),
        ("dune-project", "OCaml"),
        ("shard.yml", "Crystal"),
        ("rebar.config", "Erlang"),
        ("CMakeLists.txt", "C/C++"),
        ("meson.build", "C/C++"),
    ];
    for (fichier, langage) in ANNONCES {
        if tree.has(fichier) {
            return Some(langage);
        }
    }
    // Les projets .NET et Lua nomment leur fichier d'apres le projet : c'est
    // l'extension qui les identifie, pas un nom fixe.
    for (extension, langage) in [(".csproj", ".NET"), (".sln", ".NET"), (".rockspec", "Lua")] {
        if tree
            .paths()
            .iter()
            .any(|p| !p.contains('/') && p.ends_with(extension))
        {
            return Some(langage);
        }
    }
    None
}

/// Le langage qu'une image de base Docker designe, quand YunoHost ne sait pas
/// le provisionner.
///
/// Certains projets ne declarent leur monde nulle part ailleurs : le serveur
/// de synchronisation de KOReader n'a a sa racine qu'un `Dockerfile` et un
/// `Makefile`, et c'est son `FROM openresty/openresty` qui dit qu'il tourne
/// en Lua. Sans cela, l'analyse conclut « aucun fichier de projet reconnu »
/// et demande un langage sans rien proposer.
pub fn langage_hors_perimetre_de_l_image(image: &str) -> Option<&'static str> {
    let court = image.rsplit('/').next().unwrap_or(image).to_lowercase();
    Some(match court.as_str() {
        "openresty" | "lua" => "Lua",
        "elixir" => "Elixir",
        "erlang" => "Erlang",
        "haskell" => "Haskell",
        "swift" => "Swift",
        "dart" => "Dart",
        "mono" | "dotnet" | "aspnet" | "sdk" => ".NET",
        "perl" => "Perl",
        "ocaml" => "OCaml",
        "crystal" => "Crystal",
        _ => return None,
    })
}

/// La commande de demarrage que le projet declare lui-meme, et d'ou elle sort.
///
/// Sans Dockerfile, l'outil demandait la commande alors que beaucoup de
/// projets la declarent noir sur blanc — et il citait le `Procfile` dans les
/// pistes a consulter sans jamais l'ouvrir. On lit donc les declarations,
/// dans l'ordre ou elles sont sans ambiguite.
pub fn commande_declaree(tree: &RepoTree) -> Option<(String, String)> {
    // Un Procfile nomme ses processus : `web` est celui qui sert le trafic.
    if let Some(texte) = tree.text("Procfile") {
        for ligne in texte.lines() {
            if let Some(reste) = ligne.trim().strip_prefix("web:") {
                let reste = reste.trim();
                if !reste.is_empty() {
                    return Some((reste.to_string(), "Procfile".into()));
                }
            }
        }
    }

    // Un point d'entree declare par un projet Python devient un executable de
    // l'environnement virtuel, sous le nom exact de la cle.
    if let Some(texte) = tree.text("pyproject.toml") {
        for section in ["[project.scripts]", "[tool.poetry.scripts]"] {
            if let Some(nom) = premiere_cle(texte, section) {
                return Some((nom, format!("pyproject.toml, {section}")));
            }
        }
    }

    None
}

/// Les processus d'un Procfile autres que `web`, chacun avec son nom (dans
/// `ou`). Le Procfile de chatwoot declare `worker: bundle exec sidekiq` a
/// cote de son serveur : sans ce processus, aucune tache ne s'execute.
pub fn processus_du_procfile(tree: &RepoTree) -> Vec<CommandeDeclaree> {
    let Some(texte) = tree.text("Procfile") else {
        return Vec::new();
    };
    texte
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once(':'))
        .map(|(nom, cmd)| (nom.trim(), cmd.trim()))
        .filter(|(nom, cmd)| {
            *nom != "web"
                && !cmd.is_empty()
                && !nom.is_empty()
                && nom
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .map(|(nom, cmd)| CommandeDeclaree {
            commande: cmd.to_string(),
            ou: nom.to_string(),
        })
        .collect()
}

/// Toutes les cles declarees sous une section TOML.
fn cles_de_section(texte: &str, section: &str) -> Vec<String> {
    let Some((_, apres)) = texte.split_once(section) else {
        return Vec::new();
    };
    apres
        .lines()
        .map(str::trim)
        .take_while(|l| !l.starts_with('['))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, _)| k.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|k| !k.is_empty())
        .collect()
}

/// La premiere cle declaree sous une section TOML, telle quelle.
fn premiere_cle(texte: &str, section: &str) -> Option<String> {
    let apres = texte.split_once(section)?.1;
    for ligne in apres.lines() {
        let l = ligne.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if l.starts_with('[') {
            return None; // section suivante : celle-ci etait vide
        }
        let (cle, _) = l.split_once('=')?;
        let cle = cle.trim().trim_matches('"').trim_matches('\'');
        if !cle.is_empty() {
            return Some(cle.to_string());
        }
    }
    None
}

/// Les technologies reperees ailleurs qu'a la racine, de la mieux attestee a
/// la moins bien.
///
/// Beaucoup de gros projets ne montrent rien a leur racine : leurs composants
/// vivent dans des sous-repertoires, et `from_project_files` en sort les mains
/// vides. Plutot que de demander sans rien proposer, on compte les fichiers de
/// projet qu'ils portent plus bas et on dit ou on les a vus. C'est une
/// proposition datee de sa source, pas une deduction : le choix reste humain.
pub fn pistes_de_technologie(tree: &RepoTree) -> Vec<(Technology, String)> {
    const MARQUEURS: &[(&str, Technology)] = &[
        ("package.json", Technology::NodeJs),
        ("go.mod", Technology::Go),
        ("composer.json", Technology::Php),
        ("Gemfile", Technology::Ruby),
        ("Cargo.toml", Technology::Rust),
        ("pyproject.toml", Technology::Python),
        ("requirements.txt", Technology::Python),
        ("setup.py", Technology::Python),
        ("pom.xml", Technology::Java),
        ("build.gradle", Technology::Java),
        ("build.gradle.kts", Technology::Java),
    ];

    let mut vus: Vec<(Technology, Vec<String>)> = Vec::new();
    for chemin in tree.paths() {
        // Les composants d'un depot vivent pres de la surface. Plus bas, on
        // tombe sur des bibliotheques embarquees : le `package.json` de
        // `www/js/jquery-ui/` faisait passer Sympa, ecrit en Perl, pour du
        // Node.js — une proposition datee de sa source, et fausse.
        if chemin.matches('/').count() > 2 || est_du_decor(&chemin) {
            continue;
        }
        let Some(nom) = chemin.rsplit('/').next() else {
            continue;
        };
        let Some((_, tech)) = MARQUEURS.iter().find(|(m, _)| *m == nom) else {
            continue;
        };
        match vus.iter_mut().find(|(t, _)| t == tech) {
            Some((_, ou)) => ou.push(chemin),
            None => vus.push((*tech, vec![chemin])),
        }
    }

    // Le plus d'indices d'abord ; a egalite, l'ordre de rencontre, qui suit
    // celui du depot et donc n'invente aucune preference.
    vus.sort_by_key(|(_, ou)| std::cmp::Reverse(ou.len()));
    vus.into_iter()
        .map(|(tech, mut ou)| {
            ou.sort();
            let quoi = if ou.len() == 1 {
                ou[0].clone()
            } else {
                format!("{} et {} autres", ou[0], ou.len() - 1)
            };
            (tech, quoi)
        })
        .collect()
}

/// Vrai pour les chemins qui n'appartiennent pas au projet lui-meme.
///
/// Un `package.json` sous `node_modules` est celui d'une dependance, et un
/// `Cargo.toml` sous `tests/fixtures` celui d'un cas de test : les compter
/// ferait passer une technologie absente devant la vraie.
fn est_du_decor(chemin: &str) -> bool {
    const ECARTES: &[&str] = &[
        "node_modules",
        "vendor",
        "third_party",
        "thirdparty",
        "bower_components",
        "jquery",
        "jquery-ui",
        "static",
        "public",
        "fixtures",
        "testdata",
        "examples",
        "example",
        "docs",
        "target",
        "dist",
        "build",
        ".git",
    ];
    chemin
        .split('/')
        .any(|segment| ECARTES.contains(&segment.to_lowercase().as_str()))
}

fn package_managers(tree: &RepoTree) -> Vec<String> {
    let mut out = Vec::new();
    for (file, name) in [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lockb", "bun"),
        ("package-lock.json", "npm"),
        ("composer.lock", "composer"),
        ("poetry.lock", "poetry"),
        ("uv.lock", "uv"),
        ("Pipfile.lock", "pipenv"),
        ("Gemfile.lock", "bundler"),
        ("go.sum", "go"),
        ("Cargo.lock", "cargo"),
    ] {
        if tree.has(file) {
            out.push(name.to_string());
        }
    }
    out
}

fn json(tree: &RepoTree, path: &str) -> Option<Value> {
    serde_json::from_str(tree.text(path)?).ok()
}

fn build_script(tree: &RepoTree) -> Option<String> {
    let pkg = json(tree, "package.json")?;
    let scripts = pkg.get("scripts")?;
    // `build` d'abord, puis les variantes de production les plus repandues.
    for key in [
        "build",
        "build:prod",
        "build:production",
        "compile",
        "prepare",
    ] {
        if let Some(Value::String(s)) = scripts.get(key) {
            if !s.trim().is_empty() {
                return Some(format!("npm run {key}"));
            }
        }
    }
    None
}

/// Modules npm compilant du code natif, qui exigent des paquets Debian absents
/// du package.json. C'est la cause la plus frequente d'un `npm ci` en echec
/// sur une instance YunoHost.
fn native_deps(tree: &RepoTree) -> Vec<String> {
    let Some(pkg) = json(tree, "package.json") else {
        return Vec::new();
    };
    let k = knowledge::get();

    let mut out = Vec::new();
    for section in ["dependencies", "devDependencies", "optionalDependencies"] {
        let Some(Value::Object(deps)) = pkg.get(section) else {
            continue;
        };
        for name in deps.keys() {
            if !k.npm_native_deps(name).is_empty() && !out.contains(name) {
                out.push(name.clone());
            }
        }
    }
    out
}

// --- Extraction de version, par ecosysteme ---

/// Premiere version `X` ou `X.Y` trouvee dans une chaine.
///
/// Les contraintes amont prennent toutes les formes : `>=18`, `^8.2`, `~3.11`,
/// `20-bookworm-slim`, `1.22.3`. On en retient le debut, qui est ce qui
/// interesse les resources du manifest.
fn first_version(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|c| c.is_ascii_digit())?;
    let mut end = start;
    let mut dots = 0;
    while end < bytes.len() {
        match bytes[end] {
            b'0'..=b'9' => end += 1,
            b'.' if dots < 1 && end + 1 < bytes.len() && bytes[end + 1].is_ascii_digit() => {
                dots += 1;
                end += 1;
            }
            _ => break,
        }
    }
    Some(s[start..end].to_string())
}

fn node_version(tree: &RepoTree) -> Option<String> {
    if let Some(v) = tree.text(".nvmrc").and_then(first_version) {
        return Some(v);
    }
    let pkg = json(tree, "package.json")?;
    pkg.get("engines")?
        .get("node")?
        .as_str()
        .and_then(first_version)
}

fn go_version(tree: &RepoTree) -> Option<String> {
    tree.text("go.mod")?
        .lines()
        .find_map(|l| l.trim().strip_prefix("go "))
        .and_then(first_version)
}

fn php_version(tree: &RepoTree) -> Option<String> {
    let c = json(tree, "composer.json")?;
    c.get("require")?
        .get("php")?
        .as_str()
        .and_then(first_version)
}

fn ruby_version(tree: &RepoTree) -> Option<String> {
    if let Some(v) = tree.text(".ruby-version").and_then(first_version) {
        return Some(v);
    }
    tree.text("Gemfile")?
        .lines()
        .find(|l| l.trim_start().starts_with("ruby "))
        .and_then(first_version)
}

fn python_version(tree: &RepoTree) -> Option<String> {
    if let Some(v) = tree.text(".python-version").and_then(first_version) {
        return Some(v);
    }
    tree.text("pyproject.toml")?
        .lines()
        .find(|l| l.trim_start().starts_with("requires-python"))
        .and_then(first_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_image_de_base_peut_nommer_le_langage_a_elle_seule() {
        // Cas du serveur de synchronisation de KOReader : rien a sa racine
        // qu'un Dockerfile et un Makefile ; c'est le `FROM openresty` qui dit
        // qu'il tourne en Lua.
        for (image, attendu) in [
            ("openresty/openresty", "Lua"),
            ("elixir", "Elixir"),
            ("mcr.microsoft.com/dotnet/aspnet", ".NET"),
        ] {
            assert_eq!(
                langage_hors_perimetre_de_l_image(image),
                Some(attendu),
                "{image}"
            );
        }
        for image in ["node", "python", "golang", "debian"] {
            assert_eq!(langage_hors_perimetre_de_l_image(image), None, "{image}");
        }
    }

    #[test]
    fn un_projet_c_est_nomme_par_son_systeme_de_construction() {
        // Cas de ZoneMinder : un CMakeLists.txt a la racine, et un
        // `web/api/composer.json` qui le faisait proposer comme du PHP.
        let arbre = RepoTree::from_pairs([
            ("CMakeLists.txt", "project(zm)"),
            ("web/api/composer.json", "{}"),
        ]);
        assert_eq!(langage_hors_perimetre(&arbre), Some("C/C++"));
    }

    #[test]
    fn un_pipeline_d_assets_ne_fait_pas_le_langage_de_l_application() {
        // Cas de Koha, ecrit en Perl : son `package.json` racine construit
        // ses feuilles de style, et le faisait annoncer comme du Node.js.
        let arbre = RepoTree::from_pairs([
            ("Makefile.PL", "use ExtUtils::MakeMaker;"),
            ("package.json", r#"{"name":"koha"}"#),
        ]);
        let f = detect(&arbre, None);
        assert_eq!(f.primary, Technology::Unknown);
        assert_eq!(f.langage_hors_perimetre.as_deref(), Some("Perl"));
    }

    #[test]
    fn un_vrai_projet_node_reste_un_projet_node() {
        let arbre = RepoTree::from_pairs([("package.json", r#"{"name":"x"}"#)]);
        assert_eq!(detect(&arbre, None).primary, Technology::NodeJs);
    }

    #[test]
    fn une_bibliotheque_embarquee_n_est_pas_le_langage_de_l_application() {
        // Cas de Sympa, ecrit en Perl : son `www/js/jquery-ui/package.json`
        // le faisait proposer comme un projet Node.js.
        let arbre = RepoTree::from_pairs([
            ("cpanfile", "requires 'DBI';"),
            ("www/js/jquery-ui/package.json", "{}"),
        ]);
        assert!(pistes_de_technologie(&arbre).is_empty());
        assert_eq!(langage_hors_perimetre(&arbre), Some("Perl"));
    }

    #[test]
    fn un_composant_du_depot_reste_une_piste() {
        // A l'inverse, les composants qu'un depot range a un ou deux niveaux
        // sont bien les siens : c'est le cas de BigBlueButton.
        let arbre = RepoTree::from_pairs([
            ("bbb-export-annotations/package.json", "{}"),
            ("record-and-playback/core/Gemfile", "source 'x'"),
        ]);
        assert_eq!(pistes_de_technologie(&arbre).len(), 2);
    }
    use ynp_core::facts::BuildStage;

    fn recipe(image: &str, tag: Option<&str>) -> BuildRecipe {
        BuildRecipe {
            stages: vec![BuildStage {
                image: image.into(),
                tag: tag.map(str::to_string),
                alias: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn l_image_de_base_donne_la_technologie_et_la_version() {
        let t = RepoTree::new();
        let f = detect(&t, Some(&recipe("node", Some("20-bookworm-slim"))));
        assert_eq!(f.primary, Technology::NodeJs);
        assert_eq!(f.runtime_version.as_deref(), Some("20"));
    }

    #[test]
    fn une_base_muette_laisse_les_fichiers_de_projet_trancher() {
        // Vikunja : binaire Go statique sur `scratch`.
        let t = RepoTree::from_pairs([("go.mod", "module x\n\ngo 1.22\n")]);
        let f = detect(&t, Some(&recipe("scratch", None)));
        assert_eq!(f.primary, Technology::Go);
        assert_eq!(f.runtime_version.as_deref(), Some("1.22"));
    }

    #[test]
    fn un_projet_go_avec_un_frontal_node_reste_un_projet_go() {
        let t = RepoTree::from_pairs([
            ("go.mod", "go 1.21"),
            ("package.json", r#"{"scripts":{"build":"vite build"}}"#),
            ("pnpm-lock.yaml", ""),
        ]);
        let f = detect(&t, None);
        assert_eq!(f.primary, Technology::Go);
        // L'etape de build du frontal reste connue : il faudra l'executer.
        assert_eq!(f.build_script.as_deref(), Some("npm run build"));
        assert_eq!(f.package_managers, vec!["pnpm"]);
    }

    #[test]
    fn les_contraintes_de_version_sont_normalisees() {
        assert_eq!(first_version(">=18.17.0").as_deref(), Some("18.17"));
        assert_eq!(first_version("^8.2").as_deref(), Some("8.2"));
        assert_eq!(first_version("20-alpine").as_deref(), Some("20"));
        assert_eq!(first_version("3.12-slim-bookworm").as_deref(), Some("3.12"));
        assert_eq!(first_version("latest"), None);
    }

    #[test]
    fn chaque_ecosysteme_expose_sa_version() {
        let php = RepoTree::from_pairs([("composer.json", r#"{"require":{"php":"^8.2"}}"#)]);
        assert_eq!(detect(&php, None).runtime_version.as_deref(), Some("8.2"));

        let py = RepoTree::from_pairs([("pyproject.toml", "requires-python = \">=3.11\"\n")]);
        assert_eq!(detect(&py, None).runtime_version.as_deref(), Some("3.11"));

        let rb = RepoTree::from_pairs([("Gemfile", "source 'x'\nruby \"3.2.2\"\n")]);
        assert_eq!(detect(&rb, None).runtime_version.as_deref(), Some("3.2"));

        let node = RepoTree::from_pairs([(".nvmrc", "20.11.0\n"), ("package.json", "{}")]);
        assert_eq!(
            detect(&node, None).runtime_version.as_deref(),
            Some("20.11")
        );
    }

    #[test]
    fn les_modules_npm_a_compilation_native_sont_signales() {
        let t = RepoTree::from_pairs([(
            "package.json",
            r#"{"dependencies":{"sharp":"^0.33","lodash":"^4"},"devDependencies":{"canvas":"^2"}}"#,
        )]);
        let f = detect(&t, None);
        assert_eq!(f.native_deps, vec!["sharp", "canvas"]);
    }

    #[test]
    fn un_depot_sans_aucun_indice_reste_inconnu_plutot_que_devine() {
        let f = detect(&RepoTree::from_pairs([("README.md", "# projet")]), None);
        assert_eq!(f.primary, Technology::Unknown);
        assert_eq!(f.runtime_version, None);
    }
}

#[cfg(test)]
mod precision {
    use super::*;

    #[test]
    fn la_version_la_plus_precise_l_emporte() {
        // Cas reel de miniflux : `golang:1` dans le Dockerfile de
        // conditionnement, `go 1.26.0` dans go.mod.
        assert_eq!(
            more_precise(Some("1".into()), Some("1.26".into())).as_deref(),
            Some("1.26")
        );
        // A precision egale, l'image de base fait foi.
        assert_eq!(
            more_precise(Some("20".into()), Some("18".into())).as_deref(),
            Some("20")
        );
        assert_eq!(
            more_precise(None, Some("3.11".into())).as_deref(),
            Some("3.11")
        );
        assert_eq!(
            more_precise(Some("8.2".into()), None).as_deref(),
            Some("8.2")
        );
    }
}
