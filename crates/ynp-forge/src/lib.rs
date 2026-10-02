//! Acces aux forges : metadonnees, releases, archive du depot.
//!
//! Seule partie du projet qui parle au reseau, et elle ne parle qu'a des
//! forges. C'est ce qui permet a la garantie de l'ADR-002 — aucun appel a un
//! modele de langage — d'etre verifiable en lisant un seul crate.

pub mod alternatives;
/// Un client HTTP borne dans le temps.
///
/// Sans limite, un travail reste bloque sur « recuperation du depot… »
/// indefiniment : la connexion ne rend jamais la main, personne ne signale
/// rien, et l'utilisateur regarde une etape qui n'avancera plus. Constate sur
/// deux travaux restes vingt minutes a 0,3 % de processeur.
///
/// Le delai porte sur l'attente entre deux octets, pas sur la duree totale :
/// une archive de plusieurs centaines de megaoctets sur une ligne lente doit
/// passer, une connexion morte non.
pub fn client_http() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(concat!("yunopack/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
}

/// Envoie une requete d'API et lit sa reponse, en retentant sur une erreur
/// de transport ou une reponse 5xx.
///
/// Une coupure breve faisait echouer l'analyse entiere : pendant une
/// connexion degradee, 77 depots sur 120 se sont arretes sur « error
/// sending request » ou « error decoding response body », a la premiere
/// requete. Trois essais, espaces de 2 puis 4 secondes ; les codes 4xx
/// (absent, quota) sont des reponses, et ne se retentent pas.
pub(crate) async fn obtenir(
    requete: impl Fn() -> reqwest::RequestBuilder,
) -> Result<(u16, String), reqwest::Error> {
    let mut essai = 1;
    loop {
        let resultat = match requete().send().await {
            Ok(r) => {
                let statut = r.status().as_u16();
                r.text().await.map(|corps| (statut, corps))
            }
            Err(e) => Err(e),
        };
        match resultat {
            Ok((statut, _)) if statut >= 500 && essai < 3 => {}
            Ok(r) => return Ok(r),
            Err(e) if essai >= 3 => return Err(e),
            Err(e) => tracing::warn!("{e} — nouvelle tentative ({essai}/2)"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(2 * essai)).await;
        essai += 1;
    }
}

pub mod archive;
pub mod catalogue;
pub mod client;
pub mod git;
pub mod gitea;
pub mod github;
pub mod gitlab;
pub mod prebuilt;
pub mod recherche;
pub mod scorecard;
pub mod sources;
pub mod url;
pub mod wishlist;
pub mod workflows;

use ynp_analyze::ForgeData;
use ynp_core::tree::RepoTree;

pub use github::{ForgeError, GitHub};
pub use sources::{SourceChoice, SourceKind};
pub use url::UrlError;
pub use wishlist::Souhait;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error(transparent)]
    Url(#[from] UrlError),
    #[error(transparent)]
    Forge(#[from] ForgeError),
}

/// Tout ce que la forge sait d'un depot, plus son contenu.
pub struct Fetched {
    pub forge: ForgeData,
    pub tree: RepoTree,
    pub choice: SourceChoice,
    /// Somme de controle de l'archive choisie, telle qu'elle ira au manifest.
    pub sha256: String,
    /// Source retenue, avec les binaires preconstruits s'il y en a.
    pub selection: ynp_core::facts::SourceSelection,
}

/// Recupere un depot a partir de son URL.
///
/// L'archive est telechargee une seule fois : elle sert a la fois a l'analyse
/// du contenu et au calcul de la somme de controle qui figurera dans le
/// manifest. Telecharger deux fois exposerait a ce que l'amont change entre
/// les deux, et donc a publier une somme qui ne correspond a rien.
pub async fn fetch(repo_url: &str) -> Result<Fetched, FetchError> {
    let mut source = url::parse(repo_url)?;
    let client = client::Client::pour(&source).await?;

    let meta = client.repo(&mut source).await?;
    let releases = client.releases(&source).await?;
    let tags = client.tags(&source).await?;

    let mut choice =
        sources::choose_avec(&source, &releases, &tags, |s, r| client.archive_url(s, r));

    // Sans publication ni tag, la source est une branche. Son archive change a
    // chaque commit, alors que le manifeste en fige l'empreinte : au premier
    // commit suivant, l'installation echouerait sur une somme qui ne
    // correspond plus. On la fige donc sur son dernier commit — dont la date
    // devient la version, comme le fait l'autoupdater de YunoHost. Huit depots
    // sur un echantillon de 86 demandaient leur version pour cette raison,
    // alors que l'outil ecrivait lui-meme qu'elle serait « datee du commit ».
    if choice.kind == sources::SourceKind::Commit {
        match client.dernier_commit(&source, &choice.reference).await {
            Ok((sha, date)) => {
                choice.url = client.archive_url(&source, &sha);
                choice.reference = sha;
                choice.version = Some(date);
            }
            Err(e) => tracing::warn!(
                "dernier commit de {} introuvable ({e}) : la source reste la branche",
                choice.reference
            ),
        }
    }

    let bytes = client.download(&choice.url).await?;
    let mut sha256 = sources::sha256(&bytes);
    let mut tree = archive::extract(&bytes).map_err(ForgeError::from)?;

    // Un depot qui heberge plusieurs logiciels les versionne separement. Le
    // workflow qui construit l'image Docker se declenche sur la famille de
    // tags du serveur — celui que le paquet transpose. Buzz publie ainsi son
    // application de bureau sous `v*`, avec releases, et son serveur sous
    // `relay-v*` : sans cette regle, le paquet prenait la version de bureau.
    for prefixe in workflows::prefixes_de_l_image(&tree) {
        let Some((cle, _)) = sources::famille(&format!("{prefixe}1.0")) else {
            continue;
        };
        if choice.version_regex.as_deref() == Some(sources::version_regex(&cle).as_str()) {
            break;
        }
        let de_la_famille = client
            .tags_du_prefixe(&source, &prefixe)
            .await
            .unwrap_or_default();
        let Some(autre) =
            sources::choisir_dans_la_famille(&source, &de_la_famille, &cle, |s, r| {
                client.archive_url(s, r)
            })
        else {
            continue;
        };
        tracing::info!(
            "le workflow Docker suit les tags « {prefixe}* » : source {} au lieu de {}",
            autre.reference,
            choice.reference
        );
        let bytes = client.download(&autre.url).await?;
        sha256 = sources::sha256(&bytes);
        tree = archive::extract(&bytes).map_err(ForgeError::from)?;
        choice = autre;
        break;
    }

    source.commit = Some(choice.reference.clone());

    // Les binaires deja construits, quand l'amont en publie, evitent de
    // compiler sur la machine cible — ce que font tous les paquets YunoHost de
    // reference. Leur somme de controle demande un telechargement chacun ;
    // c'est le prix d'un paquet qui s'installe sur une petite instance.
    let prebuilt = releases
        .iter()
        .find(|r| r.tag == choice.reference)
        .map(prebuilt::select)
        .unwrap_or_default();

    // Un binaire qu'on ne peut pas telecharger — retire par l'amont, servi
    // par un stockage tiers qui refuse, protege par un quota — n'est pas une
    // raison d'abandonner l'analyse. On l'ecarte : le paquet se construira
    // depuis les sources, ce qui est moins bien mais reste juste. Constate sur
    // gitlab-runner, dont les assets sont servis hors de la forge.
    let mut retenus = Vec::new();
    for mut asset in prebuilt {
        // Quand la forge publie l'empreinte, la calculer soi-meme reviendrait
        // a rapatrier le fichier pour retrouver ce qu'elle a deja dit.
        // FlareSolverr en publie un de 263 Mo : plusieurs minutes de
        // telechargement par analyse, sans rien afficher pendant ce temps.
        // La forme de l'archive decide ou atterrit le binaire. Son debut
        // suffit a la lire.
        let tar_gz = {
            let n = asset.name.to_lowercase();
            n.ends_with(".tar.gz") || n.ends_with(".tgz")
        };
        if let Some(empreinte) = asset.empreinte_publiee.clone() {
            asset.sha256 = empreinte;
            if asset.extract && tar_gz {
                asset.sous_repertoire = client
                    .debut(&asset.url, 64 * 1024)
                    .await
                    .and_then(|d| archive::repertoire_racine(&d));
            }
            retenus.push(asset);
            continue;
        }
        tracing::info!(
            "telechargement de {} ({:.0} Mo) pour en calculer l'empreinte, \
             que la forge ne publie pas",
            asset.name,
            asset.taille as f64 / 1e6
        );
        match client.download(&asset.url).await {
            Ok(bytes) => {
                asset.sha256 = sources::sha256(&bytes);
                if asset.extract && tar_gz {
                    asset.sous_repertoire = archive::repertoire_racine(&bytes);
                }
                retenus.push(asset);
            }
            Err(e) => tracing::warn!("binaire {} ignore : {e}", asset.name),
        }
    }
    let prebuilt = retenus;

    let selection = ynp_core::facts::SourceSelection {
        reference: choice.reference.clone(),
        strategy: choice.strategy.clone(),
        version_regex: choice.version_regex.clone(),
        version: choice.version.clone(),
        kind: match choice.kind {
            SourceKind::Release => "release",
            SourceKind::Tag => "tag",
            SourceKind::Commit => "commit",
        }
        .to_string(),
        url: choice.url.clone(),
        sha256: sha256.clone(),
        prebuilt,
    };

    Ok(Fetched {
        forge: ForgeData {
            source,
            meta,
            releases,
            tags,
        },
        tree,
        choice,
        sha256,
        selection,
    })
}
