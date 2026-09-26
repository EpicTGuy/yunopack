//! Cache des evaluations de faisabilite et des icones.
//!
//! Evaluer un depot coute une poignee d'appels a l'API GitHub et le
//! telechargement de son archive — une dizaine de secondes. La liste de
//! souhaits en compte plus de cinq cents : les evaluer a chaque affichage est
//! hors de question, et l'API non authentifiee plafonne a soixante appels par
//! heure. Le cache n'est donc pas une optimisation, c'est ce qui rend la
//! fonctionnalite possible.
//!
//! Il vit sur disque, dans le repertoire de donnees de l'application, pour
//! survivre a un redemarrage : une evaluation perdue, c'est un quota gaspille.
//!
//! La peremption se juge sur `pushed_at` du depot amont, que l'API rend en un
//! seul appel. Tant que l'amont n'a pas bouge, l'evaluation reste vraie.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Ce qu'on retient d'une evaluation, et de quoi juger si elle est perimee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    /// `owner/repo` en minuscules.
    pub depot: String,
    pub url: String,
    pub verdict: String,
    pub score: u8,
    /// Ce qui interdit le packaging. Vide quand le verdict est favorable.
    #[serde(default)]
    pub bloquants: Vec<Motif>,
    /// Ce qui ne l'interdit pas mais coutera du travail.
    #[serde(default)]
    pub reserves: Vec<Motif>,
    /// Ce qui merite d'etre su sans rien coûter : depot sans activite
    /// recente, absence de SSO. C'est la que se lit l'etat de maintenance.
    #[serde(default)]
    pub remarques: Vec<Motif>,
    /// Depot archive par son auteur : le signal d'abandon le plus sur.
    #[serde(default)]
    pub archive: bool,
    #[serde(default)]
    pub etoiles: u32,
    /// Ce qui renseigne sur la conduite du projet. Aucune base libre ne
    /// l'encode ; ces faits-la, eux, sont lisibles par une machine.
    #[serde(default)]
    pub gouvernance: Gouvernance,
    /// Technologie et source retenues, utiles a l'affichage.
    #[serde(default)]
    pub technologie: String,
    #[serde(default)]
    pub reference: String,
    /// Dernier push amont connu au moment de l'evaluation. C'est lui qui dit
    /// si le resultat est encore d'actualite.
    #[serde(default)]
    pub pushed_at: String,
    pub evalue_le: String,
}

/// Ce qu'on peut etablir de la conduite d'un projet sans rien deviner.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Gouvernance {
    /// Depot dont celui-ci est une fourche, ex. `go-gitea/gitea`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fourche_de: Option<String>,
    /// Le depot appartient a une organisation plutot qu'a une personne.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proprietaire_collectif: Option<bool>,
    /// Le projet s'est donne des regles ecrites, et lesquelles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chartes: Vec<String>,
    /// Notes OpenSSF, quand le depot y figure. Son absence ne dit rien.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scorecard: Option<ynp_forge::scorecard::Scorecard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Motif {
    pub id: String,
    /// Texte francais, tel que la regle le rend.
    pub titre: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remede: String,
    /// Les autres langues, par code. Retenues avec le constat plutot que
    /// traduites a l'affichage : une fiche est mise en cache et partagee entre
    /// instances, qui n'ont pas toutes la meme langue.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub textes: std::collections::BTreeMap<String, Texte>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Texte {
    pub titre: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remede: String,
}

/// Une evaluation qui a echoue, retenue elle aussi.
///
/// Sans cela, un depot disparu ou prive serait retente a chaque affichage, et
/// consommerait le quota au profit de personne.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Echec {
    pub depot: String,
    pub url: String,
    pub erreur: String,
    pub evalue_le: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "etat", rename_all = "snake_case")]
pub enum Fiche {
    // Une evaluation reussie pese plusieurs fois l'echec qu'elle remplace ;
    // la mettre derriere un pointeur evite que chaque fiche, meme vide, en
    // occupe la taille. Elles vivent en lot dans le cache.
    Evaluee(Box<Evaluation>),
    Echouee(Echec),
}

impl Fiche {
    pub fn depot(&self) -> &str {
        match self {
            Fiche::Evaluee(e) => &e.depot,
            Fiche::Echouee(e) => &e.depot,
        }
    }

    fn pushed_at(&self) -> &str {
        match self {
            Fiche::Evaluee(e) => &e.pushed_at,
            Fiche::Echouee(_) => "",
        }
    }
}

/// Le cache, adosse a un repertoire.
#[derive(Clone)]
pub struct Cache {
    evaluations: PathBuf,
    icones: PathBuf,
}

impl Cache {
    pub fn new(racine: &Path) -> Self {
        let c = Self {
            evaluations: racine.join("evaluations"),
            icones: racine.join("icones"),
        };
        // Un cache qu'on ne peut pas ecrire n'est pas une raison de refuser de
        // demarrer : l'application marche sans, plus lentement.
        let _ = std::fs::create_dir_all(&c.evaluations);
        let _ = std::fs::create_dir_all(&c.icones);
        c
    }

    pub fn lire(&self, depot: &str) -> Option<Fiche> {
        let texte = std::fs::read_to_string(self.fichier(depot)).ok()?;
        serde_json::from_str(&texte).ok()
    }

    pub fn ecrire(&self, fiche: &Fiche) {
        if let Ok(texte) = serde_json::to_string_pretty(fiche) {
            let _ = std::fs::write(self.fichier(fiche.depot()), texte);
        }
    }

    /// Vrai si l'evaluation en cache ne vaut plus pour ce `pushed_at`.
    pub fn perimee(&self, depot: &str, pushed_at: &str) -> bool {
        match self.lire(depot) {
            None => true,
            Some(f) => f.pushed_at() != pushed_at,
        }
    }

    /// Toutes les fiches connues, pour un lot de depots.
    pub fn lot(&self, depots: &[String]) -> Vec<Fiche> {
        depots.iter().filter_map(|d| self.lire(d)).collect()
    }

    /// Verse un lot de fiches venues d'ailleurs, sans ecraser ce qu'on sait.
    ///
    /// Une evaluation locale a ete faite sur cette machine, a une date connue ;
    /// une evaluation partagee vient d'ailleurs. En cas de doublon, la locale
    /// gagne — non parce qu'elle vaut mieux, mais parce qu'on sait d'ou elle
    /// vient. Rend le nombre de fiches reellement ajoutees.
    pub fn verser(&self, fiches: Vec<Fiche>) -> usize {
        let mut ajoutees = 0;
        for f in fiches {
            if self.lire(f.depot()).is_none() {
                self.ecrire(&f);
                ajoutees += 1;
            }
        }
        ajoutees
    }

    /// Toutes les fiches du cache, pour republier ce qu'on a appris.
    pub fn toutes(&self) -> Vec<Fiche> {
        let Ok(entrees) = std::fs::read_dir(&self.evaluations) else {
            return Vec::new();
        };
        entrees
            .filter_map(Result::ok)
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| serde_json::from_str(&t).ok())
            .collect()
    }

    pub fn nombre(&self) -> usize {
        std::fs::read_dir(&self.evaluations)
            .map(|d| d.filter_map(Result::ok).count())
            .unwrap_or(0)
    }

    pub fn icone(&self, proprietaire: &str) -> Option<Vec<u8>> {
        std::fs::read(self.fichier_icone(proprietaire)).ok()
    }

    pub fn ecrire_icone(&self, proprietaire: &str, octets: &[u8]) {
        let _ = std::fs::write(self.fichier_icone(proprietaire), octets);
    }

    fn fichier(&self, depot: &str) -> PathBuf {
        self.evaluations
            .join(format!("{}.json", nom_de_fichier(depot)))
    }

    fn fichier_icone(&self, proprietaire: &str) -> PathBuf {
        self.icones
            .join(format!("{}.img", nom_de_fichier(proprietaire)))
    }
}

/// Un nom de fichier sur : le depot vient du reseau, il ne doit designer que
/// ce qu'on veut dans le repertoire du cache.
fn nom_de_fichier(depot: &str) -> String {
    depot
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('.')
        .to_string()
}

/// `owner/repo` en minuscules, a partir de n'importe quelle URL de forge.
pub fn depot_de(url: &str) -> Option<String> {
    let sans = url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(url);
    let mut parties = sans.split('/');
    let _hote = parties.next()?;
    let owner = parties.next().filter(|s| !s.is_empty())?;
    let repo = parties.next().filter(|s| !s.is_empty())?;
    Some(format!("{}/{}", owner.to_lowercase(), repo.to_lowercase()))
}

pub fn maintenant() -> String {
    // Une date ISO sans dependance supplementaire : seconde epoch suffit a
    // ordonner, et le client la met en forme.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (Cache, tempdir::Repertoire) {
        let r = tempdir::Repertoire::new();
        (Cache::new(r.chemin()), r)
    }

    fn evaluation(depot: &str, pushed: &str) -> Fiche {
        Fiche::Evaluee(Box::new(Evaluation {
            depot: depot.into(),
            url: format!("https://github.com/{depot}"),
            verdict: "FAISABLE".into(),
            score: 100,
            bloquants: Vec::new(),
            reserves: Vec::new(),
            remarques: Vec::new(),
            archive: false,
            etoiles: 0,
            gouvernance: Gouvernance::default(),
            technologie: "Go".into(),
            reference: "v1".into(),
            pushed_at: pushed.into(),
            evalue_le: maintenant(),
        }))
    }

    #[test]
    fn une_fiche_ecrite_se_relit() {
        let (c, _g) = cache();
        c.ecrire(&evaluation("miniflux/v2", "2026-01-01"));
        match c.lire("miniflux/v2") {
            Some(Fiche::Evaluee(e)) => assert_eq!(e.score, 100),
            autre => panic!("attendu une evaluation, trouve {autre:?}"),
        }
    }

    #[test]
    fn un_depot_inconnu_ne_rend_rien() {
        let (c, _g) = cache();
        assert!(c.lire("x/y").is_none());
    }

    #[test]
    fn une_evaluation_se_perime_quand_l_amont_bouge() {
        let (c, _g) = cache();
        c.ecrire(&evaluation("a/b", "2026-01-01"));
        assert!(!c.perimee("a/b", "2026-01-01"));
        assert!(c.perimee("a/b", "2026-02-01"), "l'amont a bouge");
        assert!(c.perimee("jamais/vu", "2026-01-01"));
    }

    #[test]
    fn un_echec_est_retenu_lui_aussi() {
        // Sinon un depot prive serait retente a chaque affichage, au detriment
        // du quota de tout le monde.
        let (c, _g) = cache();
        c.ecrire(&Fiche::Echouee(Echec {
            depot: "prive/depot".into(),
            url: "https://github.com/prive/depot".into(),
            erreur: "404".into(),
            evalue_le: maintenant(),
        }));
        assert!(matches!(c.lire("prive/depot"), Some(Fiche::Echouee(_))));
    }

    #[test]
    fn un_nom_de_depot_ne_peut_pas_sortir_du_repertoire() {
        // Le depot vient du reseau : il ne doit designer qu'un fichier du
        // cache. On verifie la propriete, pas une chaine : c'est elle qui
        // compte, et elle doit tenir pour toutes les formes d'attaque.
        for hostile in [
            "../../etc/passwd",
            "..",
            ".",
            "a/../../b",
            "/etc/shadow",
            "c:\\windows\\system32",
            "a\u{0}b",
        ] {
            let n = nom_de_fichier(hostile);
            assert!(!n.contains('/'), "{hostile} → {n} : traverse un repertoire");
            assert!(!n.contains('\\'), "{hostile} → {n} : separateur Windows");
            assert!(
                n != ".." && n != ".",
                "{hostile} → {n} : composant de remontee"
            );
            assert!(!n.starts_with('.'), "{hostile} → {n} : fichier cache");
        }
        assert_eq!(nom_de_fichier("a/b"), "a_b");
        assert_eq!(nom_de_fichier("Owner/Repo"), "owner_repo");
    }

    #[test]
    fn le_depot_se_deduit_de_n_importe_quelle_forme_d_url() {
        for u in [
            "https://github.com/Miniflux/v2",
            "https://github.com/miniflux/v2/",
            "https://github.com/miniflux/v2.git",
            "http://github.com/miniflux/v2/tree/main",
        ] {
            assert_eq!(depot_de(u).as_deref(), Some("miniflux/v2"), "{u}");
        }
        assert_eq!(depot_de("https://github.com/"), None);
        assert_eq!(depot_de("pas une url"), None);
    }

    #[test]
    fn le_lot_ne_rend_que_ce_qui_est_connu() {
        let (c, _g) = cache();
        c.ecrire(&evaluation("a/b", "x"));
        let f = c.lot(&["a/b".into(), "c/d".into()]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].depot(), "a/b");
    }

    #[test]
    fn verser_ajoute_l_inconnu_sans_ecraser_le_connu() {
        // Une evaluation locale a ete faite ici, a une date connue ; une
        // partagee vient d'ailleurs. En cas de doublon, la locale gagne.
        let (c, _g) = cache();
        c.ecrire(&evaluation("a/local", "date-locale"));

        let n = c.verser(vec![
            evaluation("a/local", "date-etrangere"),
            evaluation("b/neuf", "x"),
        ]);
        assert_eq!(n, 1, "seule la fiche inconnue est versee");
        match c.lire("a/local") {
            Some(Fiche::Evaluee(e)) => assert_eq!(e.pushed_at, "date-locale"),
            autre => panic!("{autre:?}"),
        }
        assert!(c.lire("b/neuf").is_some());
    }

    #[test]
    fn toutes_rend_ce_qui_a_ete_ecrit() {
        let (c, _g) = cache();
        c.ecrire(&evaluation("a/b", "x"));
        c.ecrire(&evaluation("c/d", "y"));
        let mut depots: Vec<String> = c.toutes().iter().map(|f| f.depot().to_string()).collect();
        depots.sort();
        assert_eq!(depots, vec!["a/b", "c/d"]);
    }

    #[test]
    fn un_cache_vide_ne_rend_rien_plutot_que_de_paniquer() {
        let (c, _g) = cache();
        assert!(c.toutes().is_empty());
        assert_eq!(c.verser(Vec::new()), 0);
    }

    #[test]
    fn une_icone_ecrite_se_relit() {
        let (c, _g) = cache();
        c.ecrire_icone("miniflux", b"\x89PNG");
        assert_eq!(c.icone("miniflux").as_deref(), Some(&b"\x89PNG"[..]));
        assert!(c.icone("inconnu").is_none());
    }

    /// Un repertoire temporaire efface a la fin du test.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Repertoire(PathBuf);

        impl Repertoire {
            pub fn new() -> Self {
                // Un compteur, pas seulement l'horloge : les tests tournent en
                // parallele et deux d'entre eux peuvent demarrer dans la meme
                // nanoseconde — ils partageraient alors leur cache.
                use std::sync::atomic::{AtomicU64, Ordering};
                static SUIVANT: AtomicU64 = AtomicU64::new(0);
                let n = SUIVANT.fetch_add(1, Ordering::Relaxed);
                let pid = std::process::id();
                let p = std::env::temp_dir().join(format!("yunopack-cache-{pid}-{n}"));
                std::fs::create_dir_all(&p).expect("repertoire de test");
                Self(p)
            }
            pub fn chemin(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Repertoire {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
