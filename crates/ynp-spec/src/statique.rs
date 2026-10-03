//! Les sites statiques que le projet construit.
//!
//! Beaucoup d'applications web n'ont pas de serveur : une interface
//! construite par Vite ou React, que nginx sert telle quelle. Leur image le
//! dit en finissant par `nginx`, apres avoir copie le resultat de la
//! construction dans `/usr/share/nginx/html` ; sans image, leur package.json
//! le dit par un `vite build` sans aucun serveur parmi ses dependances.
//!
//! Une telle interface reference ses fichiers depuis la racine (`/assets/…`) :
//! servie sous un chemin, elle ne trouverait rien. Le paquet demande donc un
//! domaine dedie.

use ynp_core::facts::{RepoFacts, Technology};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// Etapes de construction, dans le repertoire d'installation.
    pub etapes: Vec<String>,
    /// Le repertoire construit, relatif a l'installation : `dist`.
    pub racine: String,
}

/// Les repertoires ou une image de serveur web range ce qu'elle sert.
const RACINES_DE_SERVEUR: &[&str] = &[
    "/usr/share/nginx/html",
    "/usr/share/caddy",
    "/usr/local/apache2/htdocs",
    "/var/www/html",
    "/srv",
];

/// Les repertoires de travail d'une etape de construction.
const TRAVAIL: &[&str] = &[
    "/app/",
    "/usr/src/app/",
    "/build/",
    "/src/",
    "/opt/app/",
    "/home/node/app/",
];

pub fn site(facts: &RepoFacts) -> Option<Site> {
    if facts.stack.primary != Technology::NodeJs {
        return None;
    }
    match &facts.build {
        Some(b) => {
            let serveur = b
                .start_command()
                .and_then(|c| c.split_whitespace().next().map(str::to_string))
                .is_some_and(|m| matches!(m.rsplit('/').next(), Some("nginx" | "caddy" | "httpd")));
            if !serveur {
                return None;
            }
            // Une configuration que l'image remplit au demarrage depuis
            // l'environnement (`/etc/nginx/templates/*.template`) : le site
            // en depend, et rien ici ne la reproduit.
            if b.copies
                .keys()
                .any(|v| v.contains("/templates/") || v.ends_with(".template"))
            {
                return None;
            }
            let copie = b.copies_d_etape.iter().find(|c| {
                RACINES_DE_SERVEUR
                    .iter()
                    .any(|r| c.vers.trim_end_matches('/') == *r)
            })?;
            let mut racine = copie.depuis.clone();
            for t in TRAVAIL {
                if let Some(reste) = racine.strip_prefix(t) {
                    racine = reste.to_string();
                }
            }
            let racine = racine
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string();
            if racine.is_empty() || racine.starts_with('/') || b.build_steps.is_empty() {
                return None;
            }
            Some(Site {
                etapes: b.build_steps.clone(),
                racine,
            })
        }
        None => sans_image(facts),
    }
}

/// Sans image : une construction Vite ou React, et aucun serveur.
fn sans_image(facts: &RepoFacts) -> Option<Site> {
    let s = &facts.stack;
    if s.scripts_npm.contains_key("start") || facts.tree.iter().any(|p| p == "server.js") {
        return None;
    }
    const SERVEURS: &[&str] = &[
        "express",
        "fastify",
        "koa",
        "@hapi/hapi",
        "next",
        "nuxt",
        "@sveltejs/kit",
        "@remix-run/node",
        "@nestjs/core",
        "socket.io",
        "ws",
    ];
    if s.dependances_npm
        .iter()
        .any(|d| SERVEURS.contains(&d.as_str()))
    {
        return None;
    }
    let construction = s.scripts_npm.get("build")?;
    let racine = if construction.contains("vite build") {
        "dist"
    } else if construction.contains("react-scripts build") {
        "build"
    } else {
        return None;
    };
    let a = |p: &str| facts.tree.iter().any(|x| x == p);
    // Une bibliotheque se construit aussi par Vite : un site a sa page.
    if !a("index.html") && !a("public/index.html") {
        return None;
    }
    let installer = if a("pnpm-lock.yaml") {
        "corepack pnpm install --frozen-lockfile"
    } else if a("yarn.lock") {
        "corepack yarn install --frozen-lockfile"
    } else if a("package-lock.json") {
        "npm ci"
    } else {
        "npm install"
    };
    let lancer = installer
        .split(" install")
        .next()
        .filter(|o| *o != "npm ci")
        .unwrap_or("npm");
    Some(Site {
        etapes: vec![installer.to_string(), format!("{lancer} run build")],
        racine: racine.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::{BuildRecipe, CopieDEtape};

    #[test]
    fn un_site_vite_sans_serveur_se_sert_par_nginx() {
        let mut f = RepoFacts::default();
        f.stack.primary = Technology::NodeJs;
        f.tree = vec![
            "package.json".into(),
            "index.html".into(),
            "pnpm-lock.yaml".into(),
        ];
        f.stack
            .scripts_npm
            .insert("build".into(), "vue-tsc -b && vite build".into());
        let s = site(&f).unwrap();
        assert_eq!(s.racine, "dist");
        assert_eq!(
            s.etapes,
            vec![
                "corepack pnpm install --frozen-lockfile",
                "corepack pnpm run build"
            ]
        );
        // Un serveur parmi les dependances : ce n'est plus un site statique.
        f.stack.dependances_npm = vec!["express".into()];
        assert_eq!(site(&f), None);
    }

    #[test]
    fn une_image_qui_finit_par_nginx_sert_ce_qu_elle_a_construit() {
        let mut f = RepoFacts::default();
        f.stack.primary = Technology::NodeJs;
        f.build = Some(BuildRecipe {
            cmd: Some(vec!["nginx".into(), "-g".into(), "daemon off;".into()]),
            build_steps: vec!["npm ci".into(), "npm run build".into()],
            copies_d_etape: vec![CopieDEtape {
                depuis: "/app/dist".into(),
                vers: "/usr/share/nginx/html".into(),
            }],
            ..Default::default()
        });
        assert_eq!(site(&f).unwrap().racine, "dist");
        // Une configuration remplie au demarrage : on s'abstient.
        f.build.as_mut().unwrap().copies.insert(
            "/etc/nginx/templates/default.conf.template".into(),
            "ng.conf.template".into(),
        );
        assert_eq!(site(&f), None);
    }
}
