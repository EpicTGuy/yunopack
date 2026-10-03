//! Les conventions des cadres PHP.
//!
//! Une application PHP ne se resume pas a des fichiers servis par php-fpm.
//! Un projet Laravel ou Symfony installe ses dependances par composer, sert
//! son repertoire `public/` et non la racine du depot, migre sa base, et
//! Laravel lance en plus son planificateur chaque minute et, s'il en a une,
//! sa file de taches. Aucun projet ne le repete : c'est la convention du
//! cadre. Faute de la suivre, anonaddy s'installait sans dependances, sans
//! base, sans cle de chiffrement — un paquet qui avait l'air fini.
//!
//! Les commandes portent `__PHP_VERSION__` : la version de PHP que le
//! manifeste installe, que les scripts connaissent par `$php_version`.

use ynp_core::facts::{RepoFacts, Technology};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadre {
    Laravel,
    Symfony,
}

/// Le PHP du manifeste, la ou systemd et cron l'appellent.
pub const PHP: &str = "/usr/bin/php__PHP_VERSION__";

fn a(facts: &RepoFacts, chemin: &str) -> bool {
    facts.tree.iter().any(|p| p == chemin)
}

pub fn cadre(facts: &RepoFacts) -> Option<Cadre> {
    if facts.stack.primary != Technology::Php {
        return None;
    }
    if a(facts, "artisan") {
        Some(Cadre::Laravel)
    } else if a(facts, "bin/console") && a(facts, "config/bundles.php") {
        Some(Cadre::Symfony)
    } else {
        None
    }
}

/// Le repertoire que nginx sert, quand ce n'est pas la racine : celui qui
/// porte le controleur frontal `index.php`, s'il n'y en a pas a la racine.
pub fn racine_web(facts: &RepoFacts) -> Option<String> {
    if facts.stack.primary != Technology::Php || a(facts, "index.php") {
        return None;
    }
    let candidats: Vec<&str> = ["public", "web", "wwwroot", "htdocs", "www"]
        .into_iter()
        .filter(|d| a(facts, &format!("{d}/index.php")))
        .collect();
    match candidats.as_slice() {
        [seul] => Some((*seul).to_string()),
        _ => None,
    }
}

/// Les dependances composer, quand le depot ne les embarque pas. composer
/// est celui de Debian, lance par le PHP du manifeste : celui du systeme
/// peut etre plus ancien que ce qu'exige le projet.
pub fn etape_composer(facts: &RepoFacts) -> Option<String> {
    (facts.stack.primary == Technology::Php
        && a(facts, "composer.json")
        && !a(facts, "vendor/autoload.php"))
    .then(|| {
        // Les scripts de composer de Symfony videront le cache dans
        // l'environnement du `.env` du depot, souvent `dev`, dont les paquets
        // ne sont pas installes.
        let env = if cadre(facts) == Some(Cadre::Symfony) {
            "APP_ENV=prod "
        } else {
            ""
        };
        format!(
            "{env}COMPOSER_HOME=\"$PWD/.composer\" php$PHP_VERSION /usr/bin/composer install \
             --no-dev --optimize-autoloader --no-interaction --no-progress"
        )
    })
}

/// Une etape de composer ecrite pour l'image (`composer install`) passe par
/// le composer de Debian et le PHP du manifeste.
pub fn avec_le_php_du_manifeste(etape: &str) -> String {
    match etape.strip_prefix("composer ") {
        Some(reste) => {
            format!("COMPOSER_HOME=\"$PWD/.composer\" php$PHP_VERSION /usr/bin/composer {reste}")
        }
        None => etape.to_string(),
    }
}

/// Les fichiers du frontal, que le projet construit par un script du
/// `package.json` (`vite build`, `mix --production`, `encore production`).
pub fn etapes_du_frontal(facts: &RepoFacts) -> Vec<String> {
    if facts.stack.primary != Technology::Php || !a(facts, "package.json") {
        return Vec::new();
    }
    const OUTILS: &[&str] = &["vite build", "mix", "encore", "webpack"];
    let script = ["build", "production", "prod"].into_iter().find(|s| {
        facts
            .stack
            .scripts_npm
            .get(*s)
            .is_some_and(|c| OUTILS.iter().any(|o| c.contains(o)))
    });
    let Some(script) = script else {
        return Vec::new();
    };
    let installer = if a(facts, "package-lock.json") {
        "npm ci"
    } else {
        "npm install"
    };
    vec![installer.to_string(), format!("npm run {script}")]
}

/// Les migrations de la base, avant chaque demarrage.
pub fn migrations(facts: &RepoFacts) -> Vec<String> {
    match cadre(facts) {
        Some(Cadre::Laravel) => vec![format!("{PHP} artisan migrate --force")],
        Some(Cadre::Symfony)
            if facts
                .stack
                .dependances_php
                .iter()
                .any(|d| d == "doctrine/doctrine-migrations-bundle") =>
        {
            vec![format!(
                "{PHP} bin/console doctrine:migrations:migrate --no-interaction \
                 --allow-no-migration"
            )]
        }
        _ => Vec::new(),
    }
}

/// Les repertoires de donnees que le cadre range dans les sources : les
/// fichiers deposes de Laravel (`storage`). La mise a jour les garde.
pub fn a_garder(facts: &RepoFacts) -> Vec<String> {
    match cadre(facts) {
        Some(Cadre::Laravel) => vec!["storage".into()],
        _ => Vec::new(),
    }
}

/// Les taches planifiees : le planificateur de Laravel, chaque minute.
pub fn cron(facts: &RepoFacts) -> Vec<String> {
    match cadre(facts) {
        Some(Cadre::Laravel) => vec![format!(
            "* * * * * __APP__ cd __INSTALL_DIR__ && {PHP} artisan schedule:run > /dev/null 2>&1"
        )],
        _ => Vec::new(),
    }
}

/// La file de taches de Laravel, quand le projet en utilise une : son
/// exemple de configuration la regle sur autre chose que `sync`.
pub fn file_de_taches(facts: &RepoFacts) -> Option<String> {
    if cadre(facts) != Some(Cadre::Laravel) {
        return None;
    }
    let connexion = facts
        .config
        .variables
        .iter()
        .find(|v| v.name == "QUEUE_CONNECTION" || v.name == "QUEUE_DRIVER")?
        .default
        .as_deref()?
        .to_lowercase();
    (!matches!(connexion.as_str(), "" | "sync" | "null")).then(|| {
        format!("{PHP} __INSTALL_DIR__/artisan queue:work --sleep=3 --tries=3 --max-time=3600")
    })
}

/// Les reglages que le cadre exige et qu'aucun exemple ne remplit :
/// la cle de chiffrement de Laravel (32 octets exactement), l'environnement
/// de production.
pub fn reglages(facts: &RepoFacts) -> Vec<(&'static str, &'static str)> {
    match cadre(facts) {
        Some(Cadre::Laravel) => vec![
            ("APP_KEY", "__CLE_32__"),
            ("APP_ENV", "production"),
            ("APP_DEBUG", "false"),
        ],
        Some(Cadre::Symfony) => vec![("APP_ENV", "prod"), ("APP_DEBUG", "0")],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::ConfigVar;

    fn laravel() -> RepoFacts {
        let mut f = RepoFacts::default();
        f.stack.primary = Technology::Php;
        f.tree = [
            "artisan",
            "composer.json",
            "package.json",
            "package-lock.json",
            "public/index.php",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        f.stack
            .scripts_npm
            .insert("production".into(), "vite build".into());
        f.stack.scripts_npm.insert("prepare".into(), "husky".into());
        f
    }

    #[test]
    fn laravel_suit_sa_convention() {
        let mut f = laravel();
        assert_eq!(cadre(&f), Some(Cadre::Laravel));
        assert_eq!(racine_web(&f).as_deref(), Some("public"));
        assert!(etape_composer(&f)
            .unwrap()
            .contains("/usr/bin/composer install --no-dev"));
        assert_eq!(etapes_du_frontal(&f), vec!["npm ci", "npm run production"]);
        assert_eq!(
            migrations(&f),
            vec!["/usr/bin/php__PHP_VERSION__ artisan migrate --force"]
        );
        assert_eq!(cron(&f).len(), 1);
        assert_eq!(file_de_taches(&f), None);
        f.config.variables.push(ConfigVar {
            name: "QUEUE_CONNECTION".into(),
            default: Some("redis".into()),
            ..Default::default()
        });
        assert!(file_de_taches(&f).unwrap().contains("artisan queue:work"));
    }

    #[test]
    fn une_application_php_classique_garde_sa_racine() {
        let mut f = RepoFacts::default();
        f.stack.primary = Technology::Php;
        f.tree = vec!["index.php".into(), "public/index.php".into()];
        assert_eq!(racine_web(&f), None);
        assert_eq!(cadre(&f), None);
        assert!(migrations(&f).is_empty());
    }

    #[test]
    fn une_etape_composer_de_l_image_prend_le_php_du_manifeste() {
        assert_eq!(
            avec_le_php_du_manifeste("composer install --no-dev"),
            "COMPOSER_HOME=\"$PWD/.composer\" php$PHP_VERSION /usr/bin/composer install --no-dev"
        );
        assert_eq!(avec_le_php_du_manifeste("npm ci"), "npm ci");
    }
}
