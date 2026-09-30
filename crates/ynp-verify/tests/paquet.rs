//! La verification est jugee sur sa capacite a attraper des defauts reels,
//! pas des cas d'ecole : chaque test ci-dessous reproduit une erreur qui s'est
//! effectivement produite ou qui ferait echouer une installation.

use std::fs;
use std::path::{Path, PathBuf};
use ynp_core::finding::Severity;
use ynp_core::known::Known;
use ynp_core::spec::*;
use ynp_core::AppSpec;

fn spec() -> AppSpec {
    AppSpec {
        schema_version: SPEC_SCHEMA_VERSION,
        app: AppIdentity {
            id: "demo".into(),
            name: "Demo".into(),
            description_en: Known::resolved("Une application de demonstration".into()),
            description_fr: None,
            version: Known::resolved("1.0.0".into()),
            maintainers: vec![],
        },
        upstream: Upstream {
            license: Known::resolved("MIT".into()),
            ..Default::default()
        },
        integration: Integration::default(),
        install: InstallQuestions::default(),
        resources: Resources {
            ports: true,
            ..Default::default()
        },
        runtime: Runtime {
            technology: Known::resolved(ynp_core::facts::Technology::Go),
            execstart: Known::resolved("__INSTALL_DIR__/demo".into()),
            port_binding: Known::resolved("PORT=__PORT__".into()),
            ..Default::default()
        },
        features: Features {
            nginx: true,
            systemd: true,
            ..Default::default()
        },
        docs: Docs::default(),
    }
}

/// Construit un paquet minimal mais valide, dans un repertoire temporaire.
fn paquet_sain(nom: &str) -> PathBuf {
    let racine = std::env::temp_dir().join(format!("yunopack-test-{nom}"));
    let _ = fs::remove_dir_all(&racine);
    fs::create_dir_all(racine.join("scripts")).unwrap();
    fs::create_dir_all(racine.join("conf")).unwrap();
    fs::create_dir_all(racine.join("doc")).unwrap();

    fs::write(
        racine.join("manifest.toml"),
        r#"packaging_format = 2
id = "demo"
name = "Demo"
description.en = "Une application de demonstration"
version = "1.0.0~ynh1"
maintainers = []

[upstream]
license = "MIT"

[integration]
yunohost = ">= 12.1.17"
helpers_version = "2.1"
architectures = "all"
multi_instance = false
ldap = false
sso = false
disk = "50M"
ram.build = "50M"
ram.runtime = "50M"

[install]
    [install.domain]
    type = "domain"

[resources]
    [resources.sources]
    [resources.sources.main]
    url = "https://example.com/x.tar.gz"
    sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
"#,
    )
    .unwrap();

    for script in ["install", "remove", "upgrade", "backup", "restore"] {
        let chemin = racine.join("scripts").join(script);
        fs::write(
            &chemin,
            "#!/bin/bash\nsource /usr/share/yunohost/helpers\nynh_config_add_nginx\n",
        )
        .unwrap();
        executable(&chemin);
    }
    fs::write(racine.join("scripts/_common.sh"), "#!/bin/bash\n").unwrap();
    fs::write(
        racine.join("conf/nginx.conf"),
        "location __PATH__/ {\n  alias __INSTALL_DIR__/;\n}\n",
    )
    .unwrap();
    fs::write(racine.join("doc/DESCRIPTION.md"), "Une demo.\n").unwrap();
    // Le linter officiel exige ces deux fichiers, et refuse un LICENSE reduit
    // a une mention : on met un texte de longueur realiste.
    fs::write(
        racine.join("LICENSE"),
        format!("MIT License\n\n{}\n", "Texte de licence. ".repeat(20)),
    )
    .unwrap();
    fs::write(
        racine.join("README.md"),
        "<!-- This README was automatically generated -->\n# Demo pour YunoHost\n",
    )
    .unwrap();
    racine
}

fn executable(chemin: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(chemin, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn ids(racine: &Path, spec: &AppSpec) -> Vec<String> {
    ynp_verify::verify(racine, spec)
        .unwrap()
        .into_iter()
        .map(|f| f.id)
        .collect()
}

#[test]
fn un_paquet_conforme_ne_produit_aucun_constat() {
    let racine = paquet_sain("sain");
    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    assert!(constats.is_empty(), "{constats:#?}");
}

#[test]
fn un_jeton_sans_reglage_est_bloquant_meme_en_commentaire() {
    // Le defaut exact qui a fait echouer une installation reelle : un
    // `__MAJUSCULES__` ecrit dans un commentaire a titre d'exemple.
    let racine = paquet_sain("jeton");
    fs::write(
        racine.join("conf/app.env"),
        "# Les __MAJUSCULES__ sont remplacees a l'installation\nPORT=__PORT__\n",
    )
    .unwrap();

    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    let jeton = constats
        .iter()
        .find(|f| f.id == "CONF001")
        .expect("CONF001 attendu");

    assert_eq!(jeton.severity, Severity::Blocker);
    assert!(jeton.title.contains("MAJUSCULES"));
    // Le message doit nommer l'erreur exacte que produirait l'installation.
    assert!(jeton.detail.contains("wasn't initialized"));
}

#[test]
fn un_jeton_dont_la_ressource_est_declaree_passe() {
    let racine = paquet_sain("jeton-ok");
    // Declaree dans le manifeste, que YunoHost lit — et non seulement dans la
    // specification qui l'a produit.
    let m = fs::read_to_string(racine.join("manifest.toml")).unwrap();
    fs::write(
        racine.join("manifest.toml"),
        format!("{m}\n    [resources.ports]\n"),
    )
    .unwrap();
    fs::write(racine.join("conf/app.env"), "PORT=__PORT__\nAPP=__APP__\n").unwrap();
    assert!(!ids(&racine, &spec()).contains(&"CONF001".to_string()));
}

#[test]
fn un_jeton_de_base_sans_base_provisionnee_est_signale() {
    // Reclamer __DB_PWD__ sans declarer de base est une incoherence silencieuse.
    let racine = paquet_sain("jeton-db");
    fs::write(
        racine.join("conf/app.env"),
        "DATABASE_URL=postgres://__DB_USER__@x\n",
    )
    .unwrap();

    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    assert!(constats
        .iter()
        .any(|f| f.id == "CONF001" && f.title.contains("DB_USER")));
}

#[test]
fn un_marqueur_fixme_restant_est_bloquant() {
    // C'est la garantie centrale du projet : aucun paquet devine ne sort.
    let racine = paquet_sain("fixme");
    fs::write(
        racine.join("conf/systemd.service"),
        "ExecStart=FIXME(yunopack): runtime.execstart non determine\n",
    )
    .unwrap();

    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    let f = constats
        .iter()
        .find(|f| f.id == "FIXME001")
        .expect("FIXME001 attendu");
    assert_eq!(f.severity, Severity::Blocker);
}

#[test]
fn un_script_manquant_ou_non_executable_est_bloquant() {
    let racine = paquet_sain("scripts");
    fs::remove_file(racine.join("scripts/restore")).unwrap();
    assert!(ids(&racine, &spec()).contains(&"STRUCT001".to_string()));

    let racine = paquet_sain("chmod");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            racine.join("scripts/install"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(ids(&racine, &spec()).contains(&"STRUCT002".to_string()));
    }
}

#[test]
fn un_manifest_invalide_est_rejete_avec_le_champ_fautif() {
    let racine = paquet_sain("manifest");
    let m = fs::read_to_string(racine.join("manifest.toml")).unwrap();
    fs::write(
        racine.join("manifest.toml"),
        m.replace("maintainers = []\n", ""),
    )
    .unwrap();

    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    let f = constats
        .iter()
        .find(|f| f.id == "MANIFEST002")
        .expect("MANIFEST002 attendu");
    assert!(f.detail.contains("maintainers"), "{}", f.detail);
}

#[test]
fn une_version_sans_suffixe_de_revision_est_rejetee() {
    // Sans ~ynhN, aucune correction du paquet seul ne declenche de mise a jour.
    let racine = paquet_sain("version");
    let m = fs::read_to_string(racine.join("manifest.toml")).unwrap();
    fs::write(
        racine.join("manifest.toml"),
        m.replace("1.0.0~ynh1", "1.0.0"),
    )
    .unwrap();

    assert!(ids(&racine, &spec()).contains(&"MANIFEST005".to_string()));
}

#[test]
fn les_constats_sortent_du_plus_grave_au_moins_grave() {
    let racine = paquet_sain("tri");
    fs::remove_file(racine.join("doc/DESCRIPTION.md")).unwrap(); // mineur
    fs::write(
        racine.join("scripts/install"),
        "#!/bin/bash\nsource /usr/share/yunohost/helpers\nynh_add_nginx_config\n", // bloquant
    )
    .unwrap();
    executable(&racine.join("scripts/install"));

    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    assert_eq!(constats[0].severity, Severity::Blocker);
    assert_eq!(constats.last().unwrap().severity, Severity::Minor);
}

#[test]
fn un_paquet_sans_licence_est_bloquant() {
    // Le linter officiel nous l'a signale sur notre propre paquet avant que
    // nous reproduisions le controle ici.
    let racine = paquet_sain("licence");
    fs::remove_file(racine.join("LICENSE")).unwrap();
    assert!(ids(&racine, &spec()).contains(&"STRUCT001".to_string()));
}

#[test]
fn une_licence_reduite_a_une_mention_est_signalee() {
    let racine = paquet_sain("licence-courte");
    fs::write(racine.join("LICENSE"), "Voir le depot amont.\n").unwrap();

    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    let f = constats
        .iter()
        .find(|f| f.id == "STRUCT003")
        .expect("STRUCT003 attendu");
    assert_eq!(f.severity, Severity::Major);
}

#[test]
fn une_base_declaree_sans_serveur_apt_est_signalee() {
    let racine = paquet_sain("base");
    let m = fs::read_to_string(racine.join("manifest.toml")).unwrap();
    fs::write(
        racine.join("manifest.toml"),
        format!("{m}\n    [resources.database]\n    type = \"postgresql\"\n"),
    )
    .unwrap();

    assert!(ids(&racine, &spec()).contains(&"MANIFEST006".to_string()));
}

#[test]
fn effacer_les_journaux_a_la_suppression_est_signale() {
    // Ils disparaitraient au retour arriere d'une mise a jour ratee.
    let racine = paquet_sain("journaux");
    fs::write(
        racine.join("scripts/remove"),
        "#!/bin/bash\nsource /usr/share/yunohost/helpers\nynh_safe_rm \"/var/log/$app\"\n",
    )
    .unwrap();
    executable(&racine.join("scripts/remove"));

    assert!(ids(&racine, &spec()).contains(&"LINT005".to_string()));
}

#[test]
fn un_depot_git_dans_le_paquet_n_interrompt_pas_la_verification() {
    // Apres une premiere publication, le paquet contient un .git plein de
    // fichiers binaires. Les lire comme du texte faisait echouer verify.
    let racine = paquet_sain("git");
    fs::create_dir_all(racine.join(".git")).unwrap();
    fs::write(racine.join(".git/index"), [0u8, 159, 146, 150, 255]).unwrap();

    let constats = ynp_verify::verify(&racine, &spec()).unwrap();
    assert!(constats.is_empty(), "{constats:#?}");
}

#[test]
fn un_jeton_declare_dans_la_seule_specification_est_signale() {
    // La specification croit au port, le manifeste ne le declare pas :
    // YunoHost, qui lit le manifeste, ne remplacerait pas __PORT__.
    let racine = paquet_sain("jeton-spec-seule");
    fs::write(racine.join("conf/app.env"), "PORT=__PORT__\n").unwrap();
    assert!(ids(&racine, &spec()).contains(&"CONF001".to_string()));
}

#[test]
fn une_variable_definie_par_le_script_est_disponible() {
    let racine = paquet_sain("jeton-script");
    fs::write(
        racine.join("scripts/install"),
        "#!/bin/bash\nsource /usr/share/yunohost/helpers\nredis_db=$(ynh_redis_get_free_db)\n\
         ynh_app_setting_set --key=relay_key --value=x\nynh_config_add_nginx\n",
    )
    .unwrap();
    executable(&racine.join("scripts/install"));
    fs::write(
        racine.join("conf/app.env"),
        "R=__REDIS_DB__\nK=__RELAY_KEY__\n",
    )
    .unwrap();
    assert!(!ids(&racine, &spec()).contains(&"CONF001".to_string()));
}
