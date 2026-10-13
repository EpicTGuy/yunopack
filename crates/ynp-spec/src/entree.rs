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
        Technology::NodeJs => node(facts),
        Technology::Ruby => rails(facts),
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
    let binaire = sans_collision(facts, &binaire);
    let mut etapes = Vec::new();
    if let Some((dossier, _)) = binaire.rsplit_once('/') {
        etapes.push(format!("mkdir -p {dossier}"));
    }
    etapes.push(format!("go build -o {binaire} {paquet}"));
    Some(Entree {
        etapes,
        binaire,
        commande: None,
        migrations: Vec::new(),
    })
}

/// Ou deposer le binaire construit : a la racine, sauf si un repertoire du
/// depot y porte deja son nom. Le crate `fx/` de fx recevait le binaire en
/// `fx/fx`, et systemd refusait d'executer un repertoire.
fn sans_collision(facts: &RepoFacts, nom: &str) -> String {
    let prefixe = format!("{nom}/");
    if facts.tree.iter().any(|p| p.starts_with(&prefixe)) {
        format!(".bin/{nom}")
    } else {
        nom.to_string()
    }
}

fn rust(facts: &RepoFacts) -> Option<Entree> {
    let nom = facts.stack.binaire_rust.clone()?;
    let verrou = if facts.tree.iter().any(|p| p == "Cargo.lock") {
        " --locked"
    } else {
        ""
    };
    let binaire = sans_collision(facts, &nom);
    let mut etapes = vec![format!("cargo build --release{verrou} --bin {nom}")];
    if let Some((dossier, _)) = binaire.rsplit_once('/') {
        etapes.push(format!("mkdir -p {dossier}"));
    }
    etapes.push(format!("cp target/release/{nom} {binaire}"));
    Some(Entree {
        etapes,
        binaire,
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

/// Le serveur d'un projet Python qui cree son application au niveau d'un
/// module (`app = Flask(__name__)` dans `app.py`) : gunicorn pour WSGI,
/// uvicorn pour ASGI, sur le port reserve et la boucle locale. Rend la
/// commande et le serveur a installer. Plusieurs candidats sans racine qui
/// s'impose : on ne choisit pas.
pub fn application_python(facts: &RepoFacts) -> Option<(String, &'static str)> {
    if facts.stack.primary != Technology::Python {
        return None;
    }
    let candidats: Vec<&ynp_core::facts::ApplicationWeb> = facts
        .stack
        .applications_web
        .iter()
        .filter(|a| a.chemin.matches('/').count() <= 2)
        .collect();
    let rang = |a: &ynp_core::facts::ApplicationWeb| {
        let nom = a.chemin.rsplit('/').next().unwrap_or(&a.chemin);
        let profondeur = a.chemin.trim_start_matches("src/").matches('/').count();
        let conventionnel = matches!(
            nom,
            "app.py" | "main.py" | "wsgi.py" | "asgi.py" | "server.py" | "application.py"
        );
        (profondeur, !conventionnel)
    };
    let meilleur = candidats.iter().map(|a| rang(a)).min()?;
    let retenus: Vec<&&ynp_core::facts::ApplicationWeb> =
        candidats.iter().filter(|a| rang(a) == meilleur).collect();
    let [a] = retenus.as_slice() else {
        return None;
    };
    let (dossier, module) = match a.chemin.strip_prefix("src/") {
        Some(reste) => (Some("src"), reste),
        None => (None, a.chemin.as_str()),
    };
    let module = module.trim_end_matches(".py").replace('/', ".");
    let cible = format!("{module}:{}", a.objet);
    Some(match a.cadre.as_str() {
        "Flask" | "Bottle" => (
            format!(
                "__INSTALL_DIR__/venv/bin/gunicorn{} {cible} --bind 127.0.0.1:__PORT__",
                dossier.map(|d| format!(" --chdir {d}")).unwrap_or_default()
            ),
            "gunicorn",
        ),
        _ => (
            format!(
                "__INSTALL_DIR__/venv/bin/uvicorn{} {cible} --host 127.0.0.1 --port __PORT__",
                dossier
                    .map(|d| format!(" --app-dir {d}"))
                    .unwrap_or_default()
            ),
            "uvicorn",
        ),
    })
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
        None => match application_python(facts) {
            Some((c, serveur)) => {
                etapes.push(format!("pip install {serveur}"));
                (Some(c), Vec::new())
            }
            None => (None, Vec::new()),
        },
    };
    Some(Entree {
        binaire: String::new(),
        etapes,
        commande,
        migrations,
    })
}

/// La convention de Rails, sans Dockerfile ni Procfile qui la dise : puma
/// avec sa configuration (celle que Rails genere), les gems hors
/// developpement, les assets precompiles, la base preparee avant chaque
/// demarrage (creee ou migree). feedbin ne declare que ses processus
/// annexes.
fn rails(facts: &RepoFacts) -> Option<Entree> {
    if facts.stack.cadre_web.as_deref() != Some("rails") {
        return None;
    }
    let a = |p: &str| facts.tree.iter().any(|x| x == p);
    let commande = if a("config/puma.rb") {
        "__RUBY_DIR__/bundle exec puma -C config/puma.rb"
    } else {
        "__RUBY_DIR__/bundle exec rails server"
    };
    let mut etapes = vec![
        "bundle config set --local deployment true".to_string(),
        "bundle config set --local without 'development test'".to_string(),
        "bundle install".to_string(),
    ];
    let assets = facts
        .tree
        .iter()
        .any(|p| p.starts_with("app/assets/") || p.starts_with("app/javascript/"));
    if assets {
        etapes.push(
            "RAILS_ENV=production SECRET_KEY_BASE_DUMMY=1 bundle exec rails assets:precompile"
                .to_string(),
        );
    }
    Some(Entree {
        binaire: String::new(),
        etapes,
        commande: Some(commande.to_string()),
        migrations: vec!["__RUBY_DIR__/bundle exec rails db:prepare".to_string()],
    })
}

/// Un projet Node se lance par le script `start` de son `package.json` :
/// c'est ce que fait `npm start`, et ce que toute plateforme d'hebergement
/// Node appelle. Le script est deplie jusqu'au vrai processus (`next start`
/// chez fountain-ink), installe par le gestionnaire dont le depot porte le
/// verrou, puis construit par son script de construction.
///
/// Un `start` qui lance un serveur de developpement (docusaurus, decap-cms :
/// `develop`, `vite`, `--watch`) n'est pas un service : on ne le retient pas.
fn node(facts: &RepoFacts) -> Option<Entree> {
    let commande = if let Some(script) = script_de_production(facts) {
        let appel = format!("npm run {script}");
        let lance = crate::execstart::developper(&appel, facts);
        if de_developpement(&lance) {
            return None;
        }
        crate::execstart::traduire_avec(&appel, facts)?
    } else {
        format!("__NODEJS_DIR__/node {}", point_d_entree_node(facts)?)
    };
    let a = |p: &str| facts.tree.iter().any(|x| x == p);
    let (installer, outil) = if a("pnpm-lock.yaml") {
        ("corepack pnpm install --frozen-lockfile", "corepack pnpm")
    } else if a("yarn.lock") {
        ("corepack yarn install --frozen-lockfile", "corepack yarn")
    } else if a("package-lock.json") {
        ("npm ci", "npm")
    } else {
        ("npm install", "npm")
    };
    let mut etapes = vec![installer.to_string()];
    if let Some(b) = facts.stack.build_script.as_deref() {
        etapes.push(b.replacen("npm", outil, 1));
    }
    Some(Entree {
        binaire: String::new(),
        etapes,
        commande: Some(commande),
        migrations: Vec::new(),
    })
}

/// Le script qui demarre l'application : `start`, sinon le script de
/// production que le projet nomme a sa facon (`server:prod` chez digishare).
pub fn script_de_production(facts: &RepoFacts) -> Option<&'static str> {
    const PRODUCTION: &[&str] = &[
        "start",
        "start:prod",
        "start:production",
        "server:prod",
        "serve:prod",
        "prod",
        "production",
    ];
    if facts.stack.primary != Technology::NodeJs {
        return None;
    }
    PRODUCTION
        .iter()
        .find(|s| facts.stack.scripts_npm.contains_key(**s))
        .copied()
}

/// Sans script `start` : `server.js` a la racine, que `npm start` lance
/// alors de lui-meme ; sinon le `main` du package.json, si le projet depend
/// d'un serveur web — sans quoi `main` designe une bibliotheque.
fn point_d_entree_node(facts: &RepoFacts) -> Option<String> {
    let a = |p: &str| facts.tree.iter().any(|x| x == p);
    if a("server.js") {
        return Some("server.js".into());
    }
    const SERVEURS: &[&str] = &[
        "express",
        "fastify",
        "koa",
        "@hapi/hapi",
        "hapi",
        "restify",
        "polka",
    ];
    let serveur = facts
        .stack
        .dependances_npm
        .iter()
        .any(|d| SERVEURS.contains(&d.as_str()));
    let main = facts.stack.main_npm.as_deref()?;
    (serveur && a(main) && main.ends_with(".js")).then(|| main.to_string())
}

/// Une commande qui lance un serveur de developpement ou un mode de
/// surveillance, et non l'application.
pub(crate) fn de_developpement(commande: &str) -> bool {
    let mots: Vec<&str> = commande.split_whitespace().collect();
    let premier = mots.first().map(|m| m.rsplit('/').next().unwrap_or(m));
    matches!(
        premier,
        Some("nodemon" | "vite" | "webpack-dev-server" | "run-s" | "run-p" | "concurrently")
    ) || mots.iter().any(|m| {
        matches!(*m, "dev" | "develop" | "serve" | "--watch" | "-w" | "watch")
                // `bin/rails tailwindcss:watch`, `--watch=true`.
                || m.ends_with(":watch")
                || m.ends_with(":dev")
                || m.starts_with("--watch")
    }) || commande.contains("react-scripts start")
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
    fn un_binaire_ne_se_copie_pas_dans_le_repertoire_de_son_nom() {
        // Cas de fx : le crate `fx/` porte le nom du binaire.
        let mut f = faits(Technology::Rust, "fx", &["Cargo.lock", "fx/Cargo.toml"]);
        f.stack.binaire_rust = Some("fx".into());
        let e = deduite(&f).unwrap();
        assert_eq!(e.binaire, ".bin/fx");
        assert_eq!(
            e.etapes,
            vec![
                "cargo build --release --locked --bin fx",
                "mkdir -p .bin",
                "cp target/release/fx .bin/fx"
            ]
        );
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

    #[test]
    fn une_application_flask_ou_fastapi_se_sert_par_son_module() {
        use ynp_core::facts::ApplicationWeb;
        let mut f = faits(Technology::Python, "x", &["requirements.txt", "app.py"]);
        let app = |c: &str, o: &str, k: &str| ApplicationWeb {
            chemin: c.into(),
            objet: o.into(),
            cadre: k.into(),
        };
        f.stack.applications_web = vec![
            app("app.py", "app", "Flask"),
            app("tools/x.py", "a", "Flask"),
        ];
        let e = deduite(&f).unwrap();
        assert_eq!(
            e.commande.as_deref(),
            Some("__INSTALL_DIR__/venv/bin/gunicorn app:app --bind 127.0.0.1:__PORT__")
        );
        assert!(e.etapes.contains(&"pip install gunicorn".to_string()));

        f.stack.applications_web = vec![app("src/svc/main.py", "api", "FastAPI")];
        assert_eq!(
            application_python(&f).unwrap().0,
            "__INSTALL_DIR__/venv/bin/uvicorn --app-dir src svc.main:api --host 127.0.0.1 --port __PORT__"
        );

        // Deux modules a egalite : on ne choisit pas.
        f.stack.applications_web = vec![app("a.py", "app", "Flask"), app("b.py", "app", "Flask")];
        assert_eq!(application_python(&f), None);
    }

    #[test]
    fn rails_sans_dockerfile_suit_sa_convention() {
        let mut f = faits(
            Technology::Ruby,
            "feedbin",
            &["Gemfile", "config/puma.rb", "app/assets/config/manifest.js"],
        );
        assert_eq!(deduite(&f), None);
        f.stack.cadre_web = Some("rails".into());
        let e = deduite(&f).unwrap();
        assert_eq!(
            e.commande.as_deref(),
            Some("__RUBY_DIR__/bundle exec puma -C config/puma.rb")
        );
        assert!(e.etapes.iter().any(|x| x.contains("assets:precompile")));
        assert_eq!(
            e.migrations,
            vec!["__RUBY_DIR__/bundle exec rails db:prepare"]
        );
    }

    #[test]
    fn un_projet_node_se_lance_par_son_script_start() {
        let mut f = faits(
            Technology::NodeJs,
            "fountain",
            &["package.json", "pnpm-lock.yaml"],
        );
        f.stack.scripts_npm = [("start", "next start"), ("build", "next build")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        f.stack.build_script = Some("npm run build".into());
        f.stack.dependances_npm = vec!["next".into()];
        let e = deduite(&f).unwrap();
        assert_eq!(
            e.etapes,
            vec![
                "corepack pnpm install --frozen-lockfile",
                "corepack pnpm run build"
            ]
        );
        assert_eq!(
            e.commande.as_deref(),
            Some("__INSTALL_DIR__/node_modules/.bin/next start")
        );

        // Sans script start, `server.js` a la racine.
        let mut g = faits(Technology::NodeJs, "x", &["package.json", "server.js"]);
        assert_eq!(
            deduite(&g).unwrap().commande.as_deref(),
            Some("__NODEJS_DIR__/node server.js")
        );
        // Un `main` sans serveur web est une bibliotheque.
        g.tree = vec!["package.json".into(), "index.js".into()];
        g.stack.main_npm = Some("index.js".into());
        assert_eq!(deduite(&g), None);
        g.stack.dependances_npm = vec!["express".into()];
        assert!(deduite(&g).is_some());

        // Un serveur de developpement n'est pas un service.
        f.stack
            .scripts_npm
            .insert("start".into(), "run-s clean build develop".into());
        assert_eq!(deduite(&f), None);
        f.stack
            .scripts_npm
            .insert("start".into(), "yarn build && yarn start:website".into());
        f.stack
            .scripts_npm
            .insert("start:website".into(), "docusaurus start --watch".into());
        assert_eq!(deduite(&f), None);
    }
}
