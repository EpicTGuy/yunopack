//! Evaluation de faisabilite en tache de fond, et icones des projets.
//!
//! L'utilisateur parcourt une liste de plusieurs centaines d'applications ; il
//! veut savoir lesquelles valent la peine avant d'en ouvrir une. Evaluer les
//! cinq cents d'un coup est impossible — l'API GitHub non authentifiee accorde
//! soixante appels par heure, une evaluation en coute trois.
//!
//! D'ou ce compromis : on evalue ce qu'on regarde, une a la fois, et on retient
//! le resultat. La liste se remplit au fil de la navigation, et le quota
//! epuise n'empeche pas de lire ce qui est deja connu.

use crate::cache::{maintenant, Cache, Echec, Evaluation, Fiche, Gouvernance, Motif};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Nombre de demandes en attente au-dela duquel on refuse d'empiler.
///
/// Une file sans borne se remplirait d'evaluations que personne n'attend plus,
/// et le quota irait a des lignes que l'utilisateur a quittees depuis
/// longtemps.
const FILE_MAX: usize = 40;

#[derive(Clone)]
pub struct Evaluateur {
    cache: Cache,
    /// Ce que l'utilisateur regarde maintenant. Prioritaire.
    file: Arc<Mutex<VecDeque<String>>>,
    /// Le defrichage de fond, qui remplit le cache sans que personne attende.
    fond: Arc<Mutex<VecDeque<String>>>,
    /// Reveille l'executant des qu'une demande arrive, plutot que de le faire
    /// scruter la file.
    signal: Arc<tokio::sync::Notify>,
}

fn verrou<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Evaluateur {
    pub fn new(cache: Cache) -> Self {
        let e = Self {
            cache,
            file: Arc::new(Mutex::new(VecDeque::new())),
            fond: Arc::new(Mutex::new(VecDeque::new())),
            signal: Arc::new(tokio::sync::Notify::new()),
        };
        tokio::spawn(e.clone().executer());
        e
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Met une URL en file d'attente. Sans effet si elle y est deja, si elle
    /// est deja connue, ou si la file deborde.
    ///
    /// Rend l'etat, pour que l'interface dise ce qui se passe.
    pub fn demander(&self, url: &str, refaire: bool) -> &'static str {
        let Some(depot) = crate::cache::depot_de(url) else {
            return "url_invalide";
        };
        // Une reevaluation explicite passe outre le cache : c'est le seul moyen
        // de tenir compte d'une mise a jour amont sans depenser un appel d'API
        // par depot et par affichage, ce que le quota interdit.
        if !refaire && self.cache.lire(&depot).is_some() {
            return "deja_connue";
        }
        let mut f = verrou(&self.file);
        if f.iter().any(|u| u == url) {
            return "en_attente";
        }
        if f.len() >= FILE_MAX {
            return "file_pleine";
        }
        f.push_back(url.to_string());
        drop(f);
        self.signal.notify_one();
        "en_attente"
    }

    pub fn en_attente(&self) -> usize {
        verrou(&self.file).len()
    }

    /// Une evaluation a la fois : c'est le quota de la forge qui commande, pas
    /// la puissance de la machine.
    ///
    /// Les demandes de l'utilisateur passent devant le defrichage de fond : il
    /// attend une reponse, la tache de fond non.
    async fn executer(self) {
        loop {
            let suivante = verrou(&self.file)
                .pop_front()
                .or_else(|| verrou(&self.fond).pop_front());
            match suivante {
                None => self.signal.notified().await,
                Some(url) => {
                    let fiche = evaluer(&url).await;
                    self.cache.ecrire(&fiche);
                    // Une evaluation qui echoue sur le quota ne doit pas
                    // enchainer sur la suivante : elle echouerait pareil, et la
                    // liste entiere se remplirait d'echecs en quelques secondes.
                    if quota_epuise(&fiche) {
                        tracing::warn!("quota de la forge epuise — pause de 15 minutes");
                        tokio::time::sleep(std::time::Duration::from_secs(900)).await;
                    }
                }
            }
        }
    }

    /// Met en file de fond tout ce qui n'est pas encore connu.
    ///
    /// Sans cela, une installation neuve n'affiche aucun score tant que
    /// personne n'a fait defiler les listes. Le defrichage tourne au rythme que
    /// le quota permet et ne gene pas les demandes directes, qui passent
    /// devant.
    pub fn defricher(&self, urls: Vec<String>) -> usize {
        let mut f = verrou(&self.fond);
        let mut ajoutees = 0;
        for url in urls {
            let Some(depot) = crate::cache::depot_de(&url) else {
                continue;
            };
            if self.cache.lire(&depot).is_some() || f.iter().any(|u| u == &url) {
                continue;
            }
            f.push_back(url);
            ajoutees += 1;
        }
        drop(f);
        if ajoutees > 0 {
            self.signal.notify_one();
        }
        ajoutees
    }

    pub fn reste_a_defricher(&self) -> usize {
        verrou(&self.fond).len()
    }
}

/// Vrai si l'echec vient du quota de la forge plutot que du depot.
fn quota_epuise(fiche: &Fiche) -> bool {
    match fiche {
        Fiche::Echouee(e) => {
            let m = e.erreur.to_lowercase();
            m.contains("quota") || m.contains("rate limit") || m.contains("403")
        }
        Fiche::Evaluee(_) => false,
    }
}

/// Telecharge un lot d'evaluations deja faites ailleurs et le verse au cache.
///
/// Une evaluation est le meme calcul, sur le meme depot, avec les memes
/// regles : la refaire sur chaque instance depense du quota et du courant pour
/// arriver au meme resultat. Elle est verifiable — n'importe qui peut la
/// rejouer — donc la partager ne demande pas de faire confiance a qui l'a
/// produite. Les fiches deja connues localement ne sont pas ecrasees.
pub async fn verser_le_partage(cache: &Cache, url: &str) -> Result<usize, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("yunopack/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let r = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("{} a repondu {}", url, r.status()));
    }
    let fiches: Vec<Fiche> = r.json().await.map_err(|e| e.to_string())?;
    Ok(cache.verser(fiches))
}

/// Deroule la moitie amont du pipeline : recuperation, analyse, regles.
///
/// C'est exactement ce que fait `yunopack assess`. Le score et les motifs
/// affiches dans les listes sont donc les memes que ceux du pipeline, et non
/// une estimation faite a part qui pourrait diverger.
pub async fn evaluer(url: &str) -> Fiche {
    let depot = crate::cache::depot_de(url).unwrap_or_else(|| url.to_string());

    let recupere = match ynp_forge::fetch(url).await {
        Ok(r) => r,
        Err(e) => {
            return Fiche::Echouee(Echec {
                depot,
                url: url.to_string(),
                erreur: e.to_string(),
                evalue_le: maintenant(),
            })
        }
    };

    let pushed_at = recupere.forge.meta.pushed_at.clone().unwrap_or_default();
    let archive = recupere.forge.meta.archived;
    let etoiles = recupere.forge.meta.stars;
    let reference = recupere.choice.reference.clone();

    // Des regles ecrites ne garantissent rien, mais leur absence sur un projet
    // de taille se remarque. C'est un fait, pas un jugement.
    let chartes: Vec<String> = [
        "GOVERNANCE.md",
        "CODE_OF_CONDUCT.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
    ]
    .iter()
    .filter(|f| {
        recupere.tree.has(f)
            || recupere.tree.has(&format!(".github/{f}"))
            || recupere.tree.has(&format!("docs/{f}"))
    })
    .map(|f| f.to_string())
    .collect();

    let gouvernance = Gouvernance {
        fourche_de: recupere.forge.meta.fourche_de.clone(),
        proprietaire_collectif: recupere.forge.meta.proprietaire_collectif,
        chartes,
        // Un appel de plus, mis en cache comme le reste. Le service ne couvre
        // que GitHub, et seulement une partie de ses depots : une absence
        // n'est pas un mauvais signe, c'est une absence.
        scorecard: ynp_forge::scorecard::pour(&depot).await,
    };
    let faits = ynp_analyze::analyze(recupere.forge, &recupere.tree);
    let technologie = faits.stack.primary.to_string();

    let (faisabilite, _) =
        ynp_rules::gates::evaluate(&faits, ynp_core::DEFAULT_FEASIBILITY_THRESHOLD);

    // Le constat est retenu dans les trois langues : il est mis en cache et
    // partage entre instances, qui n'ont pas toutes la meme. Le traduire a
    // l'affichage demanderait de le reevaluer pour chaque langue.
    let motifs = |s: ynp_core::Severity| -> Vec<Motif> {
        faisabilite
            .findings
            .iter()
            .filter(|f| f.severity == s)
            .map(|f| {
                let mut textes = std::collections::BTreeMap::new();
                for langue in ["en", "es"] {
                    let (titre, remede) = ynp_core::traduction::traduire(f, langue);
                    textes.insert(
                        langue.to_string(),
                        crate::cache::Texte {
                            titre,
                            remede: remede.unwrap_or_default(),
                        },
                    );
                }
                Motif {
                    id: f.id.clone(),
                    titre: f.title.clone(),
                    remede: f.remediation.clone().unwrap_or_default(),
                    textes,
                }
            })
            .collect()
    };

    Fiche::Evaluee(Box::new(Evaluation {
        depot,
        url: url.to_string(),
        verdict: faisabilite.verdict.label().to_string(),
        score: faisabilite.score,
        bloquants: motifs(ynp_core::Severity::Blocker),
        reserves: motifs(ynp_core::Severity::Major),
        remarques: motifs(ynp_core::Severity::Minor),
        archive,
        etoiles,
        gouvernance,
        technologie,
        reference,
        pushed_at,
        evalue_le: maintenant(),
    }))
}

/// L'icone d'un projet : l'avatar du compte qui heberge le depot.
///
/// C'est le seul visuel qu'une forge expose de facon uniforme, et pour un
/// projet auto-heberge c'est presque toujours son logo. On le relaie plutot
/// que de le laisser charger par le navigateur : sans cela, afficher la liste
/// enverrait a GitHub l'adresse IP du visiteur et la liste de ce qu'il
/// consulte.
pub async fn icone(cache: &Cache, url: &str) -> Option<Vec<u8>> {
    let depot = crate::cache::depot_de(url)?;
    let proprietaire = depot.split('/').next()?.to_string();

    if let Some(octets) = cache.icone(&proprietaire) {
        // Un enregistrement vide note une absence : ne pas la retenir ferait
        // redemander a chaque affichage une image qui n'existe pas.
        return (!octets.is_empty()).then_some(octets);
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("yunopack/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let reponse = client
        .get(format!("https://github.com/{proprietaire}.png?size=80"))
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success());

    let octets = match reponse {
        Some(r) => r.bytes().await.ok().map(|b| b.to_vec()).unwrap_or_default(),
        None => Vec::new(),
    };

    cache.ecrire_icone(&proprietaire, &octets);
    (!octets.is_empty()).then_some(octets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluateur() -> Evaluateur {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SUIVANT: AtomicU64 = AtomicU64::new(0);
        let racine = std::env::temp_dir().join(format!(
            "yunopack-eval-{}-{}",
            std::process::id(),
            SUIVANT.fetch_add(1, Ordering::Relaxed)
        ));
        Evaluateur {
            cache: Cache::new(&racine),
            file: Arc::new(Mutex::new(VecDeque::new())),
            fond: Arc::new(Mutex::new(VecDeque::new())),
            signal: Arc::new(tokio::sync::Notify::new()),
        }
    }

    #[test]
    fn une_url_invalide_est_refusee_sans_occuper_la_file() {
        let e = evaluateur();
        assert_eq!(e.demander("pas une url", false), "url_invalide");
        assert_eq!(e.en_attente(), 0);
    }

    #[test]
    fn une_meme_url_ne_s_empile_pas_deux_fois() {
        let e = evaluateur();
        assert_eq!(e.demander("https://github.com/a/b", false), "en_attente");
        assert_eq!(e.demander("https://github.com/a/b", false), "en_attente");
        assert_eq!(e.en_attente(), 1);
    }

    #[test]
    fn un_depot_deja_connu_n_est_pas_reevalue() {
        let e = evaluateur();
        e.cache.ecrire(&Fiche::Echouee(Echec {
            depot: "a/b".into(),
            url: "https://github.com/a/b".into(),
            erreur: "404".into(),
            evalue_le: maintenant(),
        }));
        assert_eq!(e.demander("https://github.com/a/b", false), "deja_connue");
        assert_eq!(e.en_attente(), 0);
    }

    #[test]
    fn une_reevaluation_explicite_passe_outre_le_cache() {
        // C'est le seul moyen de tenir compte d'une mise a jour amont :
        // verifier automatiquement couterait un appel d'API par depot.
        let e = evaluateur();
        e.cache.ecrire(&Fiche::Echouee(Echec {
            depot: "a/b".into(),
            url: "https://github.com/a/b".into(),
            erreur: "404".into(),
            evalue_le: maintenant(),
        }));
        assert_eq!(e.demander("https://github.com/a/b", true), "en_attente");
        assert_eq!(e.en_attente(), 1);
    }

    #[test]
    fn le_defrichage_ignore_ce_qui_est_deja_connu_et_les_doublons() {
        let e = evaluateur();
        e.cache.ecrire(&Fiche::Echouee(Echec {
            depot: "a/connu".into(),
            url: "https://github.com/a/connu".into(),
            erreur: "404".into(),
            evalue_le: maintenant(),
        }));
        let n = e.defricher(vec![
            "https://github.com/a/connu".into(),
            "https://github.com/a/neuf".into(),
            "https://github.com/a/neuf".into(),
            "pas une url".into(),
        ]);
        assert_eq!(n, 1);
        assert_eq!(e.reste_a_defricher(), 1);
    }

    #[test]
    fn un_echec_de_quota_se_distingue_d_un_depot_introuvable() {
        // Enchainer apres un quota epuise remplirait la liste d'echecs faux.
        let quota = Fiche::Echouee(Echec {
            depot: "a/b".into(),
            url: String::new(),
            erreur: "quota d'API GitHub epuise".into(),
            evalue_le: String::new(),
        });
        let absent = Fiche::Echouee(Echec {
            depot: "a/b".into(),
            url: String::new(),
            erreur: "depot introuvable (404)".into(),
            evalue_le: String::new(),
        });
        assert!(quota_epuise(&quota));
        assert!(!quota_epuise(&absent));
    }

    #[test]
    fn la_file_refuse_de_deborder() {
        // Au-dela, ce sont des lignes que l'utilisateur a quittees : les
        // evaluer gaspillerait le quota.
        let e = evaluateur();
        for i in 0..FILE_MAX {
            assert_eq!(
                e.demander(&format!("https://github.com/a/r{i}"), false),
                "en_attente"
            );
        }
        assert_eq!(
            e.demander("https://github.com/a/trop", false),
            "file_pleine"
        );
        assert_eq!(e.en_attente(), FILE_MAX);
    }
}
