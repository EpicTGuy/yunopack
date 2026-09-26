//! `AppSpec` : le seul endroit ou une decision de packaging est prise.
//!
//! En amont, `analyze` collecte des faits ; en aval, `generate` rend des
//! templates sans rien decider. Entre les deux, ce fichier TOML concentre les
//! arbitrages — et c'est donc le seul que relise un humain ou un agent.
//!
//! Corollaire volontaire : **un agent n'ecrit jamais de bash**. Il edite
//! `appspec.toml`, relance `generate` puis `verify`. Tout ce qui n'a pas pu
//! etre deduit apparait comme un [`Known::Unresolved`], donc comme une question
//! explicite plutot qu'une invention.

use crate::facts::{Database, Technology};
use crate::known::{Candidate, Known};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const SPEC_SCHEMA_VERSION: u32 = 1;

/// Version de YunoHost visee par defaut. Alignee sur l'instance de test (12.1.26)
/// et sur ce qu'exige `example_ynh` en amont.
pub const DEFAULT_YUNOHOST_MIN: &str = ">= 12.1.17";
/// Jeu de helpers cible. La 2.1 a renomme la plupart des helpers : generer les
/// anciens noms declenche une erreur du linter officiel.
pub const DEFAULT_HELPERS_VERSION: &str = "2.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSpec {
    pub schema_version: u32,
    pub app: AppIdentity,
    pub upstream: Upstream,
    pub integration: Integration,
    pub install: InstallQuestions,
    pub resources: Resources,
    pub runtime: Runtime,
    pub features: Features,
    #[serde(default)]
    pub docs: Docs,
}

impl AppSpec {
    /// Vrai si le paquet doit embarquer un fichier de configuration.
    ///
    /// Trois sources peuvent l'exiger : des variables reconnues, la liaison du
    /// port, celle de la base. N'en tester qu'une laissait le fichier de
    /// cote — c'est ainsi que miniflux s'installait sans pouvoir demarrer.
    pub fn a_une_configuration(&self) -> bool {
        self.runtime.config_file.is_some()
            && (!self.runtime.env.is_empty()
                || self
                    .runtime
                    .port_binding
                    .value()
                    .is_some_and(|v| !v.is_empty())
                || self
                    .runtime
                    .database_binding
                    .value()
                    .is_some_and(|v| !v.is_empty()))
    }

    /// Tous les champs non resolus, sous la forme `(chemin, marqueur FIXME)`.
    ///
    /// C'est ce que `assess` affiche a l'utilisateur et ce que `verify`
    /// transforme en echec de la gate G2.
    pub fn unresolved(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut push = |path: &str, m: Option<String>| {
            if let Some(m) = m {
                out.push((path.to_string(), m));
            }
        };
        push(
            "app.description_en",
            self.app.description_en.fixme("app.description_en"),
        );
        push("app.version", self.app.version.fixme("app.version"));
        push(
            "upstream.license",
            self.upstream.license.fixme("upstream.license"),
        );
        push(
            "resources.sources.url",
            self.resources.sources.url.fixme("resources.sources.url"),
        );
        push(
            "resources.sources.sha256",
            self.resources
                .sources
                .sha256
                .fixme("resources.sources.sha256"),
        );
        push(
            "runtime.technology",
            self.runtime.technology.fixme("runtime.technology"),
        );
        if self.features.systemd {
            push(
                "runtime.execstart",
                self.runtime.execstart.fixme("runtime.execstart"),
            );
        }
        // Une ressource provisionnee dont l'application ignore l'existence ne
        // sert a rien. Constate sur miniflux : le paquet s'installait, toutes
        // les ressources etaient creees, et le service ne demarrait pas faute
        // de savoir joindre sa base.
        // Le port peut aussi etre transmis en ligne de commande plutot que par
        // un fichier de configuration. C'est le cas de yunopack lui-meme, dont
        // le serveur prend `--addr` : exiger alors une ligne de configuration
        // reclamerait un fichier qui n'aurait aucune raison d'exister.
        let port_en_ligne_de_commande = self
            .runtime
            .execstart
            .value()
            .is_some_and(|c| c.contains("__PORT__"));
        if self.resources.ports && !port_en_ligne_de_commande {
            push(
                "runtime.port_binding",
                self.runtime.port_binding.fixme("runtime.port_binding"),
            );
        }
        if self.resources.database.manifest_type().is_some() {
            push(
                "runtime.database_binding",
                self.runtime
                    .database_binding
                    .fixme("runtime.database_binding"),
            );
        }

        out
    }

    pub fn is_complete(&self) -> bool {
        self.unresolved().is_empty()
    }

    /// Les arbitrages qui restent a rendre, sous une forme posable a un humain.
    ///
    /// `unresolved()` rend le marqueur FIXME, fait pour etre depose dans un
    /// fichier. Ici on rend la matiere d'un formulaire : la raison seule, les
    /// endroits ou chercher, et les valeurs proposees.
    pub fn arbitrages(&self) -> Vec<Arbitrage> {
        self.unresolved()
            .into_iter()
            .map(|(chemin, _)| {
                let (raison, ou_chercher, candidats) = self.detail(&chemin);
                let choix = if chemin == "runtime.technology" {
                    Technology::CHOIX
                        .iter()
                        .map(|t| t.nom().to_string())
                        .collect()
                } else {
                    Vec::new()
                };
                Arbitrage {
                    champ: chemin,
                    raison,
                    ou_chercher,
                    candidats,
                    choix,
                    pistes: Vec::new(),
                }
            })
            .collect()
    }

    fn detail(&self, chemin: &str) -> (String, Vec<String>, Vec<Candidate>) {
        macro_rules! decrire {
            ($champ:expr) => {
                match $champ.reason() {
                    Some(u) => (u.unknown.clone(), u.look_in.clone(), u.candidates.clone()),
                    None => (String::new(), Vec::new(), Vec::new()),
                }
            };
        }
        match chemin {
            "app.description_en" => decrire!(self.app.description_en),
            "app.version" => decrire!(self.app.version),
            "upstream.license" => decrire!(self.upstream.license),
            "resources.sources.url" => decrire!(self.resources.sources.url),
            "resources.sources.sha256" => decrire!(self.resources.sources.sha256),
            "runtime.technology" => decrire!(self.runtime.technology),
            "runtime.execstart" => decrire!(self.runtime.execstart),
            "runtime.port_binding" => decrire!(self.runtime.port_binding),
            "runtime.database_binding" => decrire!(self.runtime.database_binding),
            _ => (String::new(), Vec::new(), Vec::new()),
        }
    }

    /// Renseigne un champ non resolu, designe par le chemin que rend
    /// [`AppSpec::unresolved`].
    ///
    /// C'est ce qui permet de repondre sans editer le TOML a la main —
    /// depuis le formulaire web comme depuis la ligne de commande. Un chemin
    /// inconnu est une erreur : accepter en silence une reponse qui ne sera
    /// jamais lue serait le pire des comportements.
    pub fn repondre(&mut self, champ: &str, valeur: &str) -> Result<(), ReponseError> {
        let v = valeur.trim();
        if v.is_empty() {
            return Err(ReponseError::Vide(champ.to_string()));
        }
        match champ {
            "app.description_en" => self.app.description_en = Known::resolved(v.to_string()),
            "app.version" => self.app.version = Known::resolved(v.to_string()),
            "upstream.license" => self.upstream.license = Known::resolved(v.to_string()),
            "resources.sources.url" => self.resources.sources.url = Known::resolved(v.to_string()),
            "resources.sources.sha256" => {
                self.resources.sources.sha256 = Known::resolved(v.to_string())
            }
            "runtime.technology" => {
                let t = Technology::from_nom(v)
                    .ok_or_else(|| ReponseError::ValeurRefusee(champ.to_string(), v.to_string()))?;
                self.runtime.technology = Known::resolved(t);
            }
            "runtime.execstart" => self.runtime.execstart = Known::resolved(v.to_string()),
            "runtime.port_binding" => self.runtime.port_binding = Known::resolved(v.to_string()),
            "runtime.database_binding" => {
                self.runtime.database_binding = Known::resolved(v.to_string())
            }
            _ => return Err(ReponseError::ChampInconnu(champ.to_string())),
        }
        Ok(())
    }
}

/// Un champ a renseigner, sous la forme d'une question posable telle quelle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arbitrage {
    /// Chemin du champ, tel qu'il s'ecrit dans `appspec.toml`.
    pub champ: String,
    pub raison: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ou_chercher: Vec<String>,
    /// Valeurs proposees, avec leur provenance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidats: Vec<Candidate>,
    /// Liste fermee de valeurs acceptees, quand le champ en a une. Vide sinon :
    /// la reponse est alors du texte libre.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choix: Vec<String>,
    /// Fichiers du depot ou la reponse a des chances de se trouver, avec leur
    /// adresse. Dire « chercher dans la documentation » sans dire ou oblige a
    /// aller fouiller soi-meme.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pistes: Vec<Piste>,
}

/// Un fichier du depot a consulter pour repondre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Piste {
    /// Chemin dans le depot, ex. `docs/configuration.md`.
    pub chemin: String,
    /// Adresse ou le lire, quand on connait le depot et la reference.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReponseError {
    #[error("« {0} » n'est pas un champ de l'appspec")]
    ChampInconnu(String),
    #[error("une reponse vide ne renseigne rien : {0}")]
    Vide(String),
    #[error("« {1} » n'est pas une valeur acceptee pour {0}")]
    ValeurRefusee(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIdentity {
    /// Minuscules, chiffres et tirets. Sert aussi de nom d'utilisateur systeme,
    /// de nom de dossier et de prefixe de conf nginx.
    pub id: String,
    /// Nom affiche. Le linter officiel refuse au-dela de 23 caracteres.
    pub name: String,
    /// 150 caracteres maximum, affichee dans le catalogue.
    pub description_en: Known<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_fr: Option<String>,
    /// Version amont seule : le suffixe `~ynhN` est ajoute a la generation.
    pub version: Known<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maintainers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    /// Identifiant SPDX. Seul champ obligatoire de la section cote YunoHost.
    pub license: Known<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admindoc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub userdoc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Le defaut d'un champ textuel est *non resolu*, jamais la chaine vide.
/// Oublier de renseigner un champ produit ainsi un FIXME visible plutot qu'un
/// paquet silencieusement incomplet.
impl Default for Known<String> {
    fn default() -> Self {
        Known::unresolved("non renseigne", &[])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integration {
    pub yunohost_min: String,
    pub helpers_version: String,
    /// `"all"` ou une liste en nomenclature `dpkg --print-architecture`.
    pub architectures: Architectures,
    pub multi_instance: bool,
    pub ldap: Triple,
    pub sso: Triple,
    pub disk: String,
    pub ram_build: String,
    pub ram_runtime: String,
}

impl Default for Integration {
    fn default() -> Self {
        Self {
            yunohost_min: DEFAULT_YUNOHOST_MIN.to_string(),
            helpers_version: DEFAULT_HELPERS_VERSION.to_string(),
            architectures: Architectures::All,
            // Les paquets officiels l'autorisent par defaut ; rien ne justifie
            // d'interdire une seconde installation sans raison.
            multi_instance: true,
            ldap: Triple::NotRelevant,
            sso: Triple::NotRelevant,
            disk: "50M".into(),
            ram_build: "50M".into(),
            ram_runtime: "50M".into(),
        }
    }
}

/// `architectures = "all"` ou une liste. Les deux formes sont valides dans le
/// manifest, on les represente sans forcer l'une ou l'autre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Architectures {
    All,
    Only(Vec<String>),
}

impl serde::Serialize for Architectures {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Architectures::All => s.serialize_str("all"),
            Architectures::Only(v) => v.serialize(s),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Architectures {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::One(s) if s == "all" => Architectures::All,
            Raw::One(s) => Architectures::Only(vec![s]),
            Raw::Many(v) => Architectures::Only(v),
        })
    }
}

/// `true` / `false` / `"not_relevant"`, comme l'exige le manifest pour `ldap` et `sso`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Triple {
    Yes,
    No,
    /// L'app n'a pas de notion de compte utilisateur.
    NotRelevant,
}

impl Triple {
    pub fn manifest_value(self) -> &'static str {
        match self {
            Triple::Yes => "true",
            Triple::No => "false",
            Triple::NotRelevant => "\"not_relevant\"",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallQuestions {
    pub url_scheme: UrlScheme,
    /// Groupe autorise a l'installation : `visitors`, `all_users`, ...
    pub init_main_permission: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<Question>,
}

impl Default for InstallQuestions {
    fn default() -> Self {
        Self {
            url_scheme: UrlScheme::DomainAndPath,
            init_main_permission: "visitors".into(),
            extra: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlScheme {
    /// `domain.tld/app` : question `domain` + question `path`.
    DomainAndPath,
    /// L'app exige un domaine dedie : question `domain` seule.
    FullDomain,
    /// Pas de composante web (daemon pur).
    NoUrl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub name: String,
    /// Parmi : string, text, select, boolean, password, email, url, number, user, group, ...
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_en: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    pub sources: Sources,
    pub system_user: bool,
    pub install_dir: bool,
    pub data_dir: bool,
    /// Chemin expose par la permission principale, typiquement `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_permission_url: Option<String>,
    /// Reserve un port pour le reverse-proxy interne nginx -> app.
    pub ports: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apt_packages: Vec<String>,
    pub database: Database,
    /// Versions de runtime a provisionner par le coeur (`[resources.nodejs]`...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodejs_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruby_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sources {
    pub url: Known<String>,
    pub sha256: Known<String>,
    /// `latest_github_release`, `latest_github_tag`, `latest_github_commit`...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoupdate_strategy: Option<String>,
    /// `false` quand l'archive n'a pas de repertoire intermediaire.
    #[serde(default = "default_true")]
    pub in_subdir: bool,
    /// Binaires publies par l'amont, un par architecture.
    ///
    /// Quand cette liste n'est pas vide, c'est elle qui fait foi : le manifest
    /// declare `amd64.url`, `arm64.url`... et `url` n'est plus qu'un repli
    /// documentaire. C'est ainsi que procedent les paquets officiels, pour
    /// eviter de compiler sur la machine cible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_arch: Vec<crate::facts::ArchAsset>,
    /// `false` pour un binaire nu, que `ynh_setup_source` deplace au lieu de
    /// l'extraire.
    #[serde(default = "default_true")]
    pub extract: bool,
    /// Nom sous lequel deposer un fichier non extrait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
}

impl Sources {
    /// Vrai si le paquet installera un binaire deja construit.
    pub fn utilise_des_binaires(&self) -> bool {
        !self.per_arch.is_empty()
    }
}

fn default_true() -> bool {
    true
}

/// Les decisions qui n'ont pas d'equivalent direct dans le manifest et qui
/// pilotent le contenu des scripts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runtime {
    pub technology: Known<Technology>,
    /// Commandes de build, dans l'ordre, executees dans `$install_dir`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_steps: Vec<String>,
    /// Valeur de `ExecStart=` de l'unite systemd.
    pub execstart: Known<String>,
    /// Variable par laquelle l'app apprend son port. Cable sur `$port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_env_var: Option<String>,
    /// Fichier de conf a generer dans `$install_dir`, avec ses substitutions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_file: Option<String>,
    /// Ligne de configuration par laquelle l'application apprend son port.
    ///
    /// Obligatoire des qu'un port est reserve : sans elle, l'application
    /// ecoute ou bon lui semble et le reverse-proxy ne la trouve pas. La forme
    /// varie d'une application a l'autre — `PORT=__PORT__` ou
    /// `LISTEN_ADDR=127.0.0.1:__PORT__` — et ne se devine pas.
    #[serde(default)]
    pub port_binding: Known<String>,
    /// Ligne par laquelle l'application apprend comment joindre sa base.
    ///
    /// Obligatoire des qu'une base est provisionnee. Sans elle, le paquet
    /// s'installe mais l'application ne demarre pas — constate sur miniflux,
    /// qui retombait sur `postgres://postgres@localhost` et echouait.
    #[serde(default)]
    pub database_binding: Known<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
}

impl Default for Known<Technology> {
    fn default() -> Self {
        Known::unresolved("stack non identifiee", &["Dockerfile", "README.md"])
    }
}

/// Briques d'integration a cabler. Chacune conditionne des blocs dans plusieurs
/// scripts a la fois : activer `systemd` ajoute du code dans install, remove,
/// upgrade, backup, restore et change_url.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features {
    pub nginx: bool,
    pub systemd: bool,
    pub phpfpm: bool,
    pub logrotate: bool,
    pub fail2ban: bool,
    pub cron: bool,
    pub change_url: bool,
    /// Declare le service aupres de YunoHost (`yunohost service add`).
    pub service_integration: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Docs {
    /// Corps de `doc/DESCRIPTION.md`. Repli deterministe : description de la
    /// forge, puis premier paragraphe du README amont.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_install: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_install: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<String>,
    /// Texte de la licence amont, recopie tel quel dans le fichier LICENSE.
    ///
    /// Le linter officiel exige ce fichier. Le recopier depuis l'amont est la
    /// seule option honnete : resumer ou reecrire une licence n'a pas de sens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> AppSpec {
        AppSpec {
            schema_version: SPEC_SCHEMA_VERSION,
            app: AppIdentity {
                id: "foo".into(),
                name: "Foo".into(),
                description_en: Known::resolved("A thing".into()),
                description_fr: None,
                version: Known::resolved("1.2.3".into()),
                maintainers: vec![],
            },
            upstream: Upstream {
                license: Known::resolved("AGPL-3.0".into()),
                ..Default::default()
            },
            integration: Integration::default(),
            install: InstallQuestions::default(),
            resources: Resources {
                sources: Sources {
                    url: Known::resolved("https://x/v1.tar.gz".into()),
                    sha256: Known::resolved("ab".repeat(32)),
                    autoupdate_strategy: None,
                    in_subdir: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime: Runtime {
                technology: Known::resolved(Technology::NodeJs),
                execstart: Known::resolved("/var/www/foo/bin/s".into()),
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
    fn une_spec_complete_ne_laisse_aucun_fixme() {
        let s = minimal();
        assert!(s.is_complete(), "{:?}", s.unresolved());
    }

    #[test]
    fn un_champ_non_resolu_remonte_avec_son_chemin() {
        let mut s = minimal();
        s.runtime.execstart = Known::unresolved("aucun CMD", &["Procfile"]);
        let u = s.unresolved();
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].0, "runtime.execstart");
        assert!(u[0].1.contains("FIXME(yunopack)"));
    }

    #[test]
    fn execstart_n_est_exige_que_si_l_app_a_un_service_systemd() {
        let mut s = minimal();
        s.runtime.execstart = Known::unresolved("aucun CMD", &[]);
        s.features.systemd = false;
        assert!(
            s.is_complete(),
            "une app sans daemon n'a pas besoin d'ExecStart"
        );
    }

    #[test]
    fn la_spec_fait_un_aller_retour_toml_sans_perte() {
        let s = minimal();
        let text = toml::to_string_pretty(&s).unwrap();
        let back: AppSpec = toml::from_str(&text).unwrap();
        assert_eq!(s, back);
    }
}

/// L'exemple documentaire doit rester valide : s'il ne se relit plus, c'est que
/// le format a bouge sans que la doc suive.
#[cfg(test)]
mod exemple_documente {
    use super::*;

    #[test]
    fn l_exemple_de_la_doc_se_relit_et_est_complet() {
        let src = include_str!("../../../tests/fixtures/appspec.example.toml");
        let spec: AppSpec = toml::from_str(src).expect("appspec.example.toml doit rester valide");

        assert_eq!(spec.app.id, "grist");
        assert_eq!(spec.runtime.technology.value(), Some(&Technology::NodeJs));
        assert_eq!(spec.resources.database, Database::PostgreSql);
        assert!(
            spec.is_complete(),
            "l'exemple ne doit contenir aucun FIXME : {:?}",
            spec.unresolved()
        );
    }
}

#[cfg(test)]
mod port_en_ligne_de_commande {
    use super::*;
    use crate::facts::Technology;

    fn spec_avec(execstart: &str) -> AppSpec {
        AppSpec {
            schema_version: SPEC_SCHEMA_VERSION,
            app: AppIdentity {
                id: "demo".into(),
                name: "Demo".into(),
                description_en: Known::resolved("x".into()),
                description_fr: None,
                version: Known::resolved("1.0".into()),
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
                sources: Sources {
                    url: Known::resolved("u".into()),
                    sha256: Known::resolved("s".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime: Runtime {
                technology: Known::resolved(Technology::Rust),
                execstart: Known::resolved(execstart.into()),
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
    fn un_port_passe_en_argument_dispense_de_ligne_de_configuration() {
        // Cas de yunopack lui-meme : son serveur prend `--addr`. Reclamer une
        // ligne de configuration exigerait un fichier sans raison d'etre.
        let s = spec_avec("__INSTALL_DIR__/serveur --addr 127.0.0.1:__PORT__");
        assert!(s.is_complete(), "champs manquants : {:?}", s.unresolved());
    }

    #[test]
    fn sans_port_dans_la_commande_la_liaison_reste_exigee() {
        let s = spec_avec("__INSTALL_DIR__/serveur");
        let manquants: Vec<String> = s.unresolved().into_iter().map(|(c, _)| c).collect();
        assert!(manquants.contains(&"runtime.port_binding".to_string()));
    }
}

#[cfg(test)]
mod reponses {
    use super::*;

    fn spec() -> AppSpec {
        let src = include_str!("../../../tests/fixtures/appspec.example.toml");
        toml::from_str(src).expect("appspec.example.toml doit rester valide")
    }

    #[test]
    fn repondre_resout_le_champ_designe() {
        let mut s = spec();
        s.runtime.port_binding = Known::unresolved("inconnu", &[]);
        assert!(!s.is_complete());

        s.repondre("runtime.port_binding", "PORT=__PORT__").unwrap();
        assert_eq!(
            s.runtime.port_binding.value().map(String::as_str),
            Some("PORT=__PORT__")
        );
        assert!(s.is_complete());
    }

    #[test]
    fn les_espaces_autour_de_la_reponse_sont_retires() {
        let mut s = spec();
        s.repondre("runtime.execstart", "  /usr/bin/foo  ").unwrap();
        assert_eq!(
            s.runtime.execstart.value().map(String::as_str),
            Some("/usr/bin/foo")
        );
    }

    #[test]
    fn un_champ_inconnu_est_refuse_plutot_qu_ignore() {
        // Accepter en silence une reponse qui ne sera jamais lue laisserait
        // croire que le champ est renseigne.
        let mut s = spec();
        assert!(matches!(
            s.repondre("runtime.inexistant", "x"),
            Err(ReponseError::ChampInconnu(_))
        ));
    }

    #[test]
    fn une_reponse_vide_est_refusee() {
        let mut s = spec();
        assert!(matches!(
            s.repondre("runtime.execstart", "   "),
            Err(ReponseError::Vide(_))
        ));
    }

    #[test]
    fn la_technologie_se_repond_sous_ses_appellations_courantes() {
        let mut s = spec();
        s.repondre("runtime.technology", "Node.js").unwrap();
        assert_eq!(s.runtime.technology.value(), Some(&Technology::NodeJs));
        s.repondre("runtime.technology", "golang").unwrap();
        assert_eq!(s.runtime.technology.value(), Some(&Technology::Go));
    }

    #[test]
    fn une_technologie_inconnue_est_refusee() {
        let mut s = spec();
        assert!(matches!(
            s.repondre("runtime.technology", "cobol"),
            Err(ReponseError::ValeurRefusee(_, _))
        ));
    }

    #[test]
    fn un_arbitrage_porte_sa_raison_et_ses_propositions() {
        let mut s = spec();
        s.runtime.port_binding = Known::unresolved_avec(
            "nom de variable inconnu",
            &["README.md"],
            vec![Candidate::new("PORT=__PORT__", "forme la plus repandue")],
        );

        let a = s.arbitrages();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].champ, "runtime.port_binding");
        assert_eq!(a[0].raison, "nom de variable inconnu");
        assert_eq!(a[0].ou_chercher, vec!["README.md"]);
        assert_eq!(a[0].candidats[0].value, "PORT=__PORT__");
        // Champ libre : aucune liste fermee.
        assert!(a[0].choix.is_empty());
    }

    #[test]
    fn la_technologie_propose_une_liste_fermee() {
        let mut s = spec();
        s.runtime.technology = Known::unresolved("aucune stack reconnue", &[]);
        let a = s.arbitrages();
        let tech = a.iter().find(|a| a.champ == "runtime.technology").unwrap();
        assert!(tech.choix.contains(&"nodejs".to_string()));
        assert!(!tech.choix.contains(&"unknown".to_string()));
    }

    #[test]
    fn une_specification_complete_ne_pose_aucune_question() {
        assert!(spec().arbitrages().is_empty());
    }
}
