//! Traduction d'une commande de demarrage Docker en `ExecStart` systemd.
//!
//! Docker lance l'application depuis une arborescence qu'il maitrise ; YunoHost
//! la lance depuis `$install_dir`, avec des runtimes provisionnes a des chemins
//! qu'il expose par des variables. La traduction consiste donc surtout a
//! replacer les chemins.
//!
//! Les placeholders `__MAJUSCULES__` sont remplaces par les helpers au moment
//! ou la configuration est installee : c'est le mecanisme natif de YunoHost.

use ynp_core::facts::{CommandeDeclaree, RepoFacts, Technology};
use ynp_core::known::{Candidate, Known};

/// Repertoires ou une image Docker range son application.
const RACINES: &[&str] = &[
    "/usr/local/bin/",
    "/usr/bin/",
    "/app/",
    "/srv/",
    "/opt/",
    "/home/app/",
    "/usr/src/app/",
];

/// La commande de demarrage, a condition qu'elle puisse tourner hors de
/// l'image : quelle que soit la branche qui l'a trouvee — Dockerfile, unite
/// systemd, Procfile —, une commande qui designe un hote de conteneur, un
/// fichier que le paquet ne cree pas ou un specificateur de modele systemd
/// (`%i`) redevient une question. L'unite de slidge lance
/// `slidge -c /etc/slidge/%i.conf`.
pub fn derive(facts: &RepoFacts, app_id: &str) -> Known<String> {
    let e = derive_sans_garde(facts, app_id);
    let Some(c) = e.value() else {
        return e;
    };
    let specificateur = c
        .split_whitespace()
        .any(|m| m.contains("%i") || m.contains("%I"));
    match hote_du_conteneur(c)
        .or_else(|| chemin_etranger(c))
        .or_else(|| specificateur.then(|| "%i".to_string()))
    {
        Some(x) => Known::unresolved_avec(
            format!(
                "la commande « {c} » designe « {x} », qui n'existe pas hors de l'image ou \
                 de son modele d'unite"
            ),
            &["la documentation du projet d'origine"],
            Vec::new(),
        ),
        None => e,
    }
}

fn derive_sans_garde(facts: &RepoFacts, app_id: &str) -> Known<String> {
    // Un binaire nu publie par l'amont est renomme d'apres l'app : son chemin
    // est connu sans qu'il faille interpreter quoi que ce soit.
    if let Some(sel) = &facts.selection {
        if sel.prebuilt.iter().any(|a| !a.extract) {
            return Known::resolved(format!("__INSTALL_DIR__/{app_id}"));
        }
        // Une archive publiee : l'executable qui porte le nom du depot, ou le
        // seul qu'elle contient. Seulement sans commande d'image a traduire.
        let sans_commande = facts
            .build
            .as_ref()
            .and_then(|b| b.start_command())
            .is_none();
        if sans_commande {
            if let Some(a) = sel
                .prebuilt
                .iter()
                .find(|a| a.extract && !a.executables.is_empty())
            {
                let depot = facts.source.repo.to_lowercase();
                let porte_le_nom = a.executables.iter().find(|e| {
                    let base = e.rsplit('/').next().unwrap_or(e).to_lowercase();
                    base == depot || base == app_id
                });
                let choisi = porte_le_nom.or(match a.executables.as_slice() {
                    [seul] => Some(seul),
                    _ => None,
                });
                if let Some(e) = choisi {
                    return Known::resolved(format!("__INSTALL_DIR__/{e}"));
                }
            }
        }
    }

    // Sans Dockerfile, le projet peut tout de meme avoir enonce sa commande :
    // un Procfile, un point d'entree declare. C'est une declaration, pas une
    // deduction, et elle vaut donc autant qu'un CMD.
    //
    // Mais toutes les declarations ne se valent pas. Un Procfile (`web:`) et
    // une unite systemd disent « voici le service ». Un point d'entree Python
    // dit seulement « voici un executable » : celui de Sosse, `sosse-admin`,
    // est l'outil d'administration, qui affiche son aide et rend la main. On
    // le propose donc, sans le retenir.
    let (declaree, proposee) = match &facts.stack.commande_declaree {
        Some(d) if d.ou.starts_with("pyproject.toml") => (
            None,
            depuis_declaration(d, facts).map(|c| {
                vec![Candidate::new(
                    c,
                    format!(
                        "executable declare dans {} — a verifier : un point d'entree \
                         n'est pas forcement le service",
                        d.ou
                    ),
                )]
            }),
        ),
        Some(d) => (depuis_declaration(d, facts), None),
        None => (None, None),
    };
    let proposee = proposee.unwrap_or_default();

    let Some(build) = &facts.build else {
        // Le programme principal d'un projet Go ou Rust, d'apres la
        // convention de son langage (voir `entree`).
        let deduite = crate::entree::deduite(facts).and_then(|e| {
            e.commande.or_else(|| {
                (!e.binaire.is_empty()).then(|| format!("__INSTALL_DIR__/{}", e.binaire))
            })
        });
        return match declaree.or(deduite) {
            Some(execstart) => Known::resolved(execstart),
            None => Known::unresolved_avec(
                "aucun Dockerfile ni commande de demarrage declaree par le projet",
                &[
                    "README.md",
                    "Procfile",
                    "la documentation du projet d'origine",
                ],
                proposee,
            ),
        };
    };
    let Some(commande) = build.start_command() else {
        let django = crate::entree::django(facts).map(|(c, _)| c);
        return match declaree.or(django) {
            Some(execstart) => Known::resolved(execstart),
            None => Known::unresolved_avec(
                "le Dockerfile ne declare ni CMD ni ENTRYPOINT",
                &[&build.dockerfile_path, "README.md", "Procfile"],
                proposee,
            ),
        };
    };

    // Un script d'entree ne se traduit pas — mais sa ligne `exec` nomme le
    // vrai processus, et elle, si. `exec "$@"` renvoie au CMD : on repart
    // alors de la commande d'origine privee du script.
    let apres_le_script = facts.stack.exec_du_point_d_entree.as_deref().map(|e| {
        // `"$@"` repasse les arguments du conteneur, c'est-a-dire le `CMD` :
        // on les remet a leur place. `exec "$@"` seul revient donc au `CMD`
        // entier, et `uwsgi "$@"` a `uwsgi` suivi du `CMD`.
        let tete = sans_changement_d_utilisateur(
            e.trim()
                .trim_end_matches("\"$@\"")
                .trim_end_matches("$@")
                .trim(),
        );
        let tete = tete.as_str();
        // Les arguments du conteneur sont ce qui suit le script, pas ce qui
        // suit le premier mot : `tini -- /entrypoint.sh bin/app start`.
        let args: Vec<&str> = if e.contains("$@") {
            let mots: Vec<&str> = commande.split_whitespace().collect();
            let apres = mots
                .iter()
                .position(|m| {
                    let b = m.to_lowercase();
                    b.ends_with(".sh") || b.contains("entrypoint")
                })
                .map_or(1, |i| i + 1);
            mots.get(apres..).unwrap_or(&[]).to_vec()
        } else {
            Vec::new()
        };
        format!("{tete} {}", args.join(" ")).trim().to_string()
    });

    let depuis_le_script = apres_le_script
        .as_deref()
        .and_then(|c| traduire_avec(c, facts));

    // Un Procfile ou une unite systemd decrivent l'application hors
    // conteneur — la situation de YunoHost. Un Procfile suppose meme une
    // plateforme qui attribue le port, exactement comme YunoHost ; un script
    // de conteneur suppose un port fixe. Helpy lancait `unicorn` sans port
    // dans son image, `unicorn_rails -p $PORT` dans son Procfile.
    match declaree
        .or_else(|| traduire_avec(&commande, facts))
        .or(depuis_le_script)
    {
        Some(execstart) => {
            match hote_du_conteneur(&execstart).or_else(|| chemin_etranger(&execstart)) {
                // Un nom qui n'existe que dans le reseau des conteneurs : la
                // commande ne peut pas tourner telle quelle.
                Some(hote) => Known::unresolved_avec(
                    format!(
                        "la commande « {commande} » designe « {hote} », qui n'existe pas hors \
                     de l'image : un hote du reseau des conteneurs, ou un chemin que le \
                     paquet ne cree pas"
                    ),
                    &[
                        &build.dockerfile_path,
                        "la documentation du projet d'origine",
                    ],
                    proposee,
                ),
                None => Known::resolved(execstart),
            }
        }
        None => Known::unresolved_avec(
            format!("commande « {commande} » non transposable automatiquement"),
            &[
                &build.dockerfile_path,
                "la documentation du projet d'origine",
            ],
            proposee,
        ),
    }
}

/// Un chemin absolu de la commande que le paquet ne cree pas.
///
/// L'unite systemd de crowdsec lance `crowdsec -c /etc/crowdsec/config.yaml` :
/// sous YunoHost, ce fichier n'existe pas, et le service ne demarrerait pas.
/// Les chemins du systeme (`/usr/bin`, `/dev/shm`, `/tmp`) restent admis.
fn chemin_etranger(commande: &str) -> Option<String> {
    const ETRANGERS: &[&str] = &[
        "/etc/",
        "/var/lib/",
        "/srv/",
        "/opt/",
        "/data/",
        "/home/",
        "/config/",
        "/app/",
    ];
    commande
        .split(|c: char| c.is_whitespace() || matches!(c, '=' | ':' | '"' | '\''))
        .find(|m| ETRANGERS.iter().any(|e| m.starts_with(e)))
        .map(str::to_string)
}

/// Le premier hote d'une adresse de la commande qui n'est pas la machine
/// locale : `jdbc:mysql://asqatasun-db_dev:3306/...` chez asqatasun. Dans
/// l'image, c'est le nom d'un autre conteneur ; sous YunoHost, il ne designe
/// rien, et le service echouerait a joindre sa base.
fn hote_du_conteneur(commande: &str) -> Option<String> {
    const LOCAUX: &[&str] = &["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]", ""];
    commande.match_indices("://").find_map(|(i, _)| {
        let reste = &commande[i + 3..];
        let fin = reste
            .find(['/', '?', ' ', '"', '\'', ';'])
            .unwrap_or(reste.len());
        // Identifiants avant `@`, port apres `:`.
        let autorite = &reste[..fin];
        let hote = autorite.rsplit('@').next().unwrap_or(autorite);
        let hote = hote.split(':').next().unwrap_or(hote);
        (!LOCAUX.contains(&hote) && !hote.starts_with("__") && !hote.contains('.'))
            .then(|| hote.to_string())
    })
}

/// Ce qu'une declaration du projet donne comme `ExecStart`.
///
/// Un Procfile enonce une vraie ligne de commande : elle passe par la meme
/// traduction que celle d'un Dockerfile. Un point d'entree Python n'est qu'un
/// nom : il designe un executable de l'environnement virtuel.
fn depuis_declaration(d: &CommandeDeclaree, facts: &RepoFacts) -> Option<String> {
    if d.ou.starts_with("pyproject.toml") {
        return Some(format!("__INSTALL_DIR__/venv/bin/{}", d.commande));
    }
    traduire_avec(&d.commande, facts)
}

/// Traduit une commande apres l'avoir depliee.
///
/// Les commandes de conteneur sont souvent des enchainements :
/// `npm run migrate && npm start`, `pm2 start … && tail -f /dev/null`. Les
/// premieres etapes preparent, la derniere est le processus qui tourne — et
/// les preparatifs ont leur place dans le script d'installation, pas dans le
/// service. `npm start`, lui, ne s'ecrit pas dans un service, mais ce qu'il
/// lance si : le `package.json` du projet le dit.
pub(crate) fn traduire_avec(commande: &str, facts: &RepoFacts) -> Option<String> {
    let c = developper(commande, facts);

    // Un executable que le `pyproject.toml` declare vit dans l'environnement
    // virtuel. Le script d'entree de Mealie finit par `exec mealie` : ce nom
    // seul ne dit pas ou le trouver, le pyproject le dit.
    if facts.stack.primary == Technology::Python {
        let c = guillemets_du_shell(&c);
        let mots = sans_enveloppe(&mots_de(&c));
        if let Some(nom) = mots.first() {
            if facts.stack.scripts_python.iter().any(|s| s == nom) {
                let args: Vec<String> = mots[1..].iter().map(|m| chemin(m)).collect();
                return Some(
                    format!("__INSTALL_DIR__/venv/bin/{nom} {}", args.join(" "))
                        .trim()
                        .to_string(),
                );
            }
        }
    }

    // Un outil installe par npm s'appelle par son nom dans un script
    // (`next start`) ; il vit dans `node_modules/.bin`, et s'execute avec
    // l'interprete de la resource.
    if facts.stack.primary == Technology::NodeJs {
        let c = guillemets_du_shell(&c);
        let mots = sans_enveloppe(&mots_de(&c));
        if let Some(outil) = mots.first() {
            if facts.stack.dependances_npm.iter().any(|d| d == outil) {
                let args: Vec<String> = mots[1..].iter().map(|m| chemin(m)).collect();
                return Some(
                    format!(
                        "__NODEJS_DIR__/node __INSTALL_DIR__/node_modules/.bin/{outil} {}",
                        args.join(" ")
                    )
                    .trim()
                    .to_string(),
                );
            }
        }
    }
    let c = substituer_l_environnement(&c, facts);
    let c = ramener_les_volumes(&c, facts);
    let traduite = traduire(&c, facts.stack.primary, &facts.source.repo)?;
    let traduite = lier_les_options_d_ecoute(&traduite, &crate::ports_exposes(facts));
    // Une variable que rien ne definit ferait demarrer le service avec une
    // valeur vide a sa place. InvenTree en portait une, `$APP_HOME`, que son
    // Dockerfile ne fixe pas.
    if traduite.contains('$') {
        return None;
    }
    Some(traduite)
}

/// Un chemin sous un `VOLUME` du Dockerfile va dans le repertoire de donnees.
///
/// Un volume est ce que le conteneur garde d'un redemarrage a l'autre : chez
/// YunoHost, c'est le repertoire de donnees, que la specification declare
/// justement quand le Dockerfile a des volumes. mautrix-googlechat demarrait
/// avec `-c /data/config.yaml`, un chemin qui n'existe pas hors du conteneur.
fn ramener_les_volumes(commande: &str, facts: &RepoFacts) -> String {
    let Some(build) = &facts.build else {
        return commande.to_string();
    };
    commande
        .split(' ')
        .map(|mot| {
            for v in &build.volumes {
                let v = v.trim_end_matches('/');
                if v.is_empty() {
                    continue;
                }
                if mot == v {
                    return "__DATA_DIR__".to_string();
                }
                if let Some(reste) = mot.strip_prefix(&format!("{v}/")) {
                    return format!("__DATA_DIR__/{reste}");
                }
            }
            mot.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Les options d'ecoute d'une commande, ramenees a YunoHost.
///
/// `--host 0.0.0.0` ouvrirait l'application a tout le reseau : la boucle
/// locale suffit, nginx est devant. `--port 3000`, quand 3000 est le port que
/// l'image expose, doit devenir le port reserve — calrs le figeait. Un autre
/// numero (`--metrics-port 9102`) n'est pas touche.
fn lier_les_options_d_ecoute(commande: &str, exposes: &[u16]) -> String {
    const HOTE: &[&str] = &["--host", "--bind", "--listen", "--address", "--addr", "-H"];
    const PORT: &[&str] = &["--port", "--http-port", "--listen-port", "--server-port"];
    const PARTOUT: &[&str] = &["0.0.0.0", "::", "[::]"];
    let expose = |v: &str| v.parse::<u16>().is_ok_and(|p| exposes.contains(&p));
    let mots: Vec<&str> = commande.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(mots.len());
    for (i, mot) in mots.iter().enumerate() {
        let precedent = i.checked_sub(1).map(|j| mots[j]);
        // Forme `--port=3000` et `--host=0.0.0.0`.
        // Aussi `--server.port=8080` et `-Dserver.port=8080`, formes de Spring.
        if let Some((option, valeur)) = mot.split_once('=') {
            let valeur = valeur.trim_matches(|c| c == '"' || c == '\'');
            if HOTE.contains(&option) && PARTOUT.contains(&valeur) {
                out.push(format!("{option}=127.0.0.1"));
                continue;
            }
            let option_de_port =
                PORT.contains(&option) || option.ends_with(".port") || option.ends_with("-port");
            if option_de_port && expose(valeur) {
                out.push(format!("{option}=__PORT__"));
                continue;
            }
        }
        // Forme `--port 3000`.
        if precedent.is_some_and(|p| HOTE.contains(&p)) && PARTOUT.contains(mot) {
            out.push("127.0.0.1".into());
            continue;
        }
        if precedent.is_some_and(|p| PORT.contains(&p)) && expose(mot) {
            out.push("__PORT__".into());
            continue;
        }
        out.push((*mot).to_string());
    }
    out.join(" ")
}

/// Remplace les variables que le Dockerfile fixe par leur valeur.
///
/// `ENV APP_HOME=/app` puis `--chdir $APP_HOME/src` : la commande ne dit
/// rien sans la valeur, et la valeur est la. Les chemins qu'elle introduit
/// sont ensuite ramenes au repertoire d'installation comme les autres.
fn substituer_l_environnement(commande: &str, facts: &RepoFacts) -> String {
    let Some(build) = &facts.build else {
        return commande.to_string();
    };
    let mut c = commande.to_string();
    for (nom, valeur) in &build.env {
        // Une valeur qui renvoie a une autre variable ne s'evalue pas ici.
        if valeur.contains('$') || nom == "PORT" {
            continue;
        }
        c = c.replace(&format!("${{{nom}}}"), valeur);
        let mut out = String::with_capacity(c.len());
        let mut reste = c.as_str();
        let motif = format!("${nom}");
        while let Some(i) = reste.find(&motif) {
            let apres = &reste[i + motif.len()..];
            let fin = !apres.starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_');
            out.push_str(&reste[..i]);
            out.push_str(if fin { valeur } else { &motif });
            reste = apres;
        }
        out.push_str(reste);
        c = out;
    }
    c
}

/// Les mots d'une commande, sans les guillemets qui les entourent.
fn mots_de(commande: &str) -> Vec<&str> {
    commande
        .split_whitespace()
        .map(|m| m.trim_matches(|c| c == '"' || c == '\''))
        .filter(|m| !m.is_empty())
        .collect()
}

/// Les affectations `NOM=valeur` qui precedent une commande.
pub(crate) fn affectations_de_tete(commande: &str) -> Vec<(String, String)> {
    commande
        .split_whitespace()
        .map_while(|m| {
            let (nom, valeur) = m.split_once('=')?;
            let valide = !nom.is_empty()
                && nom.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !nom.starts_with(|c: char| c.is_ascii_digit());
            valide.then(|| {
                (
                    nom.to_string(),
                    valeur.trim_matches(|c| c == '"' || c == '\'').to_string(),
                )
            })
        })
        .collect()
}

fn sans_affectations_de_tete(commande: &str) -> String {
    let n = affectations_de_tete(commande).len();
    commande
        .split_whitespace()
        .skip(n)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Retire les guillemets comme le shell, mot par mot.
///
/// `-Dapp.logging.path="/var/log/x/"` est, pour le shell,
/// `-Dapp.logging.path=/var/log/x/`. Ne retirer qu'un guillemet a chaque bout
/// du mot laissait celui du milieu : la commande d'asqatasun sortait pleine de
/// guillemets ouverts. Un mot aux guillemets non apparies fait partie d'une
/// chaine qui contient des espaces : il est laisse tel quel.
fn guillemets_du_shell(commande: &str) -> String {
    commande
        .split(' ')
        .map(|m| {
            let apparies = |q: char| m.matches(q).count() % 2 == 0;
            if (m.contains('"') || m.contains('\'')) && apparies('"') && apparies('\'') {
                m.replace(['"', '\''], "")
            } else {
                m.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ce qu'une commande lance vraiment, une fois retires les preparatifs et
/// deplies les scripts npm.
fn developper(commande: &str, facts: &RepoFacts) -> String {
    let mut c = derniere_etape(commande);
    // Un script npm peut en appeler un autre (`start` -> `serve`) : deux
    // niveaux suffisent, et bornent une boucle entre scripts.
    for _ in 0..2 {
        let Some(nom) = script_npm_appele(&c) else {
            break;
        };
        let Some(corps) = facts.stack.scripts_npm.get(nom) else {
            break;
        };
        c = derniere_etape(corps);
    }
    c
}

/// La derniere etape d'un enchainement, hors maintien artificiel en vie.
///
/// `tail -f /dev/null` et `sleep infinity` gardent un conteneur ouvert quand
/// le vrai processus se detache : ce n'est pas lui qu'il faut lancer.
fn derniere_etape(commande: &str) -> String {
    const MAINTIEN: &[&str] = &["tail -f /dev/null", "sleep infinity"];
    commande
        .split("&&")
        .flat_map(|s| s.split(';'))
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').trim())
        .filter(|s| !s.is_empty() && !MAINTIEN.contains(s))
        .last()
        .unwrap_or_else(|| commande.trim())
        .to_string()
}

/// Le script npm qu'une commande appelle, s'il y en a un.
fn script_npm_appele(commande: &str) -> Option<&str> {
    let mots: Vec<&str> = commande.split_whitespace().collect();
    match mots.as_slice() {
        ["npm" | "yarn" | "pnpm", "start", ..] => Some("start"),
        ["npm" | "yarn" | "pnpm", "run" | "run-script", nom, ..] => Some(nom),
        ["yarn" | "pnpm", nom, ..] => Some(nom),
        _ => None,
    }
}

fn traduire(commande: &str, tech: Technology, depot: &str) -> Option<String> {
    // `$PORT` d'abord : `-b 0.0.0.0:$PORT` doit devenir une adresse d'ecoute
    // que la suite reconnait et ramene sur la boucle locale. Dans l'ordre
    // inverse, InvenTree ecoutait sur `0.0.0.0:__PORT__` — toutes les
    // interfaces, nginx et l'authentification de YunoHost contournes.
    let commande = lier_le_port(commande);
    let commande = guillemets_du_shell(&commande);
    // Les affectations de tete (`HOSTNAME=0.0.0.0 node server.js`) passent
    // dans la configuration (voir `affectations_de_tete`) ; la commande se
    // traduit sans elles.
    let commande = sans_affectations_de_tete(&commande);
    let mots = sans_enveloppe(&mots_de(&commande));
    let premier = *mots.first()?;
    // Le nom seul : `/usr/local/bin/python3` est un python3 comme un autre.
    let nom = nom_de_fichier(premier);

    // Un script d'entree enveloppe le vrai demarrage : on ne sait pas ce qu'il
    // fait, et le deviner serait exactement ce que le projet refuse.
    if premier.ends_with(".sh") || premier.contains("entrypoint") {
        return None;
    }
    // Un orchestrateur demarre des conteneurs, pas l'application. Twenty
    // lance `podman-compose up -d`, que l'outil ecrivait tel quel dans le
    // service.
    const ORCHESTRATEURS: &[&str] = &[
        "docker",
        "docker-compose",
        "podman",
        "podman-compose",
        "nerdctl",
        "kubectl",
        "helm",
        "supervisord",
        "s6-svscan",
        "init",
    ];
    if ORCHESTRATEURS.contains(&nom) {
        return None;
    }
    // Un lanceur sans rien a lancer. Le Dockerfile de Dawarich a pour seul
    // point d'entree `bundle exec`, le reste venant du compose : l'ecrire
    // tel quel donnait un service qui ne demarre rien — alors que son
    // Procfile, lui, dit `bundle exec puma -C config/puma.rb`.
    if lanceur_tronque(&mots) {
        return None;
    }

    // Une liste de chemins separes par `:` — le classpath de Java,
    // `-cp /app/netrunner.jar:/app/config` — se traduit element par element.
    let reste = |depuis: usize| -> String {
        mots[depuis..]
            .iter()
            .map(|m| {
                if m.contains(':') && !m.contains("://") && m.split(':').any(|p| p.starts_with('/'))
                {
                    m.split(':').map(chemin).collect::<Vec<_>>().join(":")
                } else {
                    chemin(m)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    let traduite = match nom {
        // L'interprete vient de la resource du manifest, pas du systeme.
        "node" => Some(format!("__NODEJS_DIR__/node {}", reste(1))),
        // npx vit a cote de node ; il sert surtout aux migrations
        // (`npx prisma migrate deploy` chez documenso).
        "npx" => Some(format!("__NODEJS_DIR__/npx {}", reste(1))),
        "npm" | "yarn" | "pnpm" => None, // demarrer par un gestionnaire de paquets est deconseille
        "ruby" | "bundle" => Some(format!("__RUBY_DIR__/{nom} {}", reste(1))),
        // Java vient du paquet Debian ou du depot Adoptium que le manifeste
        // declare (voir `java`) : dans les deux cas, /usr/bin/java.
        "java" => Some(format!("/usr/bin/java {}", reste(1))),
        "php" | "php-fpm" => None, // pris en charge par la brique phpfpm
        "python" | "python3" => {
            // Pas de resource Python cote YunoHost : l'interprete vient d'un
            // environnement virtuel que le script d'installation cree.
            Some(format!("__INSTALL_DIR__/venv/bin/python {}", reste(1)))
        }
        // Les serveurs d'application Python s'installent dans ce meme
        // environnement virtuel, et s'y trouvent donc au meme endroit. Ce
        // n'est pas une supposition : c'est le script d'installation que
        // yunopack ecrit lui-meme qui les y met.
        "gunicorn" | "uvicorn" | "hypercorn" | "waitress-serve" | "daphne" | "celery" | "uwsgi"
        | "flask" | "granian"
            if tech == Technology::Python =>
        {
            Some(format!("__INSTALL_DIR__/venv/bin/{nom} {}", reste(1)))
        }
        // Dans une image Python, `/usr/local/bin` est l'endroit ou pip pose
        // les commandes d'un paquet. Chez YunoHost, c'est l'environnement
        // virtuel : Spodcast etait envoye a la racine de l'installation, ou
        // rien ne l'attend.
        _ if tech == Technology::Python && premier.starts_with("/usr/local/bin/") => {
            Some(format!("__INSTALL_DIR__/venv/bin/{nom} {}", reste(1)))
        }
        _ => {
            let binaire = chemin(premier);
            // Sans meme connaitre le langage, rien ne dit qu'un binaire du
            // conteneur est celui du projet — sauf s'il en porte le nom.
            // Amara demarre par `/usr/local/bin/entry`, un script de
            // l'image ; endlessh par `endlessh`, qui est bien le sien.
            if tech == Technology::Unknown && !porte_le_nom_du_depot(nom, depot) {
                return None;
            }
            // Un binaire hors des racines connues n'est pas localisable.
            if !binaire.starts_with("__INSTALL_DIR__") {
                if (tech == Technology::Go || tech == Technology::Rust)
                    && !est_un_outil_systeme(nom)
                {
                    // Un executable compile nomme sans chemin se trouve dans
                    // le repertoire d'installation.
                    // Avec ses arguments : calrs demarrait sans son `serve`.
                    return Some(
                        format!("__INSTALL_DIR__/{nom} {}", reste(1))
                            .trim()
                            .to_string(),
                    );
                }
                return None;
            }
            Some(if mots.len() > 1 {
                format!("{binaire} {}", reste(1))
            } else {
                binaire
            })
        }
    };
    traduite.map(|c| c.trim().to_string())
}

/// Vrai si un nom de binaire rappelle celui du depot.
fn porte_le_nom_du_depot(nom: &str, depot: &str) -> bool {
    let simple = |s: &str| s.to_lowercase().replace(['-', '_', '.'], "");
    let (binaire, projet) = (simple(nom), simple(depot));
    if binaire.is_empty() || projet.is_empty() {
        return false;
    }
    binaire.contains(&projet) || projet.contains(&binaire)
}

/// Vrai quand un interprete ou un lanceur n'a rien derriere lui.
fn lanceur_tronque(mots: &[&str]) -> bool {
    let noms: Vec<&str> = mots.iter().map(|m| nom_de_fichier(m)).collect();
    matches!(
        noms.as_slice(),
        ["bundle", "exec"]
            | ["node" | "ruby" | "python" | "python3" | "java" | "bundle"]
            | ["python" | "python3", "-m"]
            | ["java", "-jar"]
    )
}

/// Un outil du systeme, qu'aucun projet Go ou Rust ne livre sous ce nom.
///
/// La regle « un executable nomme sans chemin se trouve dans le repertoire
/// d'installation » ecrivait `__INSTALL_DIR__/bash` pour Crowdsec.
fn est_un_outil_systeme(nom: &str) -> bool {
    const SYSTEME: &[&str] = &[
        "bash", "sh", "curl", "wget", "cat", "echo", "sleep", "tail", "true", "su", "sudo", "cron",
        "crond", "nginx", "caddy", "apache2", "httpd",
    ];
    SYSTEME.contains(&nom)
}

/// `$PORT` dans une commande designe le port de l'application : c'est celui
/// que YunoHost reserve. Helpy demarre avec `unicorn_rails -p $PORT`, une
/// variable que rien ne definissait. `$DB_PORT` et consorts ne sont pas
/// touches : ce sont les ports d'autres services.
fn lier_le_port(commande: &str) -> String {
    let c = commande.replace("${PORT}", "__PORT__");
    let mut out = String::with_capacity(c.len());
    let mut reste = c.as_str();
    while let Some(i) = reste.find("$PORT") {
        let apres = &reste[i + 5..];
        let fin_de_nom = !apres.starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_');
        out.push_str(&reste[..i]);
        out.push_str(if fin_de_nom { "__PORT__" } else { "$PORT" });
        reste = apres;
    }
    out.push_str(reste);
    out
}

/// Retire les enveloppes qui n'ont de sens que dans un conteneur.
///
/// `tini` et `dumb-init` existent pour tenir le role de PID 1 et transmettre
/// les signaux : systemd le fait deja, et mieux. `gosu` et `su-exec` changent
/// d'utilisateur, ce que fait la directive `User=` du service. `env` ne fait
/// que chercher un binaire dans le PATH. Les garder ferait echouer la
/// Retire un changement d'utilisateur en tete de commande, avec ses
/// arguments : `gosu convertx`, `su-exec "$USER_ID:$GROUP_ID"`, `runuser -u
/// app --`, `setpriv --reuid=1000 --init-groups`.
///
/// systemd lance deja le service sous l'utilisateur de l'application. Laisse
/// en place, l'utilisateur — souvent une variable — passait pour la commande,
/// et le `$` qu'il contient faisait refuser la ligne entiere : booklore,
/// convertx et endurain s'arretaient la.
fn sans_changement_d_utilisateur(commande: &str) -> String {
    let mots: Vec<&str> = commande.split_whitespace().collect();
    let Some(premier) = mots.first() else {
        return commande.to_string();
    };
    let nom = nom_de_fichier(premier);
    let reste: &[&str] = match nom {
        // L'utilisateur, puis la commande.
        "gosu" | "su-exec" | "s6-setuidgid" => mots.get(2..).unwrap_or(&[]),
        // Options jusqu'a la commande ; `-u` prend une valeur.
        "chpst" | "runuser" | "setpriv" => {
            let mut i = 1;
            while i < mots.len() && mots[i].starts_with('-') {
                let avec_valeur = matches!(mots[i], "-u" | "-g" | "--user" | "--group");
                let fin = mots[i] == "--";
                i += if avec_valeur { 2 } else { 1 };
                if fin {
                    break;
                }
            }
            mots.get(i..).unwrap_or(&[])
        }
        _ => return commande.to_string(),
    };
    reste.join(" ")
}

/// traduction sur une commande qui, elle, se traduit tres bien.
fn sans_enveloppe<'a>(mots: &[&'a str]) -> Vec<&'a str> {
    // Les interpretes de commandes aussi : `sh -c node server.js` et
    // `/bin/dash /entrypoint.sh` ne disent rien du processus, seulement de ce
    // qui le lance.
    const ENVELOPPES: &[&str] = &[
        "tini",
        "dumb-init",
        "gosu",
        "su-exec",
        "env",
        "exec",
        "sh",
        "bash",
        "ash",
        "dash",
        "zsh",
    ];

    let mut reste = mots;
    loop {
        let Some((premier, suite)) = reste.split_first() else {
            return reste.to_vec();
        };
        let nom = nom_de_fichier(premier);
        if !ENVELOPPES.contains(&nom) {
            return reste.to_vec();
        }
        // `gosu` et `su-exec` prennent l'utilisateur en premier argument, et
        // `tini` separe ses options de la commande par `--`. On saute l'un
        // comme l'autre pour retomber sur la vraie commande.
        let mut suite = suite;
        if matches!(nom, "gosu" | "su-exec") {
            suite = suite.get(1..).unwrap_or(&[]);
        }
        while suite.first().is_some_and(|m| m.starts_with('-')) {
            suite = &suite[1..];
        }
        if suite.is_empty() {
            return reste.to_vec(); // rien derriere : ce n'etait pas une enveloppe
        }
        reste = suite;
    }
}

/// Replace un chemin de conteneur dans le repertoire d'installation.
pub(crate) fn chemin(mot: &str) -> String {
    if mot.starts_with('-') {
        return mot.to_string();
    }
    if let Some(ecoute) = adresse_d_ecoute(mot) {
        return ecoute;
    }
    for racine in RACINES {
        if let Some(reste) = mot.strip_prefix(racine) {
            return format!("__INSTALL_DIR__/{reste}");
        }
    }
    if let Some(reste) = mot.strip_prefix("./") {
        return format!("__INSTALL_DIR__/{reste}");
    }
    mot.to_string()
}

/// L'adresse d'ecoute d'un conteneur, ramenee a celle d'une app YunoHost.
///
/// Un conteneur ecoute sur toutes les interfaces et sur un port qu'il choisit.
/// Derriere le nginx de YunoHost, une app ecoute sur la boucle locale et sur
/// le port qui lui est attribue : laisser `0.0.0.0:7777` exposerait le service
/// en clair et sur le mauvais port. La reecriture est donc obligatoire, pas
/// cosmetique.
fn adresse_d_ecoute(mot: &str) -> Option<String> {
    let (hote, port) = mot.rsplit_once(':')?;
    let numero = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
    if !numero && port != "__PORT__" {
        return None;
    }
    match hote {
        "0.0.0.0" | "127.0.0.1" | "localhost" | "[::]" | "[::1]" | "" => {
            Some("127.0.0.1:__PORT__".to_string())
        }
        _ => None,
    }
}

fn nom_de_fichier(chemin: &str) -> &str {
    chemin.rsplit('/').next().unwrap_or(chemin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ynp_core::facts::{ArchAsset, BuildRecipe, CommandeDeclaree, SourceSelection, StackFacts};

    fn facts_avec(cmd: Option<Vec<String>>, tech: Technology) -> RepoFacts {
        RepoFacts {
            stack: StackFacts {
                primary: tech,
                ..Default::default()
            },
            build: Some(BuildRecipe {
                dockerfile_path: "Dockerfile".into(),
                cmd,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn facts_declarant(commande: &str, ou: &str, tech: Technology) -> RepoFacts {
        RepoFacts {
            stack: StackFacts {
                primary: tech,
                commande_declaree: Some(CommandeDeclaree {
                    commande: commande.into(),
                    ou: ou.into(),
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn facts_avec_script(cmd: Vec<&str>, exec: &str, tech: Technology) -> RepoFacts {
        let mut f = facts_avec(Some(cmd.into_iter().map(String::from).collect()), tech);
        f.stack.exec_du_point_d_entree = Some(exec.into());
        f
    }

    #[test]
    fn un_chemin_que_le_paquet_ne_cree_pas_remet_la_commande_en_question() {
        assert_eq!(
            chemin_etranger("__INSTALL_DIR__/crowdsec -c /etc/crowdsec/config.yaml"),
            Some("/etc/crowdsec/config.yaml".into())
        );
        assert_eq!(
            chemin_etranger("gunicorn app --worker-tmp-dir /dev/shm"),
            None
        );
        assert_eq!(
            chemin_etranger("/usr/bin/java -jar __INSTALL_DIR__/a.jar"),
            None
        );
    }

    #[test]
    fn un_hote_de_conteneur_dans_la_commande_la_remet_en_question() {
        assert_eq!(
            hote_du_conteneur("java -Djdbc.url=jdbc:mysql://asqatasun-db_dev:3306/a -jar x.war"),
            Some("asqatasun-db_dev".into())
        );
        assert_eq!(
            hote_du_conteneur("node a.js --url=http://localhost:8080/"),
            None
        );
        // Un domaine public n'est pas un conteneur.
        assert_eq!(
            hote_du_conteneur("app --api=https://api.example.com/v1"),
            None
        );
        assert_eq!(
            hote_du_conteneur("app --db=postgres://u:p@db:5432/x"),
            Some("db".into())
        );
    }

    #[test]
    fn les_guillemets_s_enlevent_comme_le_fait_le_shell() {
        assert_eq!(
            guillemets_du_shell(r#"java -Dpath="/var/log/x/" --server.port="8080" -jar a.jar"#),
            "java -Dpath=/var/log/x/ --server.port=8080 -jar a.jar"
        );
        assert_eq!(
            lier_les_options_d_ecoute("java --server.port=8080 -jar a.jar", &[8080]),
            "java --server.port=__PORT__ -jar a.jar"
        );
    }

    #[test]
    fn les_affectations_de_tete_ne_bloquent_pas_la_commande() {
        let mut f = RepoFacts::default();
        f.stack.primary = Technology::NodeJs;
        f.build = Some(Default::default());
        assert_eq!(
            traduire_avec("HOSTNAME=\"0.0.0.0\" node apps/web/server.js", &f).as_deref(),
            Some("__NODEJS_DIR__/node apps/web/server.js")
        );
        assert_eq!(
            affectations_de_tete("HOSTNAME=0.0.0.0 NODE_ENV=production node a.js"),
            vec![
                ("HOSTNAME".to_string(), "0.0.0.0".to_string()),
                ("NODE_ENV".to_string(), "production".to_string())
            ]
        );
    }

    #[test]
    fn le_classpath_se_traduit_element_par_element() {
        let mut f = RepoFacts::default();
        f.stack.primary = Technology::Java;
        f.build = Some(ynp_core::facts::BuildRecipe {
            workdir: Some("/app".into()),
            ..Default::default()
        });
        let c = traduire_avec("java -cp /app/netrunner.jar:/app/config web.core", &f).unwrap();
        assert!(!c.contains("/app/"), "{c}");
        assert!(c.starts_with("/usr/bin/java -cp "), "{c}");
    }

    #[test]
    fn un_changement_d_utilisateur_avant_le_cmd_ne_bloque_plus() {
        // Cas de booklore : `exec su-exec "$USER_ID:$GROUP_ID" "$@"`, CMD
        // `java -jar /app/app.jar`. systemd lance deja le service sous
        // l'utilisateur de l'application.
        let f = facts_avec_script(
            vec!["entrypoint.sh", "java", "-jar", "app.jar"],
            "su-exec \"$USER_ID:$GROUP_ID\" \"$@\"",
            Technology::Java,
        );
        let e = derive(&f, "booklore");
        assert!(
            e.value()
                .is_some_and(|c| c.contains("java") && c.contains("app.jar")),
            "{e:?}"
        );
        // gosu avec un utilisateur nomme : convertx.
        let f = facts_avec_script(
            vec!["/entrypoint.sh", "node", "dist/index.js"],
            "gosu convertx \"$@\"",
            Technology::NodeJs,
        );
        assert_eq!(
            derive(&f, "convertx").value().map(String::as_str),
            Some("__NODEJS_DIR__/node dist/index.js")
        );
    }

    #[test]
    fn un_script_qui_passe_la_main_au_cmd_ne_bloque_plus() {
        // Cas de zigbee2mqtt : `docker-entrypoint.sh /sbin/tini -- node index.js`
        // ou le script se termine par `exec "$@"`.
        let f = facts_avec_script(
            vec![
                "docker-entrypoint.sh",
                "/sbin/tini",
                "--",
                "node",
                "index.js",
            ],
            "\"$@\"",
            Technology::NodeJs,
        );
        assert_eq!(
            derive(&f, "zigbee2mqtt").value().map(String::as_str),
            Some("__NODEJS_DIR__/node index.js")
        );
    }

    #[test]
    fn un_script_qui_nomme_sa_commande_la_livre() {
        // Cas de Taiga : le script prepare le terrain puis `exec gosu taiga
        // gunicorn … --bind 0.0.0.0:8000`.
        let f = facts_avec_script(
            vec!["./docker/entrypoint.sh"],
            "gosu taiga gunicorn taiga.wsgi:application --bind 0.0.0.0:8000 --workers 3 \"$@\"",
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "taiga").value().map(String::as_str),
            Some(
                "__INSTALL_DIR__/venv/bin/gunicorn taiga.wsgi:application \
                 --bind 127.0.0.1:__PORT__ --workers 3"
            )
        );
    }

    #[test]
    fn une_derniere_ligne_qui_repasse_les_arguments_reprend_le_cmd() {
        // Cas de Kitchenowl : le script prepare la base puis finit par
        // `uwsgi "$@"`, sans `exec`. Les vrais arguments sont dans le CMD.
        let f = facts_avec_script(
            vec![
                "./entrypoint.sh",
                "--ini",
                "wsgi.ini:web",
                "--gevent",
                "200",
            ],
            "uwsgi \"$@\"",
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "kitchenowl").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/uwsgi --ini wsgi.ini:web --gevent 200")
        );
    }

    #[test]
    fn un_script_illisible_laisse_l_outil_demander() {
        // Sans ligne `exec`, rien n'a change : on ne devine pas ce que fait
        // le script.
        let f = facts_avec(Some(vec!["./entrypoint.sh".into()]), Technology::Python);
        assert!(derive(&f, "x").value().is_none());
    }

    fn facts_node(cmd: &str, scripts: &[(&str, &str)], deps: &[&str]) -> RepoFacts {
        let mut f = facts_avec(
            Some(cmd.split_whitespace().map(String::from).collect()),
            Technology::NodeJs,
        );
        f.stack.scripts_npm = scripts
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        f.stack.dependances_npm = deps.iter().map(|d| d.to_string()).collect();
        f
    }

    #[test]
    fn les_preparatifs_d_un_enchainement_sont_ecartes() {
        // Cas de Kutt : `npm run migrate && npm start`. La migration a sa
        // place dans le script d'installation ; le service lance le reste.
        let f = facts_node(
            "npm run migrate && npm start",
            &[
                ("migrate", "knex migrate:latest"),
                ("start", "node server/server.js"),
            ],
            &[],
        );
        assert_eq!(
            derive(&f, "kutt").value().map(String::as_str),
            Some("__NODEJS_DIR__/node server/server.js")
        );
    }

    #[test]
    fn un_outil_npm_appele_par_son_nom_vit_dans_node_modules() {
        let f = facts_node("npm start", &[("start", "next start")], &["next", "react"]);
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__NODEJS_DIR__/node __INSTALL_DIR__/node_modules/.bin/next start")
        );
    }

    #[test]
    fn un_maintien_artificiel_en_vie_n_est_pas_le_processus() {
        assert_eq!(
            derniere_etape("node app.js && tail -f /dev/null"),
            "node app.js"
        );
    }

    #[test]
    fn un_script_npm_absent_ne_fait_rien_inventer() {
        // Pas de `start` dans le package.json : on ne devine pas ce qu'il
        // lancerait, et npm lui-meme reste refuse.
        let f = facts_node("npm start", &[], &[]);
        assert!(derive(&f, "x").value().is_none());
    }

    #[test]
    fn un_interprete_de_commandes_s_efface_devant_ce_qu_il_lance() {
        let f = facts_avec(
            Some(vec![
                "sh".into(),
                "-c".into(),
                "\"node".into(),
                "index.js\"".into(),
            ]),
            Technology::NodeJs,
        );
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__NODEJS_DIR__/node index.js")
        );
    }

    fn cmd(mots: &[&str], tech: Technology) -> RepoFacts {
        facts_avec(Some(mots.iter().map(|m| m.to_string()).collect()), tech)
    }

    #[test]
    fn sans_langage_connu_seul_le_binaire_du_projet_est_retenu() {
        // Amara demarre par un script de l'image ; endlessh par son binaire.
        let mut amara = cmd(&["/usr/local/bin/entry", "app"], Technology::Unknown);
        amara.source.repo = "amara".into();
        assert!(derive(&amara, "amara").value().is_none());

        let mut endlessh = cmd(&["/usr/local/bin/endlessh", "-v"], Technology::Unknown);
        endlessh.source.repo = "endlessh".into();
        assert_eq!(
            derive(&endlessh, "endlessh").value().map(String::as_str),
            Some("__INSTALL_DIR__/endlessh -v")
        );
    }

    #[test]
    fn un_port_en_variable_n_ouvre_pas_toutes_les_interfaces() {
        // Cas d'InvenTree : `-b 0.0.0.0:$PORT`. L'ordre des reecritures
        // faisait ecouter l'application sur toutes les interfaces.
        let f = cmd(
            &["gunicorn", "app.wsgi", "-b", "0.0.0.0:$PORT"],
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/gunicorn app.wsgi -b 127.0.0.1:__PORT__")
        );
    }

    #[test]
    fn une_variable_fixee_par_le_dockerfile_est_remplacee() {
        let mut f = cmd(
            &["gunicorn", "--chdir", "$APP_HOME/src", "app.wsgi"],
            Technology::Python,
        );
        f.build
            .as_mut()
            .unwrap()
            .env
            .insert("APP_HOME".into(), "/app".into());
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/gunicorn --chdir __INSTALL_DIR__/src app.wsgi")
        );
    }

    #[test]
    fn une_variable_que_rien_ne_fixe_fait_renoncer() {
        let f = cmd(
            &["gunicorn", "--chdir", "$APP_HOME/src", "app.wsgi"],
            Technology::Python,
        );
        assert!(derive(&f, "x").value().is_none());
    }

    #[test]
    fn un_executable_declare_par_pyproject_et_lance_par_le_script_vit_dans_le_venv() {
        // Cas de Mealie : le script finit par `exec mealie`, et `mealie` est
        // declare dans [project.scripts].
        let mut f = facts_avec_script(vec!["/app/run.sh"], "mealie", Technology::Python);
        f.stack.scripts_python = vec!["mealie".into()];
        assert_eq!(
            derive(&f, "mealie").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/mealie")
        );
    }

    #[test]
    fn un_procfile_passe_devant_le_script_du_conteneur() {
        // Cas de Helpy : le script de l'image lance unicorn sans port, le
        // Procfile avec `-p $PORT`, comme YunoHost l'attend.
        let mut f = facts_avec_script(
            vec!["/bin/bash", "/helpy/docker/run.sh"],
            "bundle exec unicorn -E production -c config/unicorn.rb",
            Technology::Ruby,
        );
        f.stack.commande_declaree = Some(CommandeDeclaree {
            commande: "bundle exec unicorn_rails -p $PORT -c ./config/unicorn.rb".into(),
            ou: "Procfile".into(),
        });
        assert_eq!(
            derive(&f, "helpy").value().map(String::as_str),
            Some("__RUBY_DIR__/bundle exec unicorn_rails -p __PORT__ -c __INSTALL_DIR__/config/unicorn.rb")
        );
    }

    #[test]
    fn un_port_fige_sur_le_port_expose_devient_le_port_reserve() {
        // Cas de calrs : `serve --host 0.0.0.0 --port 3000`, EXPOSE 3000.
        let mut f = cmd(
            &[
                "calrs",
                "serve",
                "--host",
                "0.0.0.0",
                "--port",
                "3000",
                "--metrics-port",
                "9102",
            ],
            Technology::Rust,
        );
        f.build.as_mut().unwrap().expose = vec![3000];
        assert_eq!(
            derive(&f, "calrs").value().map(String::as_str),
            Some(
                "__INSTALL_DIR__/calrs serve --host 127.0.0.1 --port __PORT__ --metrics-port 9102"
            )
        );
    }

    #[test]
    fn un_chemin_de_volume_va_dans_le_repertoire_de_donnees() {
        // Cas de mautrix-googlechat : `-c /data/config.yaml`, VOLUME /data.
        let mut f = cmd(
            &[
                "python3",
                "-m",
                "mautrix_googlechat",
                "-c",
                "/data/config.yaml",
            ],
            Technology::Python,
        );
        f.build.as_mut().unwrap().volumes = vec!["/data".into()];
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some(
                "__INSTALL_DIR__/venv/bin/python -m mautrix_googlechat -c __DATA_DIR__/config.yaml"
            )
        );
    }

    #[test]
    fn un_orchestrateur_n_est_pas_l_application() {
        // Cas de Twenty : `podman-compose -f podman-compose.yml up -d`.
        let f = cmd(
            &["podman-compose", "-f", "podman-compose.yml", "up", "-d"],
            Technology::NodeJs,
        );
        assert!(derive(&f, "twenty").value().is_none());
    }

    #[test]
    fn un_lanceur_tronque_laisse_parler_le_procfile() {
        // Cas de Dawarich : ENTRYPOINT `bundle exec`, le reste dans le
        // compose. Son Procfile dit la commande entiere.
        let mut f = cmd(&["bundle", "exec"], Technology::Ruby);
        f.stack.commande_declaree = Some(CommandeDeclaree {
            commande: "bundle exec puma -C config/puma.rb".into(),
            ou: "Procfile".into(),
        });
        assert_eq!(
            derive(&f, "dawarich").value().map(String::as_str),
            Some("__RUBY_DIR__/bundle exec puma -C config/puma.rb")
        );
    }

    #[test]
    fn une_commande_pip_d_une_image_python_vit_dans_le_venv() {
        // Cas de Spodcast : `/usr/local/bin/spodcast`.
        let f = cmd(&["/usr/local/bin/spodcast"], Technology::Python);
        assert_eq!(
            derive(&f, "spodcast").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/spodcast")
        );
    }

    #[test]
    fn la_variable_port_d_une_commande_devient_le_port_reserve() {
        // Cas de Helpy : `unicorn_rails -p $PORT`.
        assert_eq!(lier_le_port("x -p $PORT -c y"), "x -p __PORT__ -c y");
        assert_eq!(lier_le_port("x --port=${PORT}"), "x --port=__PORT__");
        // Le port d'un autre service n'est pas touche.
        assert_eq!(
            lier_le_port("x --db $DB_PORT $PORT_HTTPS"),
            "x --db $DB_PORT $PORT_HTTPS"
        );
    }

    #[test]
    fn un_outil_systeme_n_est_pas_le_binaire_du_projet() {
        // Cas de Crowdsec : `bash` devenait `__INSTALL_DIR__/bash`.
        let f = cmd(&["curl", "http://x"], Technology::Go);
        assert!(derive(&f, "x").value().is_none());
    }

    #[test]
    fn les_enveloppes_de_conteneur_sont_retirees() {
        // Cas de Dashy : `/sbin/tini -- node server.js`. tini n'existe que
        // pour tenir le role de PID 1 ; systemd le tient deja.
        let f = facts_avec(
            Some(vec![
                "/sbin/tini".into(),
                "--".into(),
                "node".into(),
                "server.js".into(),
            ]),
            Technology::NodeJs,
        );
        assert_eq!(
            derive(&f, "dashy").value().map(String::as_str),
            Some("__NODEJS_DIR__/node server.js")
        );
    }

    #[test]
    fn gosu_saute_l_utilisateur_qu_il_prend_en_argument() {
        let f = facts_avec(
            Some(vec![
                "gosu".into(),
                "appuser".into(),
                "node".into(),
                "index.js".into(),
            ]),
            Technology::NodeJs,
        );
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__NODEJS_DIR__/node index.js")
        );
    }

    #[test]
    fn une_enveloppe_sans_rien_derriere_n_en_est_pas_une() {
        // `env` seul n'enveloppe rien : le retirer laisserait une commande
        // vide, et inventer la suite serait une supposition.
        let f = facts_avec(Some(vec!["env".into()]), Technology::NodeJs);
        assert!(derive(&f, "x").value().is_none());
    }

    #[test]
    fn un_procfile_dispense_de_demander_la_commande() {
        // Sans Dockerfile, l'outil demandait — en citant le Procfile dans les
        // pistes, qu'il n'ouvrait pas.
        let f = facts_declarant(
            "gunicorn app:server -b 0.0.0.0:8000",
            "Procfile",
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "x").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/gunicorn app:server -b 127.0.0.1:__PORT__")
        );
    }

    #[test]
    fn un_point_d_entree_python_est_propose_sans_etre_retenu() {
        // Cas de Sosse : son seul point d'entree, `sosse-admin`, est l'outil
        // d'administration. Le retenir aurait ecrit un service qui affiche
        // son aide et s'arrete. Il est propose, avec sa raison.
        let f = facts_declarant(
            "sosse-admin",
            "pyproject.toml, [project.scripts]",
            Technology::Python,
        );
        let r = derive(&f, "x");
        assert!(r.value().is_none(), "{r:?}");
        let c = &r.reason().unwrap().candidates;
        assert_eq!(c[0].value, "__INSTALL_DIR__/venv/bin/sosse-admin");
        assert!(c[0].why.contains("pas forcement le service"));
    }

    #[test]
    fn sans_dockerfile_ni_declaration_l_outil_demande() {
        let f = RepoFacts::default();
        let r = derive(&f, "x");
        assert!(r.value().is_none());
        assert!(r
            .reason()
            .unwrap()
            .unknown
            .contains("ni commande de demarrage declaree"));
    }

    #[test]
    fn un_serveur_python_est_cherche_dans_le_venv() {
        // Cas de SimpleLogin. gunicorn n'est pas sur le PATH du systeme : il
        // est installe dans l'environnement virtuel, au meme titre que python.
        let f = facts_avec(
            Some(vec![
                "gunicorn".into(),
                "wsgi:app".into(),
                "-b".into(),
                "0.0.0.0:7777".into(),
                "-w".into(),
                "2".into(),
            ]),
            Technology::Python,
        );
        assert_eq!(
            derive(&f, "simplelogin").value().map(String::as_str),
            Some("__INSTALL_DIR__/venv/bin/gunicorn wsgi:app -b 127.0.0.1:__PORT__ -w 2")
        );
    }

    #[test]
    fn le_venv_ne_vaut_que_pour_python() {
        // L'environnement virtuel n'existe que pour une application Python :
        // y chercher un binaire d'un autre monde serait une supposition.
        let f = facts_avec(Some(vec!["gunicorn".into()]), Technology::Go);
        let r = derive(&f, "x");
        assert!(!r.value().is_some_and(|v| v.contains("venv")), "{r:?}");
    }

    #[test]
    fn l_adresse_du_conteneur_devient_celle_de_l_app() {
        for a in ["0.0.0.0:8080", "127.0.0.1:3000", "[::]:9000", ":5000"] {
            assert_eq!(
                adresse_d_ecoute(a).as_deref(),
                Some("127.0.0.1:__PORT__"),
                "{a}"
            );
        }
    }

    #[test]
    fn ce_qui_n_est_pas_une_adresse_est_laisse_tel_quel() {
        // Un module Python, une image, un argument avec deux-points : rien de
        // tout cela n'est une adresse d'ecoute.
        for a in ["wsgi:app", "app.main:app", "node:20", "--timeout:15"] {
            assert_eq!(adresse_d_ecoute(a), None, "{a}");
        }
    }

    #[test]
    fn un_binaire_publie_est_localise_sans_interpretation() {
        // Cas de miniflux et gotify : l'amont publie un executable, renomme
        // d'apres l'app a l'installation.
        let mut f = facts_avec(None, Technology::Go);
        f.selection = Some(SourceSelection {
            prebuilt: vec![ArchAsset {
                arch: "amd64".into(),
                extract: false,
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(
            derive(&f, "miniflux").value().unwrap(),
            "__INSTALL_DIR__/miniflux"
        );
    }

    #[test]
    fn un_demarrage_node_passe_par_la_resource_du_manifest() {
        let f = facts_avec(
            Some(vec!["node".into(), "./dist/main.js".into()]),
            Technology::NodeJs,
        );
        assert_eq!(
            derive(&f, "app").value().unwrap(),
            "__NODEJS_DIR__/node __INSTALL_DIR__/dist/main.js"
        );
    }

    #[test]
    fn un_chemin_de_conteneur_est_replace_dans_le_repertoire_d_installation() {
        let f = facts_avec(
            Some(vec!["/usr/local/bin/buzz-relay".into()]),
            Technology::Rust,
        );
        assert_eq!(
            derive(&f, "buzz").value().unwrap(),
            "__INSTALL_DIR__/buzz-relay"
        );
    }

    #[test]
    fn un_script_d_entree_n_est_pas_devine() {
        // On ne sait pas ce que fait entrypoint.sh, et l'inventer serait
        // exactement ce que le projet refuse.
        let f = facts_avec(
            Some(vec!["/usr/local/memos/entrypoint.sh".into()]),
            Technology::Go,
        );
        let r = derive(&f, "memos");
        assert!(!r.is_resolved());
        assert!(r.reason().unwrap().unknown.contains("non transposable"));
    }

    #[test]
    fn l_absence_de_cmd_est_signalee_avec_ou_chercher() {
        let f = facts_avec(None, Technology::NodeJs);
        let r = derive(&f, "app");
        assert!(!r.is_resolved());
        assert!(r.reason().unwrap().look_in.iter().any(|s| s == "Procfile"));
    }

    #[test]
    fn un_executable_compile_sans_chemin_est_cherche_dans_l_installation() {
        let f = facts_avec(
            Some(vec!["gotify-app".into(), "serve".into()]),
            Technology::Go,
        );
        // Les arguments suivent : ce test les laissait tomber, et avec eux le
        // `serve` sans lequel l'application ne demarre pas.
        assert_eq!(
            derive(&f, "gotify").value().unwrap(),
            "__INSTALL_DIR__/gotify-app serve"
        );
    }

    #[test]
    fn php_fpm_n_est_pas_un_service_systemd() {
        // La brique phpfpm de YunoHost s'en charge.
        let f = facts_avec(Some(vec!["php-fpm".into()]), Technology::Php);
        assert!(!derive(&f, "app").is_resolved());
    }
}
