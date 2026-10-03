//! Mise a plat de l'`AppSpec` pour les templates.
//!
//! Les templates ne manipulent jamais de `Known<T>` : chaque champ leur arrive
//! soit resolu, soit sous forme de marqueur `FIXME(yunopack)`. La decision
//! d'accepter ou non un paquet incomplet appartient a l'appelant, pas au rendu.

use serde_json::{json, Value};
use ynp_core::spec::{Architectures, Triple, UrlScheme};
use ynp_core::AppSpec;

/// Construit le contexte de rendu, et la liste des champs restes a completer.
pub fn build(spec: &AppSpec) -> (Value, Vec<String>) {
    let mut fixmes = Vec::new();
    let mut resoudre = |champ: &str, valeur: Option<&String>| -> String {
        match valeur {
            Some(v) => v.clone(),
            None => {
                let m = format!("FIXME(yunopack): {champ} a completer");
                fixmes.push(champ.to_string());
                m
            }
        }
    };

    let version = resoudre("app.version", spec.app.version.value());
    let description = resoudre("app.description_en", spec.app.description_en.value());
    let licence = resoudre("upstream.license", spec.upstream.license.value());
    let execstart = if spec.features.systemd {
        resoudre("runtime.execstart", spec.runtime.execstart.value())
    } else {
        String::new()
    };
    // Une ressource provisionnee dont l'application ignore l'existence ne sert
    // a rien : ces liaisons sont exigees des que la ressource l'est.
    // Meme exemption que dans la specification : un port passe en argument de
    // la commande de demarrage n'a pas besoin d'une ligne de configuration.
    let port_en_argument = spec
        .runtime
        .execstart
        .value()
        .is_some_and(|c| c.contains("__PORT__"));
    let port_binding = if spec.resources.ports && !port_en_argument {
        resoudre("runtime.port_binding", spec.runtime.port_binding.value())
    } else {
        String::new()
    };
    let database_binding = if spec.resources.database.manifest_type().is_some() {
        resoudre(
            "runtime.database_binding",
            spec.runtime.database_binding.value(),
        )
    } else {
        String::new()
    };

    let technologie = spec
        .runtime
        .technology
        .value()
        .map(|t| format!("{t:?}").to_lowercase())
        .unwrap_or_else(|| {
            fixmes.push("runtime.technology".into());
            "unknown".into()
        });

    let contexte = json!({
        "app": {
            "id": spec.app.id,
            "name": spec.app.name,
            "description_en": description,
            "description_fr": spec.app.description_fr,
            // Le suffixe ~ynh1 marque la premiere revision du paquet pour cette
            // version amont ; il s'incremente ensuite a chaque correctif.
            "version": format!("{version}~ynh1"),
            "maintainers": spec.app.maintainers,
        },
        "upstream": {
            "license": licence,
            "website": spec.upstream.website,
            "code": spec.upstream.code,
            "demo": spec.upstream.demo,
            "admindoc": spec.upstream.admindoc,
            "userdoc": spec.upstream.userdoc,
        },
        "integration": {
            "yunohost_min": spec.integration.yunohost_min,
            "helpers_version": spec.integration.helpers_version,
            "architectures": match &spec.integration.architectures {
                Architectures::All => json!("all"),
                Architectures::Only(v) => json!(v),
            },
            "multi_instance": spec.integration.multi_instance,
            "ldap": triple(spec.integration.ldap),
            "sso": triple(spec.integration.sso),
            "disk": spec.integration.disk,
            "ram_build": spec.integration.ram_build,
            "ram_runtime": spec.integration.ram_runtime,
        },
        "install": {
            "domain": spec.install.url_scheme != UrlScheme::NoUrl,
            "path": spec.install.url_scheme == UrlScheme::DomainAndPath,
            "init_main_permission": spec.install.init_main_permission,
            "extra": spec.install.extra,
        },
        "sources": {
            "url": spec.resources.sources.url.value(),
            "sha256": spec.resources.sources.sha256.value(),
            "autoupdate": spec.resources.sources.autoupdate_strategy,
            "version_regex": spec.resources.sources.autoupdate_version_regex,
            "in_subdir": spec.resources.sources.in_subdir,
            "extract": spec.resources.sources.extract,
            "rename": spec.resources.sources.rename,
            "per_arch": spec.resources.sources.per_arch,
            "multi_arch": spec.resources.sources.utilise_des_binaires(),
        },
        "resources": {
            "system_user": spec.resources.system_user,
            "install_dir": spec.resources.install_dir,
            "data_dir": spec.resources.data_dir,
            "main_permission_url": spec.resources.main_permission_url,
            "ports": spec.resources.ports,
            "port_fixe": spec.resources.port_fixe,
            "apt": spec.resources.apt_packages,
            "apt_extras": spec.resources.apt_extras,
            "database": spec.resources.database.manifest_type(),
            "nodejs_version": spec.resources.nodejs_version,
            "ruby_version": spec.resources.ruby_version,
            "go_version": spec.resources.go_version,
            "composer_version": spec.resources.composer_version,
            "redis": spec.resources.redis,
            "s3": spec.resources.s3,
        },
        "seaweedfs": seaweedfs(),
        "runtime": {
            // Une mise a jour qui compile le fait a cote du repertoire de
            // l'application, pendant que l'ancienne version sert encore :
            // compiler Buzz prend vingt minutes, service arrete jusque-la.
            // Seulement pour les technologies dont le resultat se deplace
            // sans casse ; un environnement Python retient son chemin.
            "construire_a_cote": !spec.runtime.build_steps.is_empty()
                && matches!(technologie.as_str(), "rust" | "go" | "nodejs"),
            "technology": technologie,
            // Chaque etape passe par `bash -c` : `ynh_exec_as_app` execute une
            // commande et ses arguments, pas une ligne de shell. Une etape de
            // Dockerfile comme `(cd src/gui && npm ci)` — celle de Puter —
            // cassait la syntaxe du script d'installation.
            //
            // Quand il y a du Rust a compiler, chaque etape voit la chaine posee
            // dans le repertoire d'installation : `ynh_exec_as_app` ne transmet
            // que `PATH`, pas `CARGO_HOME`.
            "build_steps": spec
                .runtime
                .build_steps
                .iter()
                .map(|e| {
                    let venv = if spec.runtime.venv_python { ENV_VENV } else { "" };
                    let rust = if spec.runtime.rust_toolchain.is_some() { ENV_RUST } else { "" };
                    let corps = entre_apostrophes(&format!("{rust}{venv}{e}"));
                    format!("{}bash -c {corps}", env_docker(e))
                })
                .collect::<Vec<_>>(),
            "rust_toolchain": spec.runtime.rust_toolchain,
            // Le venv se cree avant toute etape : pip et python y sont pris.
            "installer_venv": spec
                .runtime
                .venv_python
                .then(|| entre_apostrophes("python3 -m venv venv")),
            "installer_rust": spec.runtime.rust_toolchain.as_ref().map(|v| {
                entre_apostrophes(&format!(
                    "{ENV_RUST}curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
                     | sh -s -- -y --no-modify-path --profile minimal --default-toolchain {v}"
                ))
            }),
            "execstart": execstart,
            "migrations": spec.runtime.migrations,
            "repertoire_de_travail": spec.runtime.repertoire_de_travail,
            "port_env_var": spec.runtime.port_env_var,
            "port_binding": port_binding,
            "database_binding": database_binding,
            "config_file": spec.runtime.config_file,
            "has_config": spec.a_une_configuration(),
            "env": spec.runtime.env,
            "a_un_secret": spec.runtime.env.values().any(|v| v.contains("__SECRET__")),
        },
        "features": {
            "nginx": spec.features.nginx,
            "systemd": spec.features.systemd,
            "phpfpm": spec.features.phpfpm,
            "logrotate": spec.features.logrotate,
            "fail2ban": spec.features.fail2ban,
            "cron": spec.features.cron,
            "change_url": spec.features.change_url,
            "service_integration": spec.features.service_integration,
        },
        "docs": {
            "description": spec.docs.description,
            "pre_install": spec.docs.pre_install,
            "post_install": spec.docs.post_install,
            "admin": spec.docs.admin,
            "license_text": spec.docs.license_text,
        },
    });

    (contexte, fixmes)
}

/// `ldap` et `sso` acceptent `true`, `false` ou la chaine `"not_relevant"`.
/// Le manifest distingue les trois, le template doit donc recevoir le litteral.
fn triple(t: Triple) -> Value {
    json!(t.manifest_value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::known::Known;
    use ynp_core::spec::*;

    fn spec_minimale() -> AppSpec {
        AppSpec {
            schema_version: SPEC_SCHEMA_VERSION,
            app: AppIdentity {
                id: "demo".into(),
                name: "Demo".into(),
                description_en: Known::resolved("Une demo".into()),
                description_fr: None,
                version: Known::resolved("1.2.3".into()),
                maintainers: vec![],
            },
            upstream: Upstream {
                license: Known::resolved("MIT".into()),
                ..Default::default()
            },
            integration: Integration::default(),
            install: InstallQuestions::default(),
            resources: Resources::default(),
            runtime: Runtime {
                technology: Known::resolved(ynp_core::facts::Technology::Go),
                execstart: Known::resolved("__INSTALL_DIR__/demo".into()),
                ..Default::default()
            },
            features: Features {
                systemd: true,
                nginx: true,
                ..Default::default()
            },
            docs: Docs::default(),
        }
    }

    #[test]
    fn le_suffixe_de_revision_est_ajoute_a_la_version_amont() {
        let (c, _) = build(&spec_minimale());
        assert_eq!(c["app"]["version"], "1.2.3~ynh1");
    }

    #[test]
    fn un_champ_manquant_devient_un_marqueur_et_est_signale() {
        let mut s = spec_minimale();
        s.upstream.license = Known::unresolved("inconnue", &["LICENSE"]);

        let (c, fixmes) = build(&s);
        assert_eq!(fixmes, vec!["upstream.license"]);
        assert!(c["upstream"]["license"]
            .as_str()
            .unwrap()
            .starts_with("FIXME(yunopack)"));
    }

    #[test]
    fn le_demarrage_n_est_reclame_que_si_un_service_existe() {
        let mut s = spec_minimale();
        s.runtime.execstart = Known::unresolved("inconnu", &[]);
        s.features.systemd = false;

        let (_, fixmes) = build(&s);
        assert!(
            fixmes.is_empty(),
            "une app sans daemon n'a pas d'ExecStart : {fixmes:?}"
        );
    }

    #[test]
    fn les_trois_valeurs_de_ldap_sont_distinguees() {
        // Le manifest attend true, false ou la chaine "not_relevant".
        let mut s = spec_minimale();
        s.integration.ldap = Triple::NotRelevant;
        assert_eq!(build(&s).0["integration"]["ldap"], "\"not_relevant\"");
        s.integration.ldap = Triple::No;
        assert_eq!(build(&s).0["integration"]["ldap"], "false");
    }
}

/// L'environnement d'une chaine Rust posee dans le repertoire d'installation.
/// Les etapes s'executent depuis ce repertoire : `$PWD` le designe.
/// Les etapes d'une application Python tournent dans son venv : sans lui,
/// `pip install` vise le Python du systeme, que Debian 12 refuse
/// (« externally-managed-environment ») et que l'utilisateur de l'application
/// n'a pas le droit de modifier.
const ENV_VENV: &str = "export VIRTUAL_ENV=\"$PWD/venv\" PATH=\"$PWD/venv/bin:$PATH\"; ";

const ENV_RUST: &str = "export CARGO_HOME=\"$PWD/.cargo\" RUSTUP_HOME=\"$PWD/.rustup\" \
                        PATH=\"$PWD/.cargo/bin:$PATH\"; ";

/// Une chaine citee pour le shell, entre apostrophes.
///
/// Rien n'est interprete entre apostrophes, sauf l'apostrophe elle-meme, qu'on
/// ferme, echappe et rouvre : `it's` devient `'it'\''s'`.
pub fn entre_apostrophes(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod citation {
    use super::entre_apostrophes;

    #[test]
    fn une_ligne_de_shell_reste_intacte_une_fois_citee() {
        assert_eq!(
            entre_apostrophes("(cd src/gui && npm ci)"),
            "'(cd src/gui && npm ci)'"
        );
        assert_eq!(entre_apostrophes("echo 'a b'"), r"'echo '\''a b'\'''");
    }

    #[test]
    fn bash_relit_la_commande_d_origine() {
        // La preuve par bash lui-meme, plutot qu'une chaine attendue.
        let origine = r#"(cd src/gui && echo "it's $HOME" > /dev/null) || true"#;
        let sortie = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("printf %s {}", entre_apostrophes(origine)))
            .output()
            .expect("bash");
        assert_eq!(String::from_utf8_lossy(&sortie.stdout), origine);
    }
}

/// Le SeaweedFS qu'embarquent les paquets dont l'application exige un
/// stockage S3 : version et empreintes, tenues dans une table de connaissance.
fn seaweedfs() -> Value {
    const TABLE: &str = include_str!("../../../assets/knowledge/stockage-objet.toml");
    let table: toml::Value = toml::from_str(TABLE).expect("stockage-objet.toml invalide");
    serde_json::to_value(&table["seaweedfs"]).expect("table convertible en JSON")
}

/// Les variables que Docker fournit a la construction d'une image
/// multi-architecture, pour une etape qui les utilise.
///
/// `go build -o fit-tool` avec `GOARCH=${TARGETARCH}` chez dreeve,
/// `case "$TARGETARCH"` chez dawarich ou duplicati : sous YunoHost ces
/// variables sont vides, et l'etape se trompe d'architecture ou echoue.
/// `ynh_exec_as_app` ne transmet que PATH : elles passent par `env`, le
/// script d'installation les remplissant d'apres `$YNH_ARCH` avant de
/// changer d'utilisateur.
fn env_docker(etape: &str) -> &'static str {
    const NOMS: &[&str] = &[
        "TARGETARCH",
        "TARGETOS",
        "TARGETPLATFORM",
        "TARGETVARIANT",
        "BUILDPLATFORM",
        "BUILDARCH",
    ];
    if NOMS.iter().any(|n| etape.contains(n)) {
        "env TARGETARCH=\"$YNH_ARCH\" TARGETOS=linux TARGETPLATFORM=\"linux/$YNH_ARCH\" \
         TARGETVARIANT= BUILDPLATFORM=\"linux/$YNH_ARCH\" BUILDARCH=\"$YNH_ARCH\" "
    } else {
        ""
    }
}
