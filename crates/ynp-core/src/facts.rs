//! `RepoFacts` : ce qui EST dans le depot, sans interpretation.
//!
//! Regle de separation stricte du pipeline : ce module ne contient que de la
//! collecte. Aucune decision de packaging ne s'y prend — elles sont toutes dans
//! [`AppSpec`](crate::spec::AppSpec). Cette frontiere est ce qui rend les
//! detecteurs testables un par un et parallelisables entre agents.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const FACTS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFacts {
    pub schema_version: u32,
    pub source: SourceRef,
    pub meta: RepoMeta,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub releases: Vec<Release>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Chemins relatifs du depot. Sert aux detecteurs « ce fichier existe-t-il ».
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree: Vec<String>,
    /// Recette extraite du Dockerfile. Le signal le plus riche du projet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildRecipe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose: Option<ComposeFacts>,
    pub stack: StackFacts,
    pub services: ServiceFacts,
    pub config: ConfigFacts,
    /// Comment le code choisit son port d'ecoute.
    #[serde(default, skip_serializing_if = "EcouteFacts::est_vide")]
    pub ecoute: EcouteFacts,
    /// Source retenue, renseignee par `ynp-forge` apres telechargement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SourceSelection>,
}

impl RepoFacts {
    pub fn new(source: SourceRef) -> Self {
        Self {
            schema_version: FACTS_SCHEMA_VERSION,
            source,
            ..Default::default()
        }
    }

    pub fn has_file(&self, path: &str) -> bool {
        self.tree.iter().any(|p| p == path)
    }

    pub fn has_any(&self, paths: &[&str]) -> bool {
        paths.iter().any(|p| self.has_file(p))
    }

    /// Premier fichier existant parmi les candidats, dans l'ordre de preference.
    pub fn first_of(&self, paths: &[&str]) -> Option<String> {
        paths
            .iter()
            .find(|p| self.has_file(p))
            .map(|p| p.to_string())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub forge: Forge,
    pub owner: String,
    pub repo: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    #[default]
    GitHub,
    GitLab,
    Gitea,
    Forgejo,
    /// Hote non identifiable a son nom de domaine. Une instance auto-hebergee
    /// doit etre interrogee pour qu'on sache quelle forge elle fait tourner.
    Inconnue,
}

impl Forge {
    /// Suffixe de strategie d'autoupdate attendu par l'outillage YunoHost,
    /// cf. `autoupdate.strategy = "latest_<forge>_release"`.
    pub fn autoupdate_slug(self) -> &'static str {
        match self {
            Forge::GitHub => "github",
            Forge::GitLab => "gitlab",
            Forge::Gitea => "gitea",
            Forge::Forgejo => "forgejo",
            Forge::Inconnue => "inconnue",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Premiere phrase de prose du README, quand la forge ne dit rien. Elle
    /// est proposee a l'humain, jamais retenue d'office.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_readme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
    pub stars: u32,
    pub archived: bool,
    /// Date ISO-8601 du dernier push, pour la regle d'abandon MAINT001.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_at: Option<String>,
    /// Identifiant SPDX tel que rendu par la forge, ex. `AGPL-3.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_spdx: Option<String>,
    /// Texte integral du fichier de licence amont, que le paquet doit inclure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_text: Option<String>,

    // --- Ce qui renseigne sur la conduite du projet ---
    //
    // Aucune base libre n'encode la gouvernance d'un logiciel. Ces quatre
    // faits, eux, sont lisibles par une machine et disent quelque chose de
    // reel : qui tient le depot, s'il descend d'un autre, et si le projet
    // s'est donne des regles ecrites. A chacun d'en tirer ses conclusions.
    /// Depot dont celui-ci est une fourche, ex. `go-gitea/gitea` pour Forgejo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fourche_de: Option<String>,
    /// Vrai si le depot appartient a une organisation plutot qu'a une
    /// personne. Un projet porte par un seul compte n'a pas le meme avenir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proprietaire_collectif: Option<bool>,
    /// Nombre de contributeurs distincts, quand la forge le rend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributeurs: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub prerelease: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<ReleaseAsset>,
    /// Tarball genere par la forge. Toujours disponible, meme sans asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tarball_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// L'empreinte que la forge publie elle-meme, si elle le fait.
    ///
    /// GitHub la donne sur chaque actif de publication. La lire dispense de
    /// telecharger le fichier pour la calculer — et ces fichiers pesent
    /// souvent des centaines de megaoctets : FlareSolverr en publie un de
    /// 263 Mo, que l'outil rapatriait en entier a chaque analyse, sans rien
    /// afficher pendant ce temps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Ce qu'un Dockerfile dit du build, transpose en donnees.
///
/// C'est la piece qui remplace le LLM : un Dockerfile est deja une
/// specification de construction lisible par une machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecipe {
    pub dockerfile_path: String,
    /// Fichiers que le Dockerfile copie depuis le depot, par leur chemin dans
    /// l'image. `COPY docker/entry.sh /app/run.sh` : le point d'entree
    /// `/app/run.sh` est en realite `docker/entry.sh`, et c'est lui qu'il faut
    /// lire pour savoir ce que lance l'image.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub copies: IndexMap<String, String>,
    /// Ce que l'etape finale recupere des etapes de construction, rapporte au
    /// depot : `COPY --from=builder /build/target/release/app /usr/local/bin/app`
    /// devient `target/release/app` vers `/usr/local/bin/app`. Hors de Docker,
    /// c'est une copie a refaire apres la construction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies_d_etape: Vec<CopieDEtape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<BuildStage>,
    /// Paquets Debian, extraits litteralement des `apt-get install`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apt_packages: Vec<String>,
    /// Paquets Alpine, a traduire via assets/knowledge/apk-to-deb.toml.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apk_packages: Vec<String>,
    /// Lignes RUN utiles au build, hors installation de paquets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expose: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
}

impl BuildRecipe {
    /// Image de la derniere etape : celle qui porte le runtime, pas le builder.
    pub fn runtime_stage(&self) -> Option<&BuildStage> {
        self.stages.last()
    }

    /// Commande de demarrage, ENTRYPOINT et CMD concatenes comme le ferait Docker.
    pub fn start_command(&self) -> Option<String> {
        match (&self.entrypoint, &self.cmd) {
            (Some(e), Some(c)) if !e.is_empty() => Some(format!("{} {}", e.join(" "), c.join(" "))),
            (Some(e), None) if !e.is_empty() => Some(e.join(" ")),
            (_, Some(c)) if !c.is_empty() => Some(c.join(" ")),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildStage {
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeFacts {
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ComposeService>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeService {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Vrai si le service est construit depuis le depot : c'est l'app elle-meme,
    /// par opposition aux services d'infrastructure (base, cache).
    pub is_app: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environment: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Technology {
    Php,
    #[serde(rename = "nodejs")]
    NodeJs,
    Python,
    Go,
    Ruby,
    Rust,
    Java,
    /// Fichiers statiques servis directement par nginx.
    Static,
    #[default]
    Unknown,
}

impl Technology {
    /// La resource `[resources.X]` correspondante du manifest, si elle existe.
    /// Python n'en a pas : c'est tout l'objet de la regle PY001.
    pub fn manifest_resource(self) -> Option<&'static str> {
        match self {
            Technology::NodeJs => Some("nodejs"),
            Technology::Ruby => Some("ruby"),
            Technology::Go => Some("go"),
            _ => None,
        }
    }

    /// Toutes les technologies qu'un humain peut choisir, `Unknown` exclue :
    /// la repondre ne renseignerait rien.
    pub const CHOIX: &'static [Technology] = &[
        Technology::Php,
        Technology::NodeJs,
        Technology::Python,
        Technology::Go,
        Technology::Ruby,
        Technology::Rust,
        Technology::Java,
        Technology::Static,
    ];

    /// Le nom canonique, celui qui s'ecrit dans `appspec.toml`.
    pub fn nom(self) -> &'static str {
        match self {
            Technology::Php => "php",
            Technology::NodeJs => "nodejs",
            Technology::Python => "python",
            Technology::Go => "go",
            Technology::Ruby => "ruby",
            Technology::Rust => "rust",
            Technology::Java => "java",
            Technology::Static => "static",
            Technology::Unknown => "unknown",
        }
    }

    /// L'inverse de [`Technology::nom`], tolerante sur la casse et sur les
    /// appellations courantes — un humain ecrit « Node.js », pas « nodejs ».
    pub fn from_nom(s: &str) -> Option<Technology> {
        let n: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        Some(match n.as_str() {
            "php" => Technology::Php,
            "nodejs" | "node" | "js" | "javascript" | "typescript" => Technology::NodeJs,
            "python" | "py" => Technology::Python,
            "go" | "golang" => Technology::Go,
            "ruby" | "rb" => Technology::Ruby,
            "rust" | "rs" => Technology::Rust,
            "java" | "kotlin" => Technology::Java,
            "static" | "statique" | "html" => Technology::Static,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Technology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Technology::Php => "PHP",
            Technology::NodeJs => "Node.js",
            Technology::Python => "Python",
            Technology::Go => "Go",
            Technology::Ruby => "Ruby",
            Technology::Rust => "Rust",
            Technology::Java => "Java",
            Technology::Static => "fichiers statiques",
            Technology::Unknown => "non identifiee",
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackFacts {
    pub primary: Technology,
    /// Version exigee par l'upstream, ex. `20` pour Node, `8.2` pour PHP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_managers: Vec<String>,
    pub has_lockfile: bool,
    /// Commande de build declaree par l'upstream (`scripts.build`, cible Makefile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_script: Option<String>,
    /// Dependances npm/pip exigeant des paquets `-dev` cote systeme.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_deps: Vec<String>,
    /// La ligne `exec` du script d'entree de l'image, telle qu'elle y figure.
    /// `"$@"` y signifie « lance le CMD » ; toute autre valeur nomme la
    /// commande directement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_du_point_d_entree: Option<String>,
    /// Les migrations que le script d'entree lance avant son `exec`, telles
    /// qu'elles y figurent : `node build/index.js --migrate`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub migrations_du_point_d_entree: Vec<String>,
    /// La contrainte PHP de `composer.json`, telle quelle : `^7.4` n'admet
    /// que PHP 7, `>=7.4` admet aussi PHP 8. La version seule ne le dit pas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrainte_php: Option<String>,
    /// Extensions PHP que le projet declare : exigences `ext-*` de son
    /// `composer.json` et `docker-php-ext-install` de son Dockerfile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions_php: Vec<String>,
    /// Les executables que le `pyproject.toml` declare. Un script d'entree
    /// qui finit par `exec mealie` lance l'un d'eux : il vit dans
    /// l'environnement virtuel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts_python: Vec<String>,
    /// Les scripts du `package.json` racine. `npm start` n'est pas une
    /// commande qu'on puisse ecrire dans un service, mais ce qu'il lance l'est,
    /// et c'est le projet qui le declare.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub scripts_npm: std::collections::BTreeMap<String, String>,
    /// Les dependances du `package.json` racine, pour reconnaitre un outil
    /// installe dans `node_modules/.bin` quand un script l'appelle par son nom.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependances_npm: Vec<String>,
    /// Le langage que le depot annonce a sa racine quand YunoHost ne sait pas
    /// le provisionner. Sa presence invalide les pistes ci-dessous : elles ne
    /// designeraient qu'un composant secondaire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub langage_hors_perimetre: Option<String>,
    /// La commande de demarrage que le projet declare lui-meme, quand il le
    /// fait ailleurs que dans un Dockerfile, et d'ou elle vient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commande_declaree: Option<CommandeDeclaree>,
    /// Technologies reperees hors de la racine, de la mieux attestee a la
    /// moins. Renseignee seulement quand la racine n'a rien appris : elle
    /// sert alors a proposer, pas a conclure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pistes: Vec<PisteTechnologie>,
}

/// Un fichier que l'etape finale d'une image recupere d'une etape de
/// construction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopieDEtape {
    /// Chemin dans le depot, une fois construit.
    pub depuis: String,
    /// Chemin dans l'image.
    pub vers: String,
}

/// Une commande de demarrage que le projet enonce, et la source qui l'enonce.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandeDeclaree {
    pub commande: String,
    pub ou: String,
}

/// Une technologie apercue quelque part dans le depot, et ou.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PisteTechnologie {
    pub technologie: Technology,
    /// Le ou les fichiers qui l'attestent, tels qu'on les montrera.
    pub ou: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Database {
    #[default]
    None,
    #[serde(rename = "mysql")]
    MySql,
    #[serde(rename = "postgresql")]
    PostgreSql,
    /// Fichier local : rien a provisionner cote YunoHost.
    Sqlite,
    #[serde(rename = "mongodb")]
    MongoDb,
    /// Detectee mais hors du perimetre YunoHost (Elasticsearch, ClickHouse...).
    Unsupported,
}

impl Database {
    /// Valeur de `[resources.database] type`, si la base est provisionnable
    /// par le coeur de YunoHost.
    pub fn manifest_type(self) -> Option<&'static str> {
        match self {
            Database::MySql => Some("mysql"),
            Database::PostgreSql => Some("postgresql"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceFacts {
    pub database: Database,
    /// Nom du service tel que vu dans compose, pour la tracabilite du constat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_evidence: Option<String>,
    pub needs_redis: bool,
    /// Le projet fait tourner un stockage objet S3 (minio, SeaweedFS,
    /// Garage) a cote de l'application : le paquet en embarque un.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_s3: bool,
    /// Services detectes sans equivalent YunoHost. Alimente la regle DB002.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
    /// Le runtime lui-meme exige docker-compose : blocage RUN001.
    pub requires_container_runtime: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFacts {
    /// Fichier d'exemple trouve (`.env.example`, `config.sample.yml`...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example_file: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<ConfigVar>,
}

impl ConfigFacts {
    pub fn get(&self, role: ConfigRole) -> Option<&ConfigVar> {
        self.variables.iter().find(|v| v.role == role)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigVar {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Role deduit via assets/knowledge/env-vars.toml.
    pub role: ConfigRole,
    /// Valeur a generer aleatoirement plutot qu'a recopier.
    pub secret: bool,
    /// Fichier d'ou la variable a ete lue. Une proposition faite a l'humain
    /// vaut surtout par sa provenance : il peut aller verifier.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Vrai quand le projet ne fixe pas la variable, mais seulement la
    /// mentionne : ligne commentee d'un fichier d'exemple, tableau de README,
    /// lecture dans le code. Lire une variable ne veut pas dire l'exiger.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub facultative: bool,
}

/// Role canonique d'une variable de configuration.
///
/// Reconnaitre `PORT`, `DATABASE_URL` ou `APP_URL` est ce qui permet de cabler
/// la configuration de l'app sur les valeurs YunoHost sans rien deviner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigRole {
    Port,
    DatabaseUrl,
    DatabaseHost,
    DatabaseName,
    DatabaseUser,
    DatabasePassword,
    BaseUrl,
    Secret,
    AdminEmail,
    AdminUser,
    AdminPassword,
    DataPath,
    SmtpHost,
    LogLevel,
    #[default]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_commande_de_demarrage_combine_entrypoint_et_cmd() {
        let mut r = BuildRecipe {
            entrypoint: Some(vec!["node".into()]),
            cmd: Some(vec!["server.js".into()]),
            ..Default::default()
        };
        assert_eq!(r.start_command().as_deref(), Some("node server.js"));

        r.entrypoint = None;
        assert_eq!(r.start_command().as_deref(), Some("server.js"));

        r.cmd = None;
        assert_eq!(r.start_command(), None);
    }

    #[test]
    fn l_etape_de_runtime_est_la_derniere_pas_le_builder() {
        let r = BuildRecipe {
            stages: vec![
                BuildStage {
                    image: "node".into(),
                    tag: Some("20".into()),
                    alias: Some("builder".into()),
                },
                BuildStage {
                    image: "nginx".into(),
                    tag: Some("alpine".into()),
                    alias: None,
                },
            ],
            ..Default::default()
        };
        assert_eq!(r.runtime_stage().unwrap().image, "nginx");
    }

    #[test]
    fn python_n_a_pas_de_resource_manifest_contrairement_a_node() {
        assert_eq!(Technology::NodeJs.manifest_resource(), Some("nodejs"));
        assert_eq!(Technology::Python.manifest_resource(), None);
    }

    #[test]
    fn seules_mysql_et_postgres_sont_provisionnables_par_le_coeur() {
        assert_eq!(Database::MySql.manifest_type(), Some("mysql"));
        assert_eq!(Database::Sqlite.manifest_type(), None);
        assert_eq!(Database::Unsupported.manifest_type(), None);
    }

    #[test]
    fn les_faits_repondent_sur_la_presence_de_fichiers() {
        let mut f = RepoFacts::new(SourceRef::default());
        f.tree = vec!["Dockerfile".into(), "package.json".into()];
        assert!(f.has_file("Dockerfile"));
        assert!(f.has_any(&["absent", "package.json"]));
        assert_eq!(
            f.first_of(&["absent", "package.json", "Dockerfile"])
                .as_deref(),
            Some("package.json")
        );
    }
}

#[cfg(test)]
mod serde_names {
    use super::*;

    /// Ces valeurs se retrouvent telles quelles dans appspec.toml, relu par des
    /// humains : « nodejs » et « postgresql », pas « node_js » ni « postgre_sql ».
    #[test]
    fn les_noms_serialises_sont_ceux_du_monde_reel() {
        let names = |t: Technology| serde_json::to_string(&t).unwrap();
        assert_eq!(names(Technology::NodeJs), "\"nodejs\"");
        assert_eq!(names(Technology::Php), "\"php\"");

        let db = |d: Database| serde_json::to_string(&d).unwrap();
        assert_eq!(db(Database::PostgreSql), "\"postgresql\"");
        assert_eq!(db(Database::MySql), "\"mysql\"");
        assert_eq!(db(Database::MongoDb), "\"mongodb\"");
    }
}

/// La source retenue pour le packaging, et sa somme de controle.
///
/// Conservee dans les faits pour que le pipeline puisse reprendre a l'etage
/// suivant sans retelecharger — et sans risquer que l'amont ait change entre
/// deux telechargements, ce qui publierait un sha256 ne correspondant a rien.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSelection {
    pub reference: String,
    /// Valeur de `autoupdate.strategy` dans le manifest.
    pub strategy: String,
    /// Valeur de `autoupdate.version_regex`, pour des tags prefixes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_regex: Option<String>,
    pub version: Option<String>,
    /// `release`, `tag` ou `commit`.
    pub kind: String,
    /// Archive des sources.
    pub url: String,
    pub sha256: String,
    /// Binaires deja construits, par architecture.
    ///
    /// Quand l'amont en publie, les paquets YunoHost les preferent aux sources :
    /// `gotify_ynh`, `memos_ynh` et `miniflux_ynh` procedent tous ainsi, avec
    /// `ram.build = 50M`. Compiler sur la machine cible est le principal motif
    /// d'echec d'installation sur les petites instances.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prebuilt: Vec<ArchAsset>,
}

impl SourceSelection {
    /// Vrai si l'on peut se dispenser de construire sur la machine cible.
    pub fn evite_la_compilation(&self) -> bool {
        !self.prebuilt.is_empty()
    }
}

/// Un binaire publie pour une architecture donnee.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchAsset {
    /// Nomenclature `dpkg --print-architecture` : amd64, i386, armhf, arm64.
    pub arch: String,
    pub name: String,
    pub url: String,
    pub sha256: String,
    /// L'empreinte telle que la forge la publie, quand elle le fait. La lire
    /// evite de rapatrier le fichier pour retrouver ce qu'elle a deja dit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empreinte_publiee: Option<String>,
    /// Vrai si l'archive enveloppe tout dans un repertoire racine, faux si
    /// elle est plate, absent si on n'a pas pu le lire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sous_repertoire: Option<bool>,
    /// Taille annoncee, pour pouvoir dire ce qu'on telecharge.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub taille: u64,
    /// Motif reconnaissant cet asset d'une version a l'autre, pour
    /// `autoupdate.asset.<arch>`.
    pub pattern: String,
    /// Faux pour un binaire nu, que `ynh_setup_source` deplace au lieu de
    /// l'extraire. Beaucoup de projets Go publient ainsi : miniflux livre un
    /// `miniflux-linux-amd64` sans extension, et le paquet officiel le declare
    /// avec `extract = false` et `rename`.
    #[serde(default)]
    pub extract: bool,
}

/// Pour ne pas ecrire une taille nulle dans les faits.
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Ce que le code dit de son port d'ecoute, faute de variable d'environnement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcouteFacts {
    /// Options de ligne de commande qui fixent le port ou l'adresse
    /// (`--port`, `--listen`), avec le fichier qui les definit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<(String, String)>,
    /// Ports ecrits en dur dans un appel d'ecoute, avec le fichier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports_en_dur: Vec<(u16, String)>,
}

impl EcouteFacts {
    pub fn est_vide(&self) -> bool {
        self.options.is_empty() && self.ports_en_dur.is_empty()
    }
}
