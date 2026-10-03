//! Controles portes de `package_linter` sur les scripts bash.
//!
//! On ne remplace pas le linter officiel : ce portage est un pre-filtre rapide,
//! qui evite d'envoyer sur l'hote de test un paquet dont on sait deja qu'il
//! sera refuse. Un test differentiel (tache L4-6) verifie que nos verdicts
//! concordent avec les siens.

use ynp_core::finding::{Evidence, Finding, Severity};

/// Helpers renommes lors du passage a la version 2.1.
///
/// Generer un ancien nom produit une erreur du linter officiel et, surtout, un
/// script qui echoue a l'execution puisque la fonction n'existe plus.
const HELPERS_OBSOLETES: &[(&str, &str)] = &[
    ("ynh_add_nginx_config", "ynh_config_add_nginx"),
    ("ynh_remove_nginx_config", "ynh_config_remove_nginx"),
    ("ynh_add_systemd_config", "ynh_config_add_systemd"),
    ("ynh_remove_systemd_config", "ynh_config_remove_systemd"),
    ("ynh_add_fpm_config", "ynh_config_add_phpfpm"),
    ("ynh_remove_fpm_config", "ynh_config_remove_phpfpm"),
    ("ynh_add_config", "ynh_config_add"),
    ("ynh_add_fail2ban_config", "ynh_config_add_fail2ban"),
    ("ynh_remove_fail2ban_config", "ynh_config_remove_fail2ban"),
    ("ynh_use_logrotate", "ynh_config_add_logrotate"),
    ("ynh_remove_logrotate", "ynh_config_remove_logrotate"),
    ("ynh_systemd_action", "ynh_systemctl"),
    ("ynh_replace_string", "ynh_replace"),
    ("ynh_secure_remove", "ynh_safe_rm"),
    ("ynh_restore_file", "ynh_restore"),
    ("ynh_exec_warn_less", "ynh_hide_warnings"),
    ("ynh_install_nodejs", "ynh_nodejs_install"),
    (
        "ynh_install_app_dependencies",
        "ynh_apt_install_dependencies",
    ),
    ("ynh_remove_app_dependencies", "ynh_apt_remove_dependencies"),
    ("ynh_print_ON", "(supprime)"),
    ("ynh_print_OFF", "(supprime)"),
];

/// Commandes a ne jamais employer dans un script de paquet.
const INTERDITS: &[(&str, &str, Severity)] = &[
    (
        "sudo ",
        "les scripts tournent deja en root ; `sudo` masque les erreurs",
        Severity::Major,
    ),
    (
        "apt-get install",
        "utiliser `[resources.apt]` du manifest",
        Severity::Major,
    ),
    (
        "apt install",
        "utiliser `[resources.apt]` du manifest",
        Severity::Major,
    ),
    (
        "systemctl start",
        "utiliser `ynh_systemctl`",
        Severity::Minor,
    ),
    (
        "systemctl restart",
        "utiliser `ynh_systemctl`",
        Severity::Minor,
    ),
    (
        "service nginx restart",
        "recharger plutot que redemarrer, via `ynh_systemctl`",
        Severity::Major,
    ),
    (
        "chown -R root",
        "un fichier appartenant a root n'est pas lisible par l'app",
        Severity::Major,
    ),
    (
        "yunohost app ssowatconf",
        "le coeur s'en charge",
        Severity::Minor,
    ),
    (
        "git clone",
        "les sources passent par `[resources.sources]`, avec un sha256",
        Severity::Major,
    ),
];

pub fn verifier(nom: &str, contenu: &str) -> Vec<Finding> {
    let mut out = Vec::new();

    for (numero, ligne) in contenu.lines().enumerate() {
        let ligne_num = numero as u32 + 1;
        let code = ligne.split('#').next().unwrap_or(ligne);

        for (obsolete, remplacant) in HELPERS_OBSOLETES {
            if contient_appel(code, obsolete) {
                out.push(
                    Finding::new(
                        "LINT001",
                        Severity::Blocker,
                        format!("Helper obsolete : {obsolete}"),
                    )
                    .detail(
                        "Renomme en 2.1. La fonction n'existe plus : le script echouerait \
                         a l'execution.",
                    )
                    .remediation(format!("Remplacer par `{remplacant}`."))
                    .evidence(Evidence::at(nom, ligne_num, ligne.trim())),
                );
            }
        }

        for (motif, raison, severite) in INTERDITS {
            if code.contains(motif) {
                out.push(
                    Finding::new(
                        "LINT002",
                        *severite,
                        format!("Commande deconseillee : {}", motif.trim()),
                    )
                    .detail((*raison).to_string())
                    .evidence(Evidence::at(nom, ligne_num, ligne.trim())),
                );
            }
        }

        // Une compilation qui ecrit son resultat hors de l'installation : dans
        // l'image, `go build -o /wrp-amd64` ou `dotnet publish -o /app/publish`
        // ecrit ou il veut ; sous YunoHost, l'utilisateur de l'application n'a
        // pas le droit d'ecrire a la racine, et le service ne trouverait pas
        // le binaire la ou il le cherche.
        if code.contains("ynh_exec_as_app") && sortie_hors_installation(code) {
            out.push(
                Finding::new(
                    "LINT006",
                    Severity::Blocker,
                    "Construction qui ecrit hors du repertoire d'installation",
                )
                .detail(
                    "L'etape reprise du Dockerfile ecrit son resultat a un chemin absolu de \
                     l'image. Sous YunoHost, elle echouerait ou produirait un binaire que le \
                     service ne trouve pas.",
                )
                .remediation(
                    "Ecrire le resultat dans le repertoire d'installation (chemin relatif), \
                     ou corriger `runtime.build_steps` dans appspec.toml.",
                )
                .evidence(Evidence::at(nom, ligne_num, ligne.trim())),
            );
        }

        // `rm -rf` sur un chemin issu d'une variable peut effacer la racine si
        // la variable est vide. Le helper refuse les chemins hors du perimetre.
        if code.contains("rm -rf") && code.contains('$') && !code.contains("ynh_safe_rm") {
            out.push(
                Finding::new(
                    "LINT003",
                    Severity::Major,
                    "`rm -rf` sur un chemin variable",
                )
                .detail(
                    "Si la variable est vide, la commande s'applique a la racine. \
                         `ynh_safe_rm` refuse tout chemin hors du perimetre de l'application.",
                )
                .remediation("Employer `ynh_safe_rm`.")
                .evidence(Evidence::at(nom, ligne_num, ligne.trim())),
            );
        }
    }

    // Effacer les journaux a la suppression les fait disparaitre si une mise a
    // jour echoue et doit etre annulee. Le coeur s'en charge au bon moment.
    if nom.ends_with("remove") && contenu.contains("/var/log/$app") {
        out.push(
            Finding::new(
                "LINT005",
                Severity::Major,
                "Les journaux sont effaces a la suppression",
            )
            .detail(
                "Ils disparaitraient aussi lors du retour arriere d'une mise a jour ratee, \
                     au moment ou l'on en a le plus besoin. Le coeur les supprime lui-meme.",
            )
            .remediation("Retirer la suppression de /var/log/$app du script.")
            .evidence(Evidence::file(nom)),
        );
    }

    // `_common.sh` est inclus par les autres scripts : il n'a pas a charger
    // les helpers lui-meme, et le paquet de reference ne le fait pas non plus.
    let est_inclus = nom.ends_with("_common.sh");
    if !est_inclus && !contenu.contains("source /usr/share/yunohost/helpers") {
        out.push(
            Finding::new(
                "LINT004",
                Severity::Blocker,
                "Les helpers YunoHost ne sont pas charges",
            )
            .detail("Sans `source /usr/share/yunohost/helpers`, aucun `ynh_*` n'existe.")
            .remediation("Ajouter la ligne en tete de script.")
            .evidence(Evidence::file(nom)),
        );
    }

    out
}

/// Vrai si la ligne appelle bien cette fonction, et non une autre dont le nom
/// la contient (`ynh_add_config` contre `ynh_add_config_truc`).
fn contient_appel(ligne: &str, fonction: &str) -> bool {
    let Some(pos) = ligne.find(fonction) else {
        return false;
    };
    let suivant = ligne[pos + fonction.len()..].chars().next();
    !matches!(suivant, Some(c) if c.is_ascii_alphanumeric() || c == '_')
}

/// Vrai pour une compilation dont la sortie est un chemin absolu hors de
/// l'installation (`/tmp` excepte).
fn sortie_hors_installation(code: &str) -> bool {
    const COMPILATEURS: &[&str] = &[
        "go build",
        "dotnet publish",
        "cargo build",
        "gcc ",
        "tsc ",
        "esbuild",
    ];
    if !COMPILATEURS.iter().any(|c| code.contains(c)) {
        return false;
    }
    let mots: Vec<&str> = code.split_whitespace().collect();
    mots.iter().enumerate().any(|(i, m)| {
        let valeur = match m.split_once('=') {
            Some(("-o" | "--output" | "--out-dir" | "--outdir", v)) => Some(v),
            _ if matches!(*m, "-o" | "--output" | "--out-dir" | "--outdir") => {
                mots.get(i + 1).copied()
            }
            _ => None,
        };
        valeur.is_some_and(|v| {
            let v = v.trim_matches(|c| c == '"' || c == '\'');
            v.starts_with('/') && !v.starts_with("/tmp")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(f: &[Finding]) -> Vec<&str> {
        f.iter().map(|x| x.id.as_str()).collect()
    }

    const ENTETE: &str = "#!/bin/bash\nsource /usr/share/yunohost/helpers\n";

    #[test]
    fn un_script_conforme_ne_declenche_rien() {
        let s =
            format!("{ENTETE}ynh_config_add_nginx\nynh_systemctl --service=$app --action=start\n");
        assert!(verifier("install", &s).is_empty());
    }

    #[test]
    fn un_helper_de_la_version_2_0_est_bloquant() {
        let s = format!("{ENTETE}ynh_add_nginx_config\n");
        let f = verifier("install", &s);
        assert_eq!(f[0].severity, Severity::Blocker);
        assert!(f[0]
            .remediation
            .as_deref()
            .unwrap()
            .contains("ynh_config_add_nginx"));
    }

    #[test]
    fn un_nom_de_fonction_qui_en_prefixe_un_autre_ne_declenche_pas() {
        // `ynh_add_config` est obsolete, `ynh_add_config_custom` n'existe pas
        // mais ne doit pas etre confondu avec lui.
        assert!(!contient_appel(
            "ynh_add_config_custom foo",
            "ynh_add_config"
        ));
        assert!(contient_appel(
            "ynh_add_config --template=x",
            "ynh_add_config"
        ));
    }

    #[test]
    fn un_helper_cite_en_commentaire_est_ignore() {
        let s = format!("{ENTETE}# autrefois : ynh_add_nginx_config\nynh_config_add_nginx\n");
        assert!(verifier("install", &s).is_empty());
    }

    #[test]
    fn les_commandes_interdites_sont_signalees() {
        let s = format!("{ENTETE}sudo apt-get install -y curl\ngit clone https://x\n");
        let f = verifier("install", &s);
        assert!(ids(&f).contains(&"LINT002"));
        assert!(f.len() >= 3, "sudo, apt-get et git clone : {:?}", ids(&f));
    }

    #[test]
    fn un_rm_rf_sur_variable_est_signale_mais_pas_le_helper() {
        let s = format!("{ENTETE}rm -rf \"$install_dir\"\n");
        assert!(ids(&verifier("remove", &s)).contains(&"LINT003"));
        let i = "ynh_exec_as_app bash -c 'go build -o /wrp-amd64'\n";
        assert!(ids(&verifier("install", i)).contains(&"LINT006"));
        let i = "ynh_exec_as_app bash -c 'go build -o wrp .'\n";
        assert!(!ids(&verifier("install", i)).contains(&"LINT006"));

        let s = format!("{ENTETE}ynh_safe_rm \"$install_dir\"\n");
        assert!(!ids(&verifier("remove", &s)).contains(&"LINT003"));
    }

    #[test]
    fn l_absence_des_helpers_est_bloquante() {
        let f = verifier("install", "#!/bin/bash\necho hello\n");
        assert!(ids(&f).contains(&"LINT004"));
        assert_eq!(
            f.iter().find(|x| x.id == "LINT004").unwrap().severity,
            Severity::Blocker
        );
    }

    #[test]
    fn le_fichier_commun_n_a_pas_a_charger_les_helpers() {
        // Il est inclus par les autres scripts ; example_ynh ne le fait pas
        // non plus.
        let f = verifier("scripts/_common.sh", "#!/bin/bash\nnodejs_version=20\n");
        assert!(!ids(&f).contains(&"LINT004"), "{:?}", ids(&f));
    }
}
