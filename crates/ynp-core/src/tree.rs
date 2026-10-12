//! Le contenu d'un depot, en memoire.
//!
//! Les detecteurs sont des fonctions pures de cet arbre vers des faits. Les
//! garder a l'ecart du reseau et du disque est ce qui permet de les tester un
//! par un sur des arborescences fabriquees, et de repartir le travail entre
//! plusieurs agents sans qu'ils se genent.

use std::collections::{BTreeMap, BTreeSet};

/// Le contenu n'est conserve que pour les fichiers utiles aux detecteurs ; les
/// autres chemins sont memorises sans leur contenu. Un depot de 50 000 fichiers
/// tient ainsi en memoire sans qu'on perde l'information « ce fichier existe ».
#[derive(Debug, Clone, Default)]
pub struct RepoTree {
    files: BTreeMap<String, String>,
    paths: BTreeSet<String>,
    /// Variables d'environnement lues par le code, et le premier fichier qui
    /// les lit. Le contenu des sources n'est pas garde, seulement ceci.
    lectures: BTreeMap<String, String>,
    /// Comment le code choisit son port d'ecoute, et le premier fichier qui
    /// le montre. Meme principe : le contenu n'est pas garde.
    ecoutes: BTreeMap<crate::ecoutes::Indice, String>,
    /// Noms de moteurs de base cites par le code (`'pgsql'`, `pg: {`), et
    /// combien de fois : c'est le vocabulaire de l'application.
    moteurs_cites: BTreeMap<String, usize>,
    /// Les valeurs par defaut que le code donne aux variables qu'il lit.
    defauts_lus: BTreeMap<String, String>,
    /// Les objets d'application web des modules Python : (chemin, objet,
    /// cadre).
    applications_web: Vec<(String, String, String)>,
    /// Les prefixes sous lesquels un cadre de configuration lit
    /// l'environnement : `Env::prefixed("CONTINUWUITY_")` (Figment).
    prefixes_env: BTreeSet<String>,
}

impl RepoTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl Into<String>, content: impl Into<String>) {
        let path = path.into();
        let content = content.into();
        self.noter_les_lectures(&path, &content);
        self.paths.insert(path.clone());
        self.files.insert(path, content);
    }

    /// Releve les variables qu'un fichier source lit, sans garder le fichier.
    pub fn noter_les_lectures(&mut self, path: &str, contenu: &str) {
        if !crate::lectures::est_un_source_de_production(path) {
            return;
        }
        for i in crate::ecoutes::indices(contenu) {
            self.ecoutes.entry(i).or_insert_with(|| path.to_string());
        }
        if path.ends_with(".py") {
            for (objet, cadre) in crate::lectures::applications_web(contenu) {
                self.applications_web.push((path.to_string(), objet, cadre));
            }
        }
        for p in crate::lectures::prefixes_env(contenu) {
            self.prefixes_env.insert(p);
        }
        for m in crate::lectures::moteurs_cites(contenu) {
            *self.moteurs_cites.entry(m).or_default() += 1;
        }
        // Prefiltre : la plupart des fichiers ne lisent aucune variable, et
        // les expressions regulieres n'ont pas a les parcourir.
        if !(contenu.contains("env") || contenu.contains("ENV")) {
            return;
        }
        for nom in crate::lectures::variables_lues(contenu) {
            self.lectures.entry(nom).or_insert_with(|| path.to_string());
        }
        for (nom, valeur) in crate::lectures::defauts_lus(contenu) {
            self.defauts_lus.entry(nom).or_insert(valeur);
        }
    }

    /// Noms de moteurs de base cites par le code, avec leur nombre.
    pub fn moteurs_cites(&self) -> &BTreeMap<String, usize> {
        &self.moteurs_cites
    }

    /// Indices d'ecoute releves dans le code, avec le fichier qui les montre.
    pub fn ecoutes(&self) -> &BTreeMap<crate::ecoutes::Indice, String> {
        &self.ecoutes
    }

    /// Les objets d'application web des modules Python.
    pub fn applications_web(&self) -> &[(String, String, String)] {
        &self.applications_web
    }

    /// Valeurs par defaut donnees par le code aux variables qu'il lit.
    pub fn prefixes_env(&self) -> &BTreeSet<String> {
        &self.prefixes_env
    }

    pub fn defauts_lus(&self) -> &BTreeMap<String, String> {
        &self.defauts_lus
    }

    /// Variables lues par le code, avec le fichier qui les lit.
    pub fn lectures(&self) -> &BTreeMap<String, String> {
        &self.lectures
    }

    /// Memorise l'existence d'un fichier sans conserver son contenu.
    pub fn add_path(&mut self, path: impl Into<String>) {
        self.paths.insert(path.into());
    }

    /// Construit un arbre a partir de paires (chemin, contenu).
    pub fn from_pairs<P, C>(pairs: impl IntoIterator<Item = (P, C)>) -> Self
    where
        P: Into<String>,
        C: Into<String>,
    {
        let mut t = Self::new();
        for (p, c) in pairs {
            t.insert(p, c);
        }
        t
    }

    /// Le sous-arbre d'un repertoire, chemins relatifs a lui : le serveur
    /// d'un monorepo (`backend/`) s'analyse comme un depot a part entiere.
    pub fn sous_arbre(&self, repertoire: &str) -> RepoTree {
        let prefixe = format!("{}/", repertoire.trim_end_matches('/'));
        let sous = |p: &str| p.strip_prefix(&prefixe).map(str::to_string);
        let mut t = RepoTree::new();
        for p in &self.paths {
            if let Some(q) = sous(p) {
                t.paths.insert(q);
            }
        }
        for (p, c) in &self.files {
            if let Some(q) = sous(p) {
                t.files.insert(q, c.clone());
            }
        }
        for (n, p) in &self.lectures {
            if let Some(q) = sous(p) {
                t.lectures.insert(n.clone(), q);
            }
        }
        for (i, p) in &self.ecoutes {
            if let Some(q) = sous(p) {
                t.ecoutes.insert(i.clone(), q);
            }
        }
        for (c, o, k) in &self.applications_web {
            if let Some(q) = sous(c) {
                t.applications_web.push((q, o.clone(), k.clone()));
            }
        }
        t.moteurs_cites = self.moteurs_cites.clone();
        t.defauts_lus = self.defauts_lus.clone();
        t
    }

    pub fn paths(&self) -> Vec<String> {
        self.paths.iter().cloned().collect()
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    pub fn has(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// Premier chemin existant parmi les candidats, dans l'ordre de preference.
    pub fn first_of(&self, candidates: &[&str]) -> Option<String> {
        candidates
            .iter()
            .find(|p| self.has(p))
            .map(|p| p.to_string())
    }

    /// Contenu du premier candidat present, avec son chemin.
    pub fn first_text(&self, candidates: &[&str]) -> Option<(String, &str)> {
        let p = self.first_of(candidates)?;
        let t = self.text(&p)?;
        Some((p, t))
    }

    /// Chemins dont le nom de fichier satisfait le predicat.
    pub fn find_by_name(&self, pred: impl Fn(&str) -> bool) -> Vec<String> {
        self.paths
            .iter()
            .filter(|p| p.rsplit('/').next().is_some_and(&pred))
            .cloned()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> RepoTree {
        RepoTree::from_pairs([
            ("package.json", "{}"),
            ("src/index.js", "x"),
            ("docker/Dockerfile", "FROM x"),
        ])
    }

    #[test]
    fn l_arbre_repond_sur_la_presence_et_le_contenu() {
        let t = tree();
        assert!(t.has("package.json"));
        assert_eq!(t.text("package.json"), Some("{}"));
        assert_eq!(t.text("absent"), None);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn le_premier_candidat_present_gagne_dans_l_ordre_donne() {
        let t = tree();
        assert_eq!(
            t.first_of(&["absent", "package.json", "src/index.js"])
                .as_deref(),
            Some("package.json")
        );
        assert_eq!(t.first_of(&["absent"]), None);
        assert_eq!(t.first_text(&["package.json"]).unwrap().1, "{}");
    }

    #[test]
    fn un_chemin_peut_exister_sans_que_son_contenu_soit_conserve() {
        // Cas des fichiers binaires ou volumineux : le detecteur doit pouvoir
        // repondre « ce fichier existe » sans qu'on ait garde ses octets.
        let mut t = tree();
        t.add_path("assets/logo.png");
        assert!(t.has("assets/logo.png"));
        assert_eq!(t.text("assets/logo.png"), None);
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn la_recherche_par_nom_ignore_le_repertoire() {
        let t = tree();
        assert_eq!(
            t.find_by_name(|f| f.starts_with("Dockerfile")),
            vec!["docker/Dockerfile"]
        );
    }
}
