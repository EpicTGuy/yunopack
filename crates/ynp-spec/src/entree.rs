//! Le programme principal d'un projet compile, quand rien d'autre ne le dit.
//!
//! Sans Dockerfile ni binaire publie, un projet Go ou Rust se construit
//! encore par une commande unique — a condition de savoir quel programme.
//! Chaque langage a sa convention : en Go, `cmd/<nom>/main.go` ou un
//! `main.go` a la racine ; en Rust, le paquet du `Cargo.toml`. hydroxide et
//! gomuks suivent la premiere, fractale la seconde.

use ynp_core::facts::{RepoFacts, Technology};

/// Le binaire a produire et les etapes qui le produisent, dans le
/// repertoire d'installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entree {
    pub binaire: String,
    pub etapes: Vec<String>,
    /// La commande complete, quand elle ne se reduit pas au binaire.
    pub commande: Option<String>,
    /// Migrations a lancer avant chaque demarrage.
    pub migrations: Vec<String>,
}

/// Seulement sans Dockerfile ni binaire publie : sinon, la recette existe.
pub fn deduite(facts: &RepoFacts) -> Option<Entree> {
    if facts.build.is_some()
        || facts
            .selection
            .as_ref()
            .is_some_and(|s| s.evite_la_compilation())
    {
        return None;
    }
    match facts.stack.primary {
        Technology::Go => go(facts),
        Technology::Rust => rust(facts),
        Technology::Python => python(facts),
        _ => None,
    }
}

fn go(facts: &RepoFacts) -> Option<Entree> {
    // `fractal6.go` est un nom de depot, pas un nom de binaire.
    let depot = facts
        .source
        .repo
        .to_lowercase()
        .trim_end_matches(".go")
        .to_string();
    let mains: Vec<&String> = facts
        .tree
        .iter()
        .filter(|p| p.starts_with("cmd/") && p.ends_with("/main.go") && p.matches('/').count() == 2)
        .collect();
    let dossier = |p: &str| p.split('/').nth(1).unwrap_or_default().to_string();
    let (binaire, paquet) =
        if let Some(p) = mains.iter().find(|p| dossier(p).to_lowercase() == depot) {
            (dossier(p), format!("./cmd/{}", dossier(p)))
        } else if facts.tree.iter().any(|p| p == "main.go") {
            (depot.clone(), ".".to_string())
        } else if let [seul] = mains.as_slice() {
            (dossier(seul), format!("./cmd/{}", dossier(seul)))
        } else {
            return None;
        };
    Some(Entree {
        etapes: vec![format!("go build -o {binaire} {paquet}")],
        binaire,
        commande: None,
        migrations: Vec::new(),
    })
}

fn rust(facts: &RepoFacts) -> Option<Entree> {
    let nom = facts.stack.binaire_rust.clone()?;
    let verrou = if facts.tree.iter().any(|p| p == "Cargo.lock") {
        " --locked"
    } else {
        ""
    };
    Some(Entree {
        etapes: vec![
            format!("cargo build --release{verrou} --bin {nom}"),
            format!("cp target/release/{nom} {nom}"),
        ],
        binaire: nom,
        commande: None,
        migrations: Vec::new(),
    })
}

/// La commande et la migration d'un projet Django, d'apres la convention du
/// cadre : `manage.py` et un unique module `<projet>/wsgi.py`. Vaut aussi
/// quand un Dockerfile existe sans dire quoi lancer (pinry).
pub fn django(facts: &RepoFacts) -> Option<(String, Vec<String>)> {
    if facts.stack.primary != Technology::Python || !facts.tree.iter().any(|p| p == "manage.py") {
        return None;
    }
    let wsgi: Vec<&String> = facts
        .tree
        .iter()
        .filter(|p| p.ends_with("/wsgi.py") && p.matches('/').count() == 1)
        .collect();
    let [module] = wsgi.as_slice() else {
        return None;
    };
    let projet = module.split('/').next().unwrap_or_default();
    Some((
        format!(
            "__INSTALL_DIR__/venv/bin/gunicorn {projet}.wsgi:application --bind 127.0.0.1:__PORT__"
        ),
        vec!["__INSTALL_DIR__/venv/bin/python manage.py migrate --noinput".to_string()],
    ))
}

/// Un projet Python s'installe dans son environnement virtuel, par ses
/// dependances declarees. Un projet Django (`manage.py` et un module
/// `<projet>/wsgi.py`) se lance par gunicorn et se migre par manage.py :
/// c'est la convention du cadre, qu'aucun projet ne repete.
fn python(facts: &RepoFacts) -> Option<Entree> {
    let a = |p: &str| facts.tree.iter().any(|x| x == p);
    let mut etapes = vec!["pip install --upgrade pip".to_string()];
    if a("requirements.txt") {
        etapes.push("pip install -r requirements.txt".into());
    } else if a("pyproject.toml") || a("setup.py") {
        etapes.push("pip install .".into());
    } else {
        return None;
    }
    let (commande, migrations) = match django(facts) {
        Some((c, m)) => {
            etapes.push("pip install gunicorn".into());
            (Some(c), m)
        }
        None => (None, Vec::new()),
    };
    Some(Entree {
        binaire: String::new(),
        etapes,
        commande,
        migrations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn faits(tech: Technology, depot: &str, arbre: &[&str]) -> RepoFacts {
        let mut f = RepoFacts::default();
        f.stack.primary = tech;
        f.source.repo = depot.into();
        f.tree = arbre.iter().map(|s| s.to_string()).collect();
        f
    }

    #[test]
    fn go_suit_la_convention_cmd_puis_la_racine() {
        let f = faits(
            Technology::Go,
            "gomuks",
            &["cmd/archivemuks/main.go", "cmd/gomuks/main.go", "go.mod"],
        );
        let e = deduite(&f).unwrap();
        assert_eq!(e.binaire, "gomuks");
        assert_eq!(e.etapes, vec!["go build -o gomuks ./cmd/gomuks"]);

        let f = faits(
            Technology::Go,
            "fractale",
            &["main.go", "cmd/testsetup/main.go"],
        );
        assert_eq!(deduite(&f).unwrap().etapes, vec!["go build -o fractale ."]);

        // Plusieurs programmes, aucun du nom du depot, pas de racine : on ne
        // devine pas.
        let f = faits(Technology::Go, "x", &["cmd/a/main.go", "cmd/b/main.go"]);
        assert_eq!(deduite(&f), None);
    }

    #[test]
    fn un_projet_django_se_lance_par_gunicorn_et_se_migre() {
        let f = faits(
            Technology::Python,
            "pinry",
            &["manage.py", "pinry/wsgi.py", "requirements.txt"],
        );
        let e = deduite(&f).unwrap();
        assert!(e
            .etapes
            .contains(&"pip install -r requirements.txt".to_string()));
        assert_eq!(
            e.commande.as_deref(),
            Some("__INSTALL_DIR__/venv/bin/gunicorn pinry.wsgi:application --bind 127.0.0.1:__PORT__")
        );
        assert_eq!(e.migrations.len(), 1);
    }
}
