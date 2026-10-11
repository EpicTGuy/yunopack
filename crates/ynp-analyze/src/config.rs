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
    let p = trouves.into_iter().next()?;
    let c = tree.text(&p)?;
    Some((p, c))
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

    for line in content.lines() {
        let trimmed = line.trim();
        let (is_comment, body) = match trimmed.strip_prefix('#') {
            Some(rest) => (true, rest.trim()),
            None => (false, trimmed),
        };

        let Some((name, value)) = body.split_once('=') else {
            continue;
        };
        let name = name.trim().trim_start_matches("export ").trim();
        if name.is_empty() || !is_env_name(name) {
            continue;
        }

        let mut var = make_var(name, non_empty(value.trim()), source);
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
