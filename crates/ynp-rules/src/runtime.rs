//! Regles sur ce que l'application exige pour tourner.
//!
//! Le point commun de ces regles : elles confrontent ce que l'amont demande a
//! ce que YunoHost sait fournir. La plupart des refus du projet viennent d'ici.

use crate::Rule;
use ynp_analyze::knowledge;
use ynp_core::facts::{RepoFacts, Technology};
use ynp_core::finding::{Evidence, Finding, Severity};

// --- Bloquants ---

pub struct ConteneurRequis;

impl Rule for ConteneurRequis {
    fn id(&self) -> &'static str {
        "RUN001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        if !facts.services.requires_container_runtime {
            return None;
        }
        Some(
            Finding::new(
                self.id(),
                Severity::Blocker,
                "Distribuee uniquement sous forme d'image",
            )
            .detail(
                "Une application YunoHost tourne en natif sur l'hote. Ce depot ne fournit \
                     aucun chemin d'installation native : ni Dockerfile a transposer, ni \
                     fichiers de projet a construire.",
            )
            .remediation(
                "Chercher si le projet documente une installation sans conteneur. A defaut, \
                     l'application sort du modele YunoHost.",
            ),
        )
    }
}

pub struct KubernetesSeulement;

impl Rule for KubernetesSeulement {
    fn id(&self) -> &'static str {
        "K8S001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        let a_un_chart = facts.tree.iter().any(|p| p.ends_with("Chart.yaml"));
        if !a_un_chart || facts.build.is_some() || facts.stack.primary != Technology::Unknown {
            return None;
        }
        Some(
            Finding::new(
                self.id(),
                Severity::Blocker,
                "Deploiement uniquement Kubernetes",
            )
            .detail(
                "Le depot ne propose qu'un chart Helm. Il n'y a pas de chemin \
                     d'installation sur une machine unique.",
            )
            .remediation("Aucune dans le cadre du catalogue YunoHost."),
        )
    }
}

pub struct BaseNonSupportee;

impl Rule for BaseNonSupportee {
    fn id(&self) -> &'static str {
        "DB002"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        if facts.services.unsupported.is_empty() {
            return None;
        }
        let liste = facts.services.unsupported.join(", ");

        // Un compose sans service applicatif decrit un environnement de
        // developpement, pas un deploiement. Ses services accompagnent le
        // travail de l'equipe sans etre forcement exiges par l'application.
        // Constate sur block/buzz, dont le compose ne lance que postgres,
        // redis, keycloak, minio et prometheus — l'application tourne a cote.
        let compose_de_dev = facts
            .compose
            .as_ref()
            .is_some_and(|c| !c.services.is_empty() && c.services.iter().all(|s| !s.is_app));

        if compose_de_dev {
            return Some(
                Finding::new(
                    self.id(),
                    Severity::Major,
                    format!("Service sans equivalent YunoHost, en environnement de dev : {liste}"),
                )
                .arg("services", &liste)
                .detail(
                    "Ce service apparait dans un compose qui ne lance aucun service applicatif : \
                     c'est un environnement de developpement, pas un deploiement. Rien ne dit \
                     que l'application en a besoin pour tourner.",
                )
                .remediation(
                    "Lire la documentation d'installation du projet pour savoir si ce service \
                     est reellement exige. S'il l'est, le refus devient definitif.",
                ),
            );
        }

        Some(
            Finding::new(
                self.id(),
                Severity::Blocker,
                format!(
                    "Service sans equivalent YunoHost : {}",
                    facts.services.unsupported.join(", ")
                ),
            )
            .arg("services", facts.services.unsupported.join(", "))
            .detail(
                "`[resources.database]` ne provisionne que MySQL et PostgreSQL ; MongoDB et \
                 Redis passent par des helpers. Les autres services demanderaient d'empaqueter \
                 un serveur a part entiere.",
            )
            .remediation(
                "Verifier si l'application peut fonctionner sans ce service, ce que certaines \
                 permettent au prix d'une fonctionnalite. Sinon, hors perimetre.",
            ),
        )
    }
}

// --- Majeurs ---

pub struct PythonHorsBookworm;

impl Rule for PythonHorsBookworm {
    fn id(&self) -> &'static str {
        "PY001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        if facts.stack.primary != Technology::Python {
            return None;
        }
        let k = knowledge::get();
        let fournie = k.bookworm_version("python")?;
        let exigee = facts.stack.runtime_version.as_deref()?;

        // Comparaison sur majeure.mineure : `3.11.5` et `3.11` sont la meme chose.
        if exigee.starts_with(fournie) {
            return None;
        }

        Some(
            Finding::new(
                self.id(),
                Severity::Major,
                format!("Python {exigee} exige, bookworm fournit {fournie}"),
            )
            .arg("exigee", exigee)
            .arg("fournie", fournie)
            .detail(
                "Contrairement a nodejs, ruby, go et composer, Python n'a pas de `[resources]` \
                 cote YunoHost. Une version differente de celle de Debian impose un venv ou un \
                 pyenv ecrit a la main dans les scripts.",
            )
            .remediation(format!(
                "Verifier si l'application tourne reellement sur Python {fournie} — c'est \
                 souvent le cas malgre la contrainte declaree. Sinon, prevoir du travail manuel."
            ))
            .evidence(Evidence::file("pyproject.toml")),
        )
    }
}

pub struct BuildGourmand;

impl Rule for BuildGourmand {
    fn id(&self) -> &'static str {
        "BUILD001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        // Si l'amont publie des binaires deja construits, on ne compilera rien
        // sur la machine cible : l'avertissement n'a plus lieu d'etre.
        if facts
            .selection
            .as_ref()
            .is_some_and(|s| s.evite_la_compilation())
        {
            return None;
        }
        let build = facts.build.as_ref()?;
        // Un build de frontal se reconnait a ses gestionnaires de paquets
        // JavaScript combines a une etape de construction.
        let etapes_js = build
            .build_steps
            .iter()
            .filter(|s| {
                ["npm", "yarn", "pnpm", "bun"]
                    .iter()
                    .any(|m| s.starts_with(m))
            })
            .count();
        let construit = build.build_steps.iter().any(|s| s.contains("build"));

        if etapes_js == 0 || !construit {
            return None;
        }

        Some(
            Finding::new(self.id(), Severity::Major, "Build de frontal JavaScript")
                .detail(
                    "Un build de frontal depasse frequemment 1,5 Go de memoire. Beaucoup \
                     d'instances YunoHost tournent sur ARM avec 1 a 2 Go : le build echouera \
                     chez une partie des utilisateurs.",
                )
                .remediation(
                    "Chercher une release contenant les assets deja construits, ce qui evite \
                     le build sur la machine cible. A defaut, renseigner `ram.build` honnetement.",
                )
                .evidence(Evidence::file(build.dockerfile_path.clone())),
        )
    }
}

pub struct RecetteIntrouvable;

impl Rule for RecetteIntrouvable {
    fn id(&self) -> &'static str {
        "BUILD002"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        let build = facts.build.as_ref()?;

        // L'application a manifestement besoin d'etre construite...
        let a_besoin_d_un_build = facts.stack.build_script.is_some()
            || matches!(
                facts.stack.primary,
                Technology::NodeJs | Technology::Rust | Technology::Java
            );
        if !a_besoin_d_un_build {
            return None;
        }

        let motif = if build.build_steps.is_empty() {
            "le Dockerfile retenu n'execute aucune etape de construction"
        } else if tous_les_dockerfiles_sont_hors_produit(&facts.tree) {
            // Cas d'AFFiNE : ses trois Dockerfiles vivent dans .github et
            // .render. Ils assemblent ce que la chaine d'integration a bati
            // ailleurs ; aucun ne dit comment construire depuis les sources.
            "tous les Dockerfiles du depot appartiennent a la chaine \
             d'integration continue, aucun ne construit depuis les sources"
        } else {
            return None;
        };

        Some(
            Finding::new(
                self.id(),
                Severity::Blocker,
                "Recette de construction introuvable",
            )
            .detail(format!(
                "Le packager ne sait pas produire l'application a partir des sources, car \
                     {motif} — et il ne l'inventera pas. C'est typiquement le cas d'un projet \
                     dont le seul chemin d'auto-hebergement documente est une image publiee."
            ))
            .remediation(
                "Chercher la recette ailleurs : instructions de construction du projet, \
                     definition de la chaine d'integration, ou Dockerfile de developpement. \
                     Renseigner ensuite `runtime.build_steps` a la main dans appspec.toml. Si \
                     l'amont ne publie aucune procedure native, l'application sort du modele \
                     YunoHost.",
            )
            .evidence(Evidence::file(build.dockerfile_path.clone())),
        )
    }
}

/// Vrai si le depot ne contient aucun Dockerfile hors des repertoires
/// d'integration continue, d'hebergeur ou de poste de developpement.
fn tous_les_dockerfiles_sont_hors_produit(tree: &[String]) -> bool {
    const HORS_PRODUIT: &[&str] = &[
        ".github",
        ".render",
        ".devcontainer",
        ".gitlab",
        ".circleci",
        ".woodpecker",
    ];

    let dockerfiles: Vec<&String> = tree
        .iter()
        .filter(|p| {
            p.rsplit('/')
                .next()
                .unwrap_or(p)
                .to_lowercase()
                .starts_with("dockerfile")
        })
        .collect();

    !dockerfiles.is_empty()
        && dockerfiles.iter().all(|p| {
            let lower = p.to_lowercase();
            HORS_PRODUIT.iter().any(|k| lower.contains(k))
        })
}

pub struct PortPrivilegie;

impl Rule for PortPrivilegie {
    fn id(&self) -> &'static str {
        "PORT001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        let build = facts.build.as_ref()?;
        let privilegies: Vec<u16> = build
            .expose
            .iter()
            .copied()
            .filter(|p| *p == 80 || *p == 443)
            .collect();

        // Un port privilegie accompagne d'un autre port est presque toujours le
        // fait d'un frontal interne, pas une exigence : `EXPOSE 80 3000`.
        if privilegies.is_empty() || build.expose.len() > privilegies.len() {
            return None;
        }

        Some(
            Finding::new(
                self.id(),
                Severity::Major,
                format!("Port privilegie expose : {privilegies:?}"),
            )
            .arg("ports", format!("{privilegies:?}"))
            .detail(
                "Les ports 80 et 443 sont occupes par le nginx de YunoHost, qui assure le \
                 reverse-proxy vers les applications.",
            )
            .remediation(
                "Verifier si le port est configurable, typiquement par une variable \
                 d'environnement. Si l'application exige le port 80, elle est incompatible.",
            )
            .evidence(Evidence::file(build.dockerfile_path.clone())),
        )
    }
}

pub struct StackInconnue;

impl Rule for StackInconnue {
    fn id(&self) -> &'static str {
        "STACK001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        if facts.stack.primary != Technology::Unknown {
            return None;
        }
        Some(
            Finding::new(self.id(), Severity::Major, "Technologie non identifiee")
                .detail(
                    "Ni Dockerfile exploitable, ni fichier de projet reconnu. Le packager ne \
                     peut pas deduire comment l'application se construit et se lance — et il \
                     ne le devinera pas.",
                )
                .remediation(
                    "Renseigner `runtime.technology`, `runtime.build_steps` et \
                     `runtime.execstart` a la main dans appspec.toml, en s'appuyant sur les \
                     instructions d'installation du projet.",
                ),
        )
    }
}

// --- Informatifs : pas des defauts, des renseignements pour l'appspec ---

pub struct BaseDetectee;

impl Rule for BaseDetectee {
    fn id(&self) -> &'static str {
        "DB001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        let db = facts.services.database;
        let manifest = db.manifest_type()?;
        Some(
            Finding::new(
                self.id(),
                Severity::Info,
                format!("Base {manifest} provisionnable"),
            )
            .arg("base", manifest)
            .detail(format!(
                "Sera declaree en `[resources.database] type = \"{manifest}\"` ; le coeur \
                     de YunoHost fournira $db_name, $db_user et $db_pwd."
            ))
            .evidence(Evidence {
                file: "(analyse)".into(),
                line: None,
                excerpt: facts.services.database_evidence.clone(),
            }),
        )
    }
}

pub struct PaquetsAlpineATraduire;

impl Rule for PaquetsAlpineATraduire {
    fn id(&self) -> &'static str {
        "APK001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        let build = facts.build.as_ref()?;
        if build.apk_packages.is_empty() {
            return None;
        }
        let k = knowledge::get();
        let inconnus: Vec<&String> = build
            .apk_packages
            .iter()
            .filter(|p| k.apk_to_deb(p).is_none())
            .collect();

        if inconnus.is_empty() {
            return Some(
                Finding::new(self.id(), Severity::Info, "Dependances Alpine traduites").detail(
                    format!(
                        "Les {} paquets apk ont tous un equivalent Debian connu.",
                        build.apk_packages.len()
                    ),
                ),
            );
        }

        Some(
            Finding::new(
                self.id(),
                Severity::Minor,
                format!("{} paquet(s) Alpine sans equivalent connu", inconnus.len()),
            )
            .arg("nombre", inconnus.len().to_string())
            .detail(format!(
                "Non traduits : {}. Les laisser tels quels ferait echouer l'installation apt.",
                inconnus
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .remediation(
                "Chercher l'equivalent Debian et l'ajouter a assets/knowledge/apk-to-deb.toml, \
                 pour que le cas suivant soit traite automatiquement.",
            ),
        )
    }
}

pub struct ModulesNatifs;

impl Rule for ModulesNatifs {
    fn id(&self) -> &'static str {
        "NPM001"
    }

    fn check(&self, facts: &RepoFacts) -> Option<Finding> {
        if facts.stack.native_deps.is_empty() {
            return None;
        }
        let k = knowledge::get();
        let mut paquets: Vec<String> = facts
            .stack
            .native_deps
            .iter()
            .flat_map(|m| k.npm_native_deps(m).to_vec())
            .collect();
        paquets.sort_unstable();
        paquets.dedup();

        Some(
            Finding::new(
                self.id(),
                Severity::Info,
                "Modules npm a compilation native",
            )
            .detail(format!(
                "{} exige(nt) des paquets Debian absents du package.json : {}. \
                     C'est la cause la plus frequente d'un `npm ci` en echec.",
                facts.stack.native_deps.join(", "),
                paquets.join(", ")
            ))
            .evidence(Evidence::file("package.json")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::depot_sain;
    use ynp_core::facts::{BuildRecipe, Database, ServiceFacts, StackFacts};

    #[test]
    fn un_depot_sain_ne_declenche_aucune_regle_de_runtime() {
        let f = depot_sain();
        for r in [
            &ConteneurRequis as &dyn Rule,
            &KubernetesSeulement,
            &BaseNonSupportee,
            &PythonHorsBookworm,
            &PortPrivilegie,
            &StackInconnue,
        ] {
            assert!(
                r.check(&f).is_none(),
                "{} n'aurait pas du se declencher",
                r.id()
            );
        }
    }

    #[test]
    fn une_application_distribuee_en_image_seule_est_refusee() {
        let mut f = depot_sain();
        f.services.requires_container_runtime = true;
        assert_eq!(
            ConteneurRequis.check(&f).unwrap().severity,
            Severity::Blocker
        );
    }

    #[test]
    fn la_presence_d_un_dockerfile_n_est_jamais_un_motif_de_refus() {
        // C'est notre meilleure source d'information, pas un probleme.
        let f = depot_sain();
        assert!(f.build.is_some());
        assert!(ConteneurRequis.check(&f).is_none());
    }

    #[test]
    fn un_service_sans_equivalent_bloque() {
        let mut f = depot_sain();
        f.services = ServiceFacts {
            unsupported: vec!["meilisearch (getmeili/meilisearch)".into()],
            ..Default::default()
        };
        let finding = BaseNonSupportee.check(&f).unwrap();
        assert_eq!(finding.severity, Severity::Blocker);
        assert!(finding.title.contains("meilisearch"));
    }

    #[test]
    fn python_dans_la_version_de_bookworm_ne_declenche_rien() {
        let mut f = depot_sain();
        f.stack = StackFacts {
            primary: Technology::Python,
            runtime_version: Some("3.11.5".into()),
            ..Default::default()
        };
        assert!(
            PythonHorsBookworm.check(&f).is_none(),
            "3.11.5 est bien le Python de bookworm"
        );
    }

    #[test]
    fn python_hors_bookworm_est_signale_avec_sa_raison() {
        let mut f = depot_sain();
        f.stack = StackFacts {
            primary: Technology::Python,
            runtime_version: Some("3.13".into()),
            ..Default::default()
        };
        let finding = PythonHorsBookworm.check(&f).unwrap();

        assert_eq!(finding.severity, Severity::Major);
        assert!(finding.detail.contains("pas de `[resources]`"));
    }

    #[test]
    fn un_build_de_frontal_est_signale_pour_sa_consommation_memoire() {
        let mut f = depot_sain();
        f.build = Some(BuildRecipe {
            dockerfile_path: "Dockerfile".into(),
            build_steps: vec!["npm ci".into(), "npm run build".into()],
            ..Default::default()
        });
        assert_eq!(BuildGourmand.check(&f).unwrap().severity, Severity::Major);
    }

    #[test]
    fn une_simple_installation_de_dependances_n_est_pas_un_build_gourmand() {
        let mut f = depot_sain();
        f.build = Some(BuildRecipe {
            dockerfile_path: "Dockerfile".into(),
            build_steps: vec!["npm ci --omit=dev".into()],
            ..Default::default()
        });
        assert!(BuildGourmand.check(&f).is_none());
    }

    #[test]
    fn un_port_privilegie_accompagne_d_un_autre_port_n_est_pas_une_exigence() {
        // `EXPOSE 80 3000` designe un frontal interne, pas une contrainte.
        let mut f = depot_sain();
        f.build = Some(BuildRecipe {
            expose: vec![80, 3000],
            ..Default::default()
        });
        assert!(PortPrivilegie.check(&f).is_none());

        f.build = Some(BuildRecipe {
            expose: vec![80],
            ..Default::default()
        });
        assert!(PortPrivilegie.check(&f).is_some());
    }

    #[test]
    fn une_stack_non_identifiee_renvoie_vers_l_appspec() {
        let mut f = depot_sain();
        f.stack = StackFacts::default();
        let finding = StackInconnue.check(&f).unwrap();

        assert!(finding.detail.contains("ne le devinera pas"));
        assert!(finding
            .remediation
            .as_deref()
            .unwrap()
            .contains("appspec.toml"));
    }

    #[test]
    fn une_base_provisionnable_est_un_renseignement_pas_un_defaut() {
        let mut f = depot_sain();
        f.services.database = Database::PostgreSql;
        let finding = BaseDetectee.check(&f).unwrap();

        assert_eq!(finding.severity, Severity::Info);
        assert!(finding.detail.contains("postgresql"));
    }

    #[test]
    fn un_paquet_alpine_inconnu_est_signale_avec_ou_l_ajouter() {
        let mut f = depot_sain();
        f.build = Some(BuildRecipe {
            apk_packages: vec!["build-base".into(), "paquet-jamais-vu".into()],
            ..Default::default()
        });
        let finding = PaquetsAlpineATraduire.check(&f).unwrap();

        assert_eq!(finding.severity, Severity::Minor);
        assert!(finding.detail.contains("paquet-jamais-vu"));
        assert!(finding
            .remediation
            .as_deref()
            .unwrap()
            .contains("apk-to-deb.toml"));
    }

    #[test]
    fn des_paquets_alpine_tous_traduits_ne_sont_qu_une_information() {
        let mut f = depot_sain();
        f.build = Some(BuildRecipe {
            apk_packages: vec!["build-base".into(), "vips-dev".into()],
            ..Default::default()
        });
        assert_eq!(
            PaquetsAlpineATraduire.check(&f).unwrap().severity,
            Severity::Info
        );
    }

    #[test]
    fn les_dependances_cachees_d_un_module_natif_sont_explicitees() {
        let mut f = depot_sain();
        f.stack.native_deps = vec!["sharp".into()];
        let finding = ModulesNatifs.check(&f).unwrap();

        assert_eq!(finding.severity, Severity::Info);
        assert!(finding.detail.contains("libvips-dev"));
    }
}

#[cfg(test)]
mod compose_de_developpement {
    use super::*;
    use crate::tests::depot_sain;
    use ynp_core::facts::{ComposeFacts, ComposeService};

    fn compose(services: &[(&str, bool)]) -> ComposeFacts {
        ComposeFacts {
            path: "docker-compose.yml".into(),
            services: services
                .iter()
                .map(|(n, is_app)| ComposeService {
                    name: (*n).into(),
                    is_app: *is_app,
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn un_service_non_supporte_dans_un_deploiement_bloque() {
        let mut f = depot_sain();
        f.services.unsupported = vec!["meilisearch".into()];
        f.compose = Some(compose(&[("app", true), ("meilisearch", false)]));

        assert_eq!(
            BaseNonSupportee.check(&f).unwrap().severity,
            Severity::Blocker
        );
    }

    #[test]
    fn le_meme_service_dans_un_compose_de_dev_degrade_sans_refuser() {
        // Cas reel de block/buzz : le compose ne lance que des dependances de
        // developpement, l'application tourne a cote. Rien ne dit qu'elle exige
        // minio pour fonctionner.
        let mut f = depot_sain();
        f.services.unsupported = vec!["minio (minio/minio:latest)".into()];
        f.compose = Some(compose(&[
            ("postgres", false),
            ("minio", false),
            ("keycloak", false),
        ]));

        let finding = BaseNonSupportee.check(&f).unwrap();
        assert_eq!(finding.severity, Severity::Major);
        assert!(finding.title.contains("environnement de dev"));
        assert!(finding
            .remediation
            .as_deref()
            .unwrap()
            .contains("documentation d'installation du projet"));
    }

    #[test]
    fn sans_compose_du_tout_le_refus_reste_ferme() {
        // Le service vient alors d'un autre indice, plus proche du deploiement.
        let mut f = depot_sain();
        f.services.unsupported = vec!["elasticsearch".into()];
        f.compose = None;

        assert_eq!(
            BaseNonSupportee.check(&f).unwrap().severity,
            Severity::Blocker
        );
    }
}

#[cfg(test)]
mod recette_de_build {
    use super::*;
    use crate::tests::depot_sain;
    use ynp_core::facts::{BuildRecipe, StackFacts};

    fn depot_node(build_steps: Vec<String>) -> RepoFacts {
        let mut f = depot_sain();
        f.stack = StackFacts {
            primary: Technology::NodeJs,
            build_script: Some("npm run build".into()),
            ..Default::default()
        };
        f.build = Some(BuildRecipe {
            dockerfile_path: ".github/deployment/node/Dockerfile".into(),
            build_steps,
            ..Default::default()
        });
        f
    }

    #[test]
    fn un_dockerfile_qui_assemble_un_artefact_preconstruit_est_refuse() {
        // Cas reel d'AFFiNE : le Dockerfile de deploiement recopie un `dist/`
        // bati ailleurs. L'outil annoncait « faisable, 100/100 » alors qu'il ne
        // savait pas du tout construire l'application.
        let finding = RecetteIntrouvable.check(&depot_node(vec![])).unwrap();

        assert_eq!(finding.severity, Severity::Blocker);
        assert!(finding.detail.contains("il ne l'inventera pas"));
        assert!(finding
            .remediation
            .as_deref()
            .unwrap()
            .contains("appspec.toml"));
    }

    #[test]
    fn un_dockerfile_qui_construit_vraiment_ne_declenche_rien() {
        let f = depot_node(vec!["npm ci".into(), "npm run build".into()]);
        assert!(RecetteIntrouvable.check(&f).is_none());
    }

    #[test]
    fn une_application_qui_n_a_rien_a_construire_n_est_pas_concernee() {
        // Un binaire Go preconstruit n'a pas d'etape de build, et c'est normal.
        let mut f = depot_sain();
        f.stack = StackFacts {
            primary: Technology::Go,
            ..Default::default()
        };
        f.build = Some(BuildRecipe {
            dockerfile_path: "Dockerfile".into(),
            build_steps: vec![],
            ..Default::default()
        });
        assert!(RecetteIntrouvable.check(&f).is_none());
    }
}

#[cfg(test)]
mod dockerfiles_hors_produit {
    use super::*;
    use crate::tests::depot_sain;
    use ynp_core::facts::{BuildRecipe, StackFacts};

    #[test]
    fn un_depot_dont_tous_les_dockerfiles_sont_dans_la_ci_est_refuse() {
        // Cas reel d'AFFiNE : .github/deployment/node/Dockerfile,
        // .github/helm/.../Dockerfile_pgvector et .render/Dockerfile. Aucun ne
        // construit depuis les sources — ils assemblent ce que la chaine
        // d'integration a bati ailleurs.
        let mut f = depot_sain();
        f.stack = StackFacts {
            primary: Technology::NodeJs,
            build_script: Some("npm run build".into()),
            ..Default::default()
        };
        f.tree = vec![
            ".github/deployment/node/Dockerfile".into(),
            ".github/helm/separate-config/Dockerfile_pgvector".into(),
            ".render/Dockerfile".into(),
            "package.json".into(),
        ];
        f.build = Some(BuildRecipe {
            dockerfile_path: ".github/deployment/node/Dockerfile".into(),
            // Une etape existe, mais ce n'est pas une construction.
            build_steps: vec!["node ./scripts/docker-clean.mjs".into()],
            ..Default::default()
        });

        let finding = RecetteIntrouvable.check(&f).unwrap();
        assert_eq!(finding.severity, Severity::Blocker);
        assert!(finding.detail.contains("integration continue"));
    }

    #[test]
    fn un_seul_dockerfile_hors_ci_suffit_a_lever_le_doute() {
        let mut f = depot_sain();
        f.stack = StackFacts {
            primary: Technology::NodeJs,
            build_script: Some("npm run build".into()),
            ..Default::default()
        };
        f.tree = vec![".github/deployment/Dockerfile".into(), "Dockerfile".into()];
        f.build = Some(BuildRecipe {
            dockerfile_path: "Dockerfile".into(),
            build_steps: vec!["yarn build".into()],
            ..Default::default()
        });
        assert!(RecetteIntrouvable.check(&f).is_none());
    }

    #[test]
    fn la_detection_ne_se_declenche_pas_sur_un_depot_sans_dockerfile() {
        assert!(!tous_les_dockerfiles_sont_hors_produit(&[
            "package.json".to_string()
        ]));
    }
}

#[cfg(test)]
mod binaires_publies {
    use super::*;
    use crate::tests::depot_sain;
    use ynp_core::facts::{ArchAsset, BuildRecipe, SourceSelection};

    #[test]
    fn l_avertissement_de_build_se_tait_quand_rien_n_est_a_compiler() {
        let mut f = depot_sain();
        f.build = Some(BuildRecipe {
            dockerfile_path: "Dockerfile".into(),
            build_steps: vec!["npm ci".into(), "npm run build".into()],
            ..Default::default()
        });
        assert!(
            BuildGourmand.check(&f).is_some(),
            "sans binaire publie, l'alerte tient"
        );

        f.selection = Some(SourceSelection {
            prebuilt: vec![ArchAsset {
                arch: "amd64".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        assert!(
            BuildGourmand.check(&f).is_none(),
            "avec un binaire publie, la machine cible ne compile rien"
        );
    }
}
