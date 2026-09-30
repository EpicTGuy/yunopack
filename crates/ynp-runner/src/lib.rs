//! Validation dynamique : la gate G3.
//!
//! Le cycle reproduit ici a d'abord ete execute a la main, commande par
//! commande, sur l'instance de test. Il a revele ce qu'aucune verification
//! statique n'aurait montre : un paquet qui s'installe, provisionne tout
//! correctement, et dont le service ne demarre pas.
//!
//! Chaque etape est enregistree avec son verdict et son journal, de sorte
//! qu'un echec dise ou et pourquoi sans qu'il faille rejouer la campagne.

pub mod ssh;

use serde::{Deserialize, Serialize};
use ssh::{Hote, SshError};
use ynp_core::gate::GateOutcome;
use ynp_core::AppSpec;

/// Ou et comment installer pendant le test.
#[derive(Debug, Clone)]
pub struct Cible {
    pub hote: String,
    /// Domaine YunoHost. A defaut, le domaine principal de l'hote est utilise.
    pub domaine: Option<String>,
    pub chemin: String,
    pub permission: String,
    /// Sauter la sauvegarde et la restauration, plus lentes.
    pub rapide: bool,
}

impl Cible {
    pub fn new(hote: impl Into<String>) -> Self {
        Self {
            hote: hote.into(),
            domaine: None,
            chemin: String::new(),
            permission: "all_users".into(),
            rapide: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Etape {
    pub nom: String,
    pub reussie: bool,
    // `default` est indispensable : sans lui, une etape reussie — dont le
    // detail vide n'est pas serialise — devient illisible a la relecture, et
    // le rapport entier est silencieusement perdu.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub duree_s: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rapport {
    pub hote: String,
    pub app: String,
    pub url: String,
    pub etapes: Vec<Etape>,
}

impl Rapport {
    pub fn reussi(&self) -> bool {
        self.etapes.iter().all(|e| e.reussie)
    }

    pub fn premiere_erreur(&self) -> Option<&Etape> {
        self.etapes.iter().find(|e| !e.reussie)
    }

    pub fn gate(&self) -> GateOutcome {
        use ynp_core::finding::{Evidence, Finding, Severity};
        match self.premiere_erreur() {
            None => GateOutcome::Pass,
            Some(e) => GateOutcome::fail(vec![Finding::new(
                "G3",
                Severity::Blocker,
                format!("Etape en echec : {}", e.nom),
            )
            .detail(e.detail.clone())
            .evidence(Evidence::file(format!("{}:{}", self.hote, self.app)))]),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error(transparent)]
    Ssh(#[from] SshError),
    #[error("l'hote n'expose aucun domaine YunoHost")]
    SansDomaine,
}

/// Deroule le cycle complet sur l'hote.
///
/// L'application est toujours desinstallee a la fin, y compris en cas d'echec :
/// une campagne qui laisse des traces rend la suivante ininterpretable.
pub async fn run_g3(
    paquet: &std::path::Path,
    spec: &AppSpec,
    cible: &Cible,
) -> Result<Rapport, RunnerError> {
    let hote = Hote::new(&cible.hote);
    hote.joignable().await?;

    let app = spec.app.id.clone();
    let domaine = match &cible.domaine {
        Some(d) => d.clone(),
        None => {
            let s = hote
                .executer("yunohost domain main-domain --output-as plain")
                .await?;
            s.texte()
                .lines()
                .find(|l| !l.starts_with('#') && !l.trim().is_empty())
                .map(str::to_string)
                .ok_or(RunnerError::SansDomaine)?
        }
    };
    // Un paquet qui ne pose pas de question de chemin occupe la racine du
    // domaine : c'est le cas des relais et des applications qui servent leur
    // interface depuis `/`. Lui en imposer un faisait echouer l'installation,
    // ou tester une adresse qui ne sert rien.
    let a_une_question_de_chemin = std::fs::read_to_string(paquet.join("manifest.toml"))
        .map(|m| a_une_question_de_chemin(&m))
        .unwrap_or(true);
    let chemin = if !cible.chemin.is_empty() {
        cible.chemin.clone()
    } else if a_une_question_de_chemin {
        format!("/{app}")
    } else {
        String::new()
    };
    let url = format!("https://{domaine}{chemin}/");

    let mut etapes = Vec::new();
    let distant = format!("/tmp/yunopack/{app}_ynh");

    // Une installation residuelle d'une campagne precedente fausserait tout.
    let _ = hote
        .executer(&format!("yunohost app remove {app} 2>/dev/null"))
        .await;

    let mut chrono = Chrono::new();
    let envoi = hote.envoyer(paquet, &distant).await?;
    etapes.push(chrono.etape("transfert du paquet", envoi.ok(), envoi.diagnostic()));

    if envoi.ok() {
        let args = if a_une_question_de_chemin {
            format!(
                "domain={domaine}&path={chemin}&init_main_permission={}",
                cible.permission
            )
        } else {
            format!("domain={domaine}&init_main_permission={}", cible.permission)
        };
        let install = hote
            .executer(&format!(
                "yunohost app install {distant} --force -a \"{args}\""
            ))
            .await?;
        let diagnostic = if install.ok() {
            String::new()
        } else {
            cause_dans_le_journal(&hote, &install.diagnostic_complet())
                .await
                .map(|c| format!("{c}\n{}", install.diagnostic()))
                .unwrap_or_else(|| install.diagnostic())
        };
        etapes.push(chrono.etape("installation", install.ok(), diagnostic));

        if install.ok() {
            etapes.extend(verifier_le_service(&hote, spec, &app, &url, &mut chrono).await?);
            if !cible.rapide {
                etapes.extend(mise_a_jour(&hote, spec, &app, &distant, &url, &mut chrono).await?);
                etapes.extend(sauvegarde_restauration(&hote, &app, &url, &mut chrono).await?);
            }
        }
    }

    // Desinstallation et controle des residus, quoi qu'il soit arrive avant.
    //
    // Quand une installation echoue, YunoHost l'annule lui-meme : l'appli
    // n'existe plus, et la desinstaller « echoue » faute d'objet. Le rapport
    // de Buzz affichait ainsi deux echecs pour une seule cause, ce qui noie
    // la vraie. On regarde donc d'abord si elle est la — par son repertoire,
    // qui ne depend pas de la langue du serveur.
    let presente = hote
        .executer(&format!("test -d /etc/yunohost/apps/{app}"))
        .await?
        .ok();
    if presente {
        let remove = hote.executer(&format!("yunohost app remove {app}")).await?;
        etapes.push(chrono.etape("desinstallation", remove.ok(), remove.diagnostic()));
    } else {
        etapes.push(chrono.etape(
            "desinstallation",
            true,
            "rien a desinstaller : YunoHost a deja annule l'installation".to_string(),
        ));
    }

    let residus = chercher_les_residus(&hote, spec, &app).await?;
    etapes.push(chrono.etape(
        "absence de residus",
        residus.is_empty(),
        if residus.is_empty() {
            String::new()
        } else {
            format!("restants : {}", residus.join(", "))
        },
    ));

    let _ = hote.executer(&format!("rm -rf {distant}")).await;

    Ok(Rapport {
        hote: cible.hote.clone(),
        app,
        url,
        etapes,
    })
}

/// Rejoue le paquet sur l'application installee, puis la reverifie.
///
/// C'est la mise a jour « depuis la meme version » de package_check : elle
/// exerce le script `upgrade`, que l'installation seule ne touche jamais.
/// Un script qui regenere un secret, oublie de garder un repertoire ou ne
/// redemarre pas le service ne se voit qu'ici.
async fn mise_a_jour(
    hote: &Hote,
    spec: &AppSpec,
    app: &str,
    distant: &str,
    url: &str,
    chrono: &mut Chrono,
) -> Result<Vec<Etape>, SshError> {
    let upgrade = hote
        .executer(&format!(
            "yunohost app upgrade {app} --file {distant} --force"
        ))
        .await?;
    let diagnostic = if upgrade.ok() {
        String::new()
    } else {
        cause_dans_le_journal(hote, &upgrade.diagnostic_complet())
            .await
            .map(|c| format!("{c}\n{}", upgrade.diagnostic()))
            .unwrap_or_else(|| upgrade.diagnostic())
    };
    let mut etapes = vec![chrono.etape("mise a jour", upgrade.ok(), diagnostic)];
    if upgrade.ok() {
        etapes.extend(
            verifier_le_service(hote, spec, app, url, chrono)
                .await?
                .into_iter()
                .map(|mut e| {
                    e.nom = format!("{} apres mise a jour", e.nom);
                    e
                }),
        );
    }
    Ok(etapes)
}

async fn verifier_le_service(
    hote: &Hote,
    spec: &AppSpec,
    app: &str,
    url: &str,
    chrono: &mut Chrono,
) -> Result<Vec<Etape>, SshError> {
    let mut etapes = Vec::new();

    if spec.features.systemd {
        // Le service peut mettre un instant a se stabiliser ; « activating »
        // n'est pas un succes, c'est une boucle de redemarrage qui commence.
        let _ = hote.executer("sleep 3").await;
        let etat = hote.executer(&format!("systemctl is-active {app}")).await?;
        let actif = etat.texte() == "active";
        let detail = if actif {
            String::new()
        } else {
            let journal = hote
                .executer(&format!("journalctl -u {app} -n 15 --no-pager -o cat"))
                .await?;
            format!("etat : {}\n{}", etat.texte(), journal.diagnostic())
        };
        etapes.push(chrono.etape("service actif", actif, detail));
    }

    // Un binaire compile ailleurs peut exiger une bibliotheque absente de la
    // cible. Constate sur yunopack lui-meme : ses binaires, construits sur
    // Ubuntu 24.04, reclamaient la glibc 2.39 quand bookworm n'a que la 2.36.
    // Le service demarrait quand meme — l'un des deux binaires suffisait — et
    // rien ne signalait que l'autre etait inutilisable.
    etapes.push(binaires_executables(hote, app, chrono).await?);

    // Un service « actif » ne prouve rien : il peut tourner sans repondre, ou
    // redemarrer en boucle assez vite pour que systemd le dise actif. Seule une
    // reponse sur le port reserve etablit que l'application sert vraiment.
    if spec.resources.ports {
        etapes.push(repondre_sur_son_port(hote, app, chrono).await?);
    }

    if spec.features.nginx {
        etapes.push(endpoint_public(hote, url, chrono).await?);
    }
    Ok(etapes)
}

/// Verifie que chaque executable livre peut reellement tourner sur la cible.
///
/// `ldd` liste les bibliotheques dynamiques et signale celles qui manquent.
/// C'est un controle generique : il attrape aussi bien une glibc trop recente
/// qu'une dependance oubliee dans `[resources.apt]`.
async fn binaires_executables(
    hote: &Hote,
    app: &str,
    chrono: &mut Chrono,
) -> Result<Etape, SshError> {
    let commande = format!(
        "for f in $(find /var/www/{app} -maxdepth 2 -type f -perm -u+x 2>/dev/null); do \
           if file -b \"$f\" 2>/dev/null | grep -q ELF; then \
             manque=$(ldd \"$f\" 2>&1 | grep -E 'not found|no version information' || true); \
             [ -n \"$manque\" ] && echo \"$f : $manque\"; \
           fi; \
         done; true"
    );
    let sortie = hote.executer(&commande).await?;
    let manquantes = sortie.texte().to_string();

    Ok(chrono.etape(
        "binaires executables",
        manquantes.is_empty(),
        if manquantes.is_empty() {
            String::new()
        } else {
            format!("dependances non satisfaites :\n{manquantes}")
        },
    ))
}

/// Interroge l'application sur le port que le coeur lui a reserve, a l'adresse
/// exacte que le reverse-proxy lui transmettra.
///
/// Le chemin compte autant que le port. nginx passe l'URL complete, chemin
/// d'installation compris : une application qui n'ecoute qu'a la racine rend
/// 404 sur tout le reste. Interroger « / » ne le verrait pas, et l'endpoint
/// public non plus des que le portail protege l'application — sa redirection
/// vers le SSO ressemble a une reponse. Constate sur yunopack lui-meme, dont
/// l'interface etait injoignable sur `/yunopack/` alors que les trois autres
/// controles etaient au vert.
async fn repondre_sur_son_port(
    hote: &Hote,
    app: &str,
    chrono: &mut Chrono,
) -> Result<Etape, SshError> {
    let port = hote
        .executer(&format!("yunohost app setting {app} port"))
        .await?;
    let port = port.texte().to_string();
    if port.is_empty() {
        return Ok(chrono.etape("reponse sur le port", false, "aucun port reserve".into()));
    }

    let chemin = hote
        .executer(&format!("yunohost app setting {app} path"))
        .await?;
    let chemin = chemin_interroge(chemin.texte());

    // nginx transmet le domaine dans l'en-tete Host : l'application doit etre
    // interrogee de meme. Buzz choisit sa communaute d'apres l'hote, et
    // repond 404 a 127.0.0.1 ; Django et Rails refusent un hote non autorise.
    let domaine = hote
        .executer(&format!("yunohost app setting {app} domain"))
        .await?;
    let domaine = domaine.texte().to_string();
    let en_tete_hote = if !domaine.is_empty()
        && domaine
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        format!("-H 'Host: {domaine}' ")
    } else {
        String::new()
    };

    // Une application peut mettre du temps a ouvrir son port apres le
    // demarrage de son service : migrations, controles de ses dependances.
    // Buzz verifie son stockage S3 pendant plusieurs secondes avant
    // d'ecouter ; l'interroger une seule fois l'a dit muette a tort. On lui
    // laisse une minute tant que rien n'ecoute ; toute autre reponse compte.
    let code = hote
        .executer(&format!(
            "for i in $(seq 1 30); do \
               c=$(curl -s -o /dev/null -w '%{{http_code}}' --max-time 10 {en_tete_hote}http://127.0.0.1:{port}{chemin}); \
               [ \"$c\" != 000 ] && break; sleep 2; \
             done; echo \"$c\""
        ))
        .await?;
    let c = code.texte().to_string();
    let adresse = format!("127.0.0.1:{port}{chemin}");

    // « 000 » est la reponse de curl quand rien n'ecoute ; 404 celle d'une
    // application qui tourne mais ignore le chemin ou elle est installee.
    let muet = c.is_empty() || c == "000";
    let hors_chemin = c == "404";
    let repond = !muet && !hors_chemin;

    Ok(chrono.etape(
        "reponse sur le port",
        repond,
        match (muet, hors_chemin) {
            (true, _) => {
                format!("rien n'ecoute sur {adresse} — le service tourne mais ne sert pas")
            }
            (_, true) => format!(
                "404 sur {adresse} — l'application ne sert pas le chemin ou elle est installee ; \
                 nginx lui transmet l'URL complete, elle doit en tenir compte"
            ),
            _ => String::new(),
        },
    ))
}

/// Le chemin d'installation ramene a la forme interrogeable par curl.
fn chemin_interroge(reglage: &str) -> String {
    let net = reglage.trim().trim_matches('/');
    if net.is_empty() {
        "/".to_string()
    } else {
        format!("/{net}/")
    }
}

/// Interroge l'adresse publique, en distinguant l'application du portail.
///
/// Une redirection vers `/yunohost/sso` signifie que le portail a intercepte
/// la requete : cela prouve que nginx est configure, pas que l'application
/// repond. Constate en installant yunopack, dont l'endpoint rendait 302 alors
/// que son binaire ne pouvait meme pas demarrer.
async fn endpoint_public(hote: &Hote, url: &str, chrono: &mut Chrono) -> Result<Etape, SshError> {
    let reponse = hote
        .executer(&format!(
            "curl -sk -o /dev/null -w '%{{http_code}} %{{redirect_url}}' --max-time 15 {url}"
        ))
        .await?;
    let texte = reponse.texte().to_string();
    let (code, redirection) = texte.split_once(' ').unwrap_or((texte.as_str(), ""));

    let vers_le_portail = redirection.contains("/yunohost/sso");
    let servi = code.starts_with('2') || code.starts_with('3');

    Ok(chrono.etape(
        "endpoint HTTP",
        servi,
        match (servi, vers_le_portail) {
            (true, true) => String::new(), // protege par le portail, attendu
            (true, false) => String::new(),
            (false, _) => format!("code {code} sur {url}"),
        },
    ))
}

async fn sauvegarde_restauration(
    hote: &Hote,
    app: &str,
    url: &str,
    chrono: &mut Chrono,
) -> Result<Vec<Etape>, SshError> {
    let mut etapes = Vec::new();
    let archive = format!("yunopack_{app}");

    // Une archive laissee par une campagne interrompue ferait echouer
    // celle-ci sur « une archive de ce nom existe deja ».
    let _ = hote
        .executer(&format!("yunohost backup delete {archive} 2>/dev/null"))
        .await;

    let sauvegarde = hote
        .executer(&format!(
            "yunohost backup create --apps {app} --name {archive}"
        ))
        .await?;
    etapes.push(chrono.etape("sauvegarde", sauvegarde.ok(), sauvegarde.diagnostic()));
    if !sauvegarde.ok() {
        return Ok(etapes);
    }

    let _ = hote.executer(&format!("yunohost app remove {app}")).await;
    let restauration = hote
        .executer(&format!(
            "yunohost backup restore {archive} --apps {app} --force"
        ))
        .await?;
    etapes.push(chrono.etape("restauration", restauration.ok(), restauration.diagnostic()));

    if restauration.ok() {
        let code = hote
            .executer(&format!("curl -sk -o /dev/null -w '%{{http_code}}' {url}"))
            .await?;
        let c = code.texte().to_string();
        let servi = c.starts_with('2') || c.starts_with('3');
        etapes.push(chrono.etape(
            "endpoint apres restauration",
            servi,
            if servi {
                String::new()
            } else {
                format!("code {c}")
            },
        ));
    }

    let _ = hote
        .executer(&format!("yunohost backup delete {archive}"))
        .await;
    Ok(etapes)
}

/// La cause d'un echec, lue dans le journal de l'operation YunoHost.
///
/// La fin de la sortie d'une installation ratee montre le retour arriere —
/// retrait des dependances, mise a jour de GRUB — et non ce qui a echoue.
/// Pour Buzz, la vraie raison (« Variable $secret wasn't initialized ») n'y
/// figurait pas. Le journal, lui, la porte sur ses lignes `ERROR`. Son
/// identifiant se trouve dans la sortie, quelle que soit la langue du serveur.
async fn cause_dans_le_journal(hote: &Hote, sortie: &str) -> Option<String> {
    let id = identifiant_de_journal(sortie)?;
    let journal = hote
        .executer(&format!("yunohost log show {id} --output-as plain"))
        .await
        .ok()?;
    let erreurs: Vec<&str> = journal
        .stdout
        .lines()
        .filter_map(|l| l.split_once(": ERROR - ").map(|(_, m)| m.trim()))
        .filter(|m| !m.is_empty())
        .collect();
    // La derniere erreur est souvent le recapitulatif generique ; les
    // premieres disent ce qui s'est passe.
    (!erreurs.is_empty()).then(|| {
        erreurs
            .iter()
            .take(3)
            .map(|e| format!("CAUSE {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Vrai si le manifeste demande un chemin d'installation.
fn a_une_question_de_chemin(manifeste: &str) -> bool {
    manifeste
        .parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("install")?.get("path").cloned())
        .is_some()
}

#[cfg(test)]
mod chemin_d_installation {
    use super::a_une_question_de_chemin;

    #[test]
    fn un_paquet_sans_question_de_chemin_occupe_la_racine() {
        let avec =
            "[install]\n[install.domain]\ntype = \"domain\"\n[install.path]\ntype = \"path\"\n";
        let sans = "[install]\n[install.domain]\ntype = \"domain\"\n";
        assert!(a_une_question_de_chemin(avec));
        assert!(!a_une_question_de_chemin(sans));
    }
}

/// L'identifiant d'un journal d'operation YunoHost cite dans une sortie :
/// `20260930-052101-app_install-buzz`.
fn identifiant_de_journal(sortie: &str) -> Option<String> {
    sortie.split_whitespace().find_map(|mot| {
        let mot = mot.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
        let ok = mot.len() > 16
            && mot.as_bytes().get(8) == Some(&b'-')
            && mot[..8].chars().all(|c| c.is_ascii_digit())
            && mot.contains("-app_");
        ok.then(|| mot.to_string())
    })
}

#[cfg(test)]
mod journal {
    use super::identifiant_de_journal;

    #[test]
    fn l_identifiant_se_trouve_quelle_que_soit_la_langue() {
        // Sortie reelle du Dell, en francais.
        let fr = "ERROR L'opération 'Installer l'application 'buzz'' a échoué ! Pour obtenir de \
                  l'aide, merci de partager le journal de l'opération en utilisant la commande \
                  'yunohost log share 20260930-052101-app_install-buzz'";
        assert_eq!(
            identifiant_de_journal(fr).as_deref(),
            Some("20260930-052101-app_install-buzz")
        );
        assert_eq!(identifiant_de_journal("rien ici"), None);
    }
}

/// Ce qu'une desinstallation propre ne doit laisser derriere elle.
///
/// C'est le controle qui attrape les scripts `remove` incomplets, defaut le
/// plus frequent des paquets ecrits a la main.
async fn chercher_les_residus(
    hote: &Hote,
    spec: &AppSpec,
    app: &str,
) -> Result<Vec<String>, SshError> {
    let mut restants = Vec::new();
    let mut verifier = |nom: &str, present: bool| {
        if present {
            restants.push(nom.to_string());
        }
    };

    verifier(
        "utilisateur systeme",
        hote.reussit(&format!("getent passwd {app}")).await,
    );
    verifier(
        "repertoire d'installation",
        hote.reussit(&format!("test -d /var/www/{app}")).await,
    );
    verifier(
        "configuration nginx",
        hote.reussit(&format!("ls /etc/nginx/conf.d/*.d/{app}.conf"))
            .await,
    );
    verifier(
        "unite systemd",
        hote.reussit(&format!("test -f /etc/systemd/system/{app}.service"))
            .await,
    );
    verifier(
        "reglages",
        hote.reussit(&format!("test -d /etc/yunohost/apps/{app}"))
            .await,
    );
    verifier(
        "logrotate",
        hote.reussit(&format!("test -f /etc/logrotate.d/{app}"))
            .await,
    );

    match spec.resources.database.manifest_type() {
        Some("postgresql") => verifier(
            "base postgresql",
            hote.reussit(&format!(
                "sudo -u postgres psql -lqt | cut -d'|' -f1 | grep -qw {app}"
            ))
            .await,
        ),
        Some("mysql") => verifier(
            "base mysql",
            hote.reussit(&format!("mysql -N -e 'show databases' | grep -qw {app}"))
                .await,
        ),
        _ => {}
    }
    Ok(restants)
}

/// Mesure la duree de chaque etape, pour reperer ce qui coute cher.
struct Chrono(std::time::Instant);

impl Chrono {
    fn new() -> Self {
        Self(std::time::Instant::now())
    }

    fn etape(&mut self, nom: &str, reussie: bool, detail: String) -> Etape {
        let duree_s = self.0.elapsed().as_secs();
        self.0 = std::time::Instant::now();
        Etape {
            nom: nom.to_string(),
            reussie,
            detail,
            duree_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn etape(nom: &str, reussie: bool) -> Etape {
        Etape {
            nom: nom.into(),
            reussie,
            detail: String::new(),
            duree_s: 1,
        }
    }

    fn rapport(etapes: Vec<Etape>) -> Rapport {
        Rapport {
            hote: "machine-de-test".into(),
            app: "demo".into(),
            url: "https://x/demo/".into(),
            etapes,
        }
    }

    #[test]
    fn le_chemin_interroge_suit_celui_de_l_installation() {
        assert_eq!(chemin_interroge("/yunopack"), "/yunopack/");
        assert_eq!(chemin_interroge("/yunopack/"), "/yunopack/");
        assert_eq!(chemin_interroge(" /outils/yunopack "), "/outils/yunopack/");
    }

    #[test]
    fn une_installation_a_la_racine_interroge_la_racine() {
        assert_eq!(chemin_interroge("/"), "/");
        assert_eq!(chemin_interroge(""), "/");
    }

    #[test]
    fn un_cycle_sans_faute_passe_la_porte() {
        let r = rapport(vec![
            etape("installation", true),
            etape("service actif", true),
        ]);
        assert!(r.reussi());
        assert!(!r.gate().blocks_pipeline());
    }

    #[test]
    fn la_premiere_etape_en_echec_est_celle_qui_est_rapportee() {
        let r = rapport(vec![
            etape("installation", true),
            etape("service actif", false),
            etape("endpoint HTTP", false),
        ]);
        assert_eq!(r.premiere_erreur().unwrap().nom, "service actif");
        assert!(r.gate().blocks_pipeline());
    }

    #[test]
    fn un_residu_fait_echouer_la_porte_meme_si_tout_le_reste_a_marche() {
        // C'est precisement ce que la gate doit attraper : un paquet qui
        // fonctionne mais ne sait pas se retirer.
        let r = rapport(vec![
            etape("installation", true),
            etape("absence de residus", false),
        ]);
        assert!(!r.reussi());
        assert_eq!(r.premiere_erreur().unwrap().nom, "absence de residus");
    }
}

#[cfg(test)]
mod serialisation {
    use super::*;

    #[test]
    fn un_rapport_fait_un_aller_retour_json_sans_perte() {
        // Le rapport est relu par `publish` pour decider du niveau a publier :
        // une relecture qui echoue fait annoncer un niveau 0 a tort.
        let r = Rapport {
            hote: "machine-de-test".into(),
            app: "demo".into(),
            url: "https://x/demo/".into(),
            etapes: vec![
                Etape {
                    nom: "installation".into(),
                    reussie: true,
                    detail: String::new(),
                    duree_s: 12,
                },
                Etape {
                    nom: "service".into(),
                    reussie: false,
                    detail: "journal".into(),
                    duree_s: 3,
                },
            ],
        };
        let texte = serde_json::to_string(&r).unwrap();
        let relu: Rapport = serde_json::from_str(&texte).unwrap();

        assert_eq!(relu.etapes.len(), 2);
        assert_eq!(relu.etapes[0].nom, "installation");
        assert_eq!(relu.premiere_erreur().unwrap().nom, "service");
    }
}

#[cfg(test)]
mod controles_probants {
    use super::*;

    /// Reproduit ce qui nous a trompes : un service que systemd dit actif,
    /// un endpoint qui rend 302, et une application qui ne tourne pas.
    fn rapport_trompeur() -> Rapport {
        Rapport {
            hote: "machine-de-test".into(),
            app: "demo".into(),
            url: "https://test.local/demo/".into(),
            etapes: vec![
                Etape {
                    nom: "installation".into(),
                    reussie: true,
                    detail: String::new(),
                    duree_s: 12,
                },
                Etape {
                    nom: "service actif".into(),
                    reussie: true,
                    detail: String::new(),
                    duree_s: 5,
                },
                Etape {
                    nom: "reponse sur le port".into(),
                    reussie: false,
                    detail: "rien n'ecoute sur 127.0.0.1:19329".into(),
                    duree_s: 1,
                },
                Etape {
                    nom: "endpoint HTTP".into(),
                    reussie: true,
                    detail: String::new(),
                    duree_s: 1,
                },
            ],
        }
    }

    #[test]
    fn un_service_actif_qui_ne_sert_rien_fait_echouer_la_porte() {
        // Avant ce controle, la campagne passait au vert sur un paquet dont le
        // binaire ne pouvait pas demarrer : systemd le disait actif, et le 302
        // venait du portail SSO, pas de l'application.
        let r = rapport_trompeur();
        assert!(!r.reussi());
        assert_eq!(r.premiere_erreur().unwrap().nom, "reponse sur le port");
        assert!(r.gate().blocks_pipeline());
    }

    #[test]
    fn une_application_qui_repond_vraiment_passe() {
        let mut r = rapport_trompeur();
        r.etapes[2].reussie = true;
        r.etapes[2].detail.clear();
        assert!(r.reussi());
    }
}

#[cfg(test)]
mod binaires {
    use super::*;

    #[test]
    fn un_binaire_dont_les_dependances_manquent_fait_echouer_la_porte() {
        // Cas reel : les binaires de yunopack, construits sur Ubuntu 24.04,
        // reclamaient la glibc 2.39 quand Debian bookworm n'a que la 2.36.
        // Le service demarrait — l'un des deux binaires suffisait — et rien
        // ne signalait que l'autre etait inutilisable.
        let r = Rapport {
            hote: "machine-de-test".into(),
            app: "demo".into(),
            url: "https://x/demo/".into(),
            etapes: vec![
                Etape {
                    nom: "service actif".into(),
                    reussie: true,
                    detail: String::new(),
                    duree_s: 5,
                },
                Etape {
                    nom: "binaires executables".into(),
                    reussie: false,
                    detail: "GLIBC_2.39 not found".into(),
                    duree_s: 1,
                },
                Etape {
                    nom: "reponse sur le port".into(),
                    reussie: true,
                    detail: String::new(),
                    duree_s: 2,
                },
            ],
        };
        assert_eq!(r.premiere_erreur().unwrap().nom, "binaires executables");
        assert!(r.gate().blocks_pipeline());
    }
}
