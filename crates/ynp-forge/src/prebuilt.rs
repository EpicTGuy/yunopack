//! Reconnaissance des binaires publies par architecture.
//!
//! Les paquets YunoHost de reference — `gotify_ynh`, `memos_ynh`,
//! `miniflux_ynh` — telechargent tous un binaire deja construit plutot que
//! l'archive des sources, et annoncent `ram.build = 50M`. Compiler sur la
//! machine cible est le principal motif d'echec d'installation sur les petites
//! instances, et c'est ce que la regle BUILD001 signale.
//!
//! Reconnaitre ces assets est donc le levier le plus rentable du projet.

use ynp_core::facts::{ArchAsset, Release, ReleaseAsset};

/// Architectures Debian visees, avec les graphies rencontrees dans les noms
/// d'assets. L'ordre des graphies compte : la plus specifique d'abord, sans
/// quoi `arm` attraperait `arm64`.
const ARCHS: &[(&str, &[&str])] = &[
    ("arm64", &["arm64", "aarch64"]),
    ("armhf", &["armv7", "arm-7", "armhf", "armv6", "arm32"]),
    ("amd64", &["amd64", "x86_64", "x64"]),
    ("i386", &["i386", "386", "x86"]),
];

/// Extensions propres aux applications de bureau. Leur presence dans une
/// release indique un client graphique, pas un serveur : AFFiNE publie ainsi
/// des `.appimage`, `.dmg` et `.flatpak` qui n'ont rien a voir avec le serveur
/// que l'on chercherait a packager.
const BUREAU: &[&str] = &[
    ".dmg",
    ".exe",
    ".msi",
    ".appimage",
    ".flatpak",
    ".snap",
    ".apk",
];

/// Extensions exploitables pour un binaire serveur.
const ARCHIVES: &[&str] = &[".tar.gz", ".tgz", ".tar.xz", ".tar.bz2", ".zip"];

/// Selectionne un asset par architecture, s'il y en a.
///
/// Rend une liste vide quand la release n'en contient pas d'exploitable : on
/// retombe alors sur l'archive des sources.
pub fn select(release: &Release) -> Vec<ArchAsset> {
    if est_une_release_de_bureau(&release.assets) {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (arch, graphies) in ARCHS {
        if let Some(asset) = meilleur_asset(&release.assets, graphies) {
            out.push(ArchAsset {
                arch: (*arch).to_string(),
                pattern: motif(&asset.name, graphies),
                name: asset.name.clone(),
                url: asset.url.clone(),
                sha256: String::new(),
                empreinte_publiee: asset.sha256.clone(),
                taille: asset.size,
                sous_repertoire: None,
                executables: Vec::new(),
                extract: ARCHIVES
                    .iter()
                    .any(|e| asset.name.to_lowercase().ends_with(e)),
            });
        }
    }

    // Un seul asset pour une seule architecture n'est pas un jeu multi-arch :
    // c'est plus souvent un artefact annexe (somme de controle, notes de
    // version) mal reconnu. On exige au moins une correspondance franche.
    if out.len() == 1 && !out[0].name.to_lowercase().contains("linux") {
        return Vec::new();
    }
    out
}

fn est_une_release_de_bureau(assets: &[ReleaseAsset]) -> bool {
    // Seuls les formats d'une interface graphique le disent : un `.exe` ou un
    // `.apk` accompagne aussi les serveurs en Go ou en Rust (wrp, fishnet,
    // anubis), dont le binaire Linux etait alors ignore.
    const GRAPHIQUES: &[&str] = &[".dmg", ".appimage", ".flatpak", ".snap"];
    assets.iter().any(|a| {
        let n = a.name.to_lowercase();
        GRAPHIQUES.iter().any(|e| n.ends_with(e))
    })
}

fn meilleur_asset<'a>(assets: &'a [ReleaseAsset], graphies: &[&str]) -> Option<&'a ReleaseAsset> {
    assets
        .iter()
        .filter(|a| convient(&a.name, graphies))
        .max_by_key(|a| score(&a.name))
}

/// Un asset convient s'il vise Linux et l'architecture demandee, dans un
/// format que `ynh_setup_source` sait deployer.
fn convient(name: &str, graphies: &[&str]) -> bool {
    let n = name.to_lowercase();

    if ["darwin", "macos", "windows", "freebsd", "android"]
        .iter()
        .any(|o| n.contains(o))
    {
        return false;
    }
    if BUREAU.iter().any(|e| n.ends_with(e)) {
        return false;
    }
    if [
        ".sha256",
        ".asc",
        ".sig",
        ".md5",
        ".txt",
        ".json",
        ".yml",
        ".yaml",
        ".changes",
        ".deb",
        ".rpm",
        ".dsc",
        ".buildinfo",
        ".sbom",
        ".spdx",
        ".pem",
        ".crt",
        ".intoto.jsonl",
    ]
    .iter()
    .any(|e| n.ends_with(e))
    {
        return false;
    }
    // Un installeur n'est pas le serveur : pangolin publie
    // `installer_linux_amd64`, qui deploie l'application par Docker.
    let nom_seul = n.rsplit('/').next().unwrap_or(&n);
    if nom_seul.starts_with("installer")
        || nom_seul.contains("-installer")
        || nom_seul.contains("_installer")
    {
        return false;
    }
    // Une archive, ou un binaire nu : beaucoup de projets Go publient un
    // executable sans extension, que YunoHost deploie avec `extract = false`.
    let derniere_partie = n.rsplit('/').next().unwrap_or(&n).to_string();
    let a_une_extension = derniere_partie.rsplit_once('.').is_some_and(|(_, ext)| {
        // `miniflux-linux-amd64` n'a pas d'extension ; `app.tar.gz` si.
        // Jusqu'a huit lettres : `.changes` (metadonnees d'un paquet Debian,
        // retenues chez zoneminder) n'est pas un binaire sans extension.
        ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphabetic())
    });
    if a_une_extension && !ARCHIVES.iter().any(|e| n.ends_with(e)) {
        return false;
    }

    // La graphie doit apparaitre isolee, pour que « arm » ne corresponde pas a
    // « arm64 » ni « 386 » a « x86_64 ».
    graphies.iter().any(|g| isole(&n, g))
}

fn isole(nom: &str, motif: &str) -> bool {
    let separateur = |c: char| !c.is_ascii_alphanumeric();
    nom.match_indices(motif).any(|(i, _)| {
        let avant = i == 0 || nom[..i].chars().next_back().is_some_and(separateur);
        let apres_at = i + motif.len();
        let apres = apres_at == nom.len() || nom[apres_at..].chars().next().is_some_and(separateur);
        // `x86_64` et `x86-64` sont de l'amd64, pas de l'i386 :
        // dmarc-report-viewer voyait son archive x86_64 retenue pour i386.
        let suite_64 = nom[apres_at..].starts_with("_64") || nom[apres_at..].starts_with("-64");
        avant && apres && !suite_64
    })
}

/// Entre plusieurs candidats, prefere celui qui annonce Linux explicitement.
fn score(name: &str) -> i32 {
    let n = name.to_lowercase();
    let mut s = 0;
    if n.contains("linux") {
        s += 4;
    }
    if n.contains("musl") {
        s -= 1; // glibc convient mieux a Debian
    }
    if n.contains("static") {
        s += 1;
    }
    s
}

/// Motif reconnaissant l'asset d'une version a l'autre.
///
/// Le numero de version est remplace par un joker, pour que
/// `autoupdate.asset.<arch>` continue de fonctionner apres une mise a jour.
fn motif(name: &str, _graphies: &[&str]) -> String {
    match plage_de_version(name) {
        // Le numero de version change a chaque release : on le generalise.
        Some((debut, fin)) => {
            format!("{}.*{}", echapper(&name[..debut]), echapper(&name[fin..]))
        }
        // Nom stable d'une version a l'autre : on l'ancre, comme le font les
        // paquets officiels (`^miniflux-linux-amd64$`).
        None => format!("^{}$", echapper(name)),
    }
}

/// Localise un numero de version de la forme `X.Y` ou `X.Y.Z` dans un nom.
///
/// Distinguer `0.31.0` d'un `64` de nom d'architecture demande d'exiger au
/// moins un point : sans quoi `amd64` serait pris pour une version.
fn plage_de_version(name: &str) -> Option<(usize, usize)> {
    let o = name.as_bytes();
    let mut i = 0;
    while i < o.len() {
        if !o[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let debut = i;
        let mut points = 0;
        while i < o.len() {
            match o[i] {
                b'0'..=b'9' => i += 1,
                b'.' if i + 1 < o.len() && o[i + 1].is_ascii_digit() => {
                    points += 1;
                    i += 1;
                }
                _ => break,
            }
        }
        if points >= 1 {
            return Some((debut, i));
        }
    }
    None
}

fn echapper(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                format!("\\{c}")
            }
            _ => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets(noms: &[&str]) -> Vec<ReleaseAsset> {
        noms.iter()
            .map(|n| ReleaseAsset {
                name: (*n).to_string(),
                url: format!("https://x/{n}"),
                size: 1024,
                sha256: None,
            })
            .collect()
    }

    #[test]
    fn l_empreinte_publiee_par_la_forge_suit_l_asset() {
        // Cas de FlareSolverr : GitHub publie `sha256:…` sur chaque actif.
        // La transporter jusqu'ici evite de rapatrier 263 Mo pour la
        // recalculer.
        let mut r = release(&["app-linux-amd64.tar.gz"]);
        r.assets[0].sha256 = Some("ab".repeat(32));
        r.assets[0].size = 263_000_000;
        let choisis = select(&r);
        assert_eq!(
            choisis[0].empreinte_publiee.as_deref(),
            Some("ab".repeat(32).as_str())
        );
        assert_eq!(choisis[0].taille, 263_000_000);
    }

    fn release(noms: &[&str]) -> Release {
        Release {
            tag: "v1.0.0".into(),
            assets: assets(noms),
            ..Default::default()
        }
    }

    #[test]
    fn x86_64_n_est_pas_de_l_i386() {
        let r = release(&["linux-aarch64.zip", "linux-x86_64.zip"]);
        let archs: Vec<String> = select(&r).into_iter().map(|a| a.arch).collect();
        assert_eq!(archs, vec!["arm64", "amd64"]);
    }

    #[test]
    fn les_binaires_de_gotify_sont_reconnus_par_architecture() {
        // Jeu d'assets reel, que le paquet officiel utilise tel quel.
        let r = release(&[
            "gotify-linux-386.zip",
            "gotify-linux-amd64.zip",
            "gotify-linux-arm-7.zip",
            "gotify-linux-arm64.zip",
            "gotify-linux-riscv64.zip",
            "gotify-windows-amd64.exe.zip",
        ]);
        let choisis = select(&r);

        let par_arch: Vec<(&str, &str)> = choisis
            .iter()
            .map(|a| (a.arch.as_str(), a.name.as_str()))
            .collect();
        assert_eq!(
            par_arch,
            vec![
                ("arm64", "gotify-linux-arm64.zip"),
                ("armhf", "gotify-linux-arm-7.zip"),
                ("amd64", "gotify-linux-amd64.zip"),
                ("i386", "gotify-linux-386.zip"),
            ]
        );
    }

    #[test]
    fn un_exe_a_cote_ne_fait_pas_une_application_de_bureau() {
        // Cas de wrp : binaires nus pour chaque systeme.
        let r = release(&[
            "wrp-amd64-linux",
            "wrp-arm64-linux",
            "wrp-amd64-windows.exe",
            "wrp-amd64-macos",
        ]);
        let noms: Vec<String> = select(&r).into_iter().map(|a| a.name).collect();
        assert!(noms.contains(&"wrp-amd64-linux".to_string()));
        assert!(noms.contains(&"wrp-arm64-linux".to_string()));
    }

    #[test]
    fn les_binaires_windows_et_macos_sont_ecartes() {
        let r = release(&["app-windows-amd64.zip", "app-darwin-arm64.tar.gz"]);
        assert!(select(&r).is_empty());
    }

    #[test]
    fn metadonnees_de_paquet_et_installeurs_ne_sont_pas_des_binaires() {
        // zoneminder : fichiers .changes de paquets Debian.
        let r = release(&[
            "zoneminder_1.38.4+trixie1_amd64.changes",
            "zoneminder_1.38.4+trixie1_arm64.changes",
        ]);
        assert!(select(&r).is_empty());
        // pangolin : l'installeur, pas le serveur.
        let r = release(&["installer_linux_amd64", "installer_linux_arm64"]);
        assert!(select(&r).is_empty());
    }

    #[test]
    fn une_release_d_application_de_bureau_n_est_pas_prise_pour_un_serveur() {
        // Cas reel d'AFFiNE : ses assets sont des clients graphiques. Le zip
        // linux-x64 aurait pu passer pour un binaire serveur.
        let r = release(&[
            "affine-0.27.4-linux-x64.appimage",
            "affine-0.27.4-linux-x64.zip",
            "affine-0.27.4-macos-arm64.dmg",
            "affine-0.27.4-linux-x64.flatpak",
        ]);
        assert!(select(&r).is_empty(), "un serveur ne publie pas de .dmg");
    }

    #[test]
    fn les_sommes_et_signatures_ne_sont_pas_des_binaires() {
        let r = release(&[
            "app-linux-amd64.tar.gz.sha256",
            "app-linux-amd64.tar.gz.asc",
        ]);
        assert!(select(&r).is_empty());
    }

    #[test]
    fn une_graphie_ne_deborde_pas_sur_une_autre() {
        // « arm » ne doit pas attraper « arm64 », ni « 386 » attraper « x86_64 ».
        assert!(isole("app-linux-arm64.zip", "arm64"));
        assert!(!isole("app-linux-arm64.zip", "arm-7"));
        assert!(isole("app-linux-x86_64.tar.gz", "x86_64"));
        assert!(!isole("app-linux-x86_64.tar.gz", "386"));
    }

    #[test]
    fn la_version_glibc_est_preferee_a_la_version_musl() {
        let r = release(&["app-linux-amd64.tar.gz", "app-linux-amd64-musl.tar.gz"]);
        let c = select(&r);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "app-linux-amd64.tar.gz");
    }

    #[test]
    fn le_motif_d_autoupdate_survit_a_un_changement_de_version() {
        let r = release(&["memos_1.2.3_linux_amd64.tar.gz"]);
        let c = select(&r);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].pattern, "memos_.*_linux_amd64\\.tar\\.gz");
    }

    #[test]
    fn un_nom_stable_est_ancre_plutot_que_generalise() {
        // Comme le fait le paquet officiel de miniflux : `^miniflux-linux-amd64$`.
        let r = release(&["gotify-linux-amd64.zip", "gotify-linux-arm64.zip"]);
        let c = select(&r);
        assert_eq!(c[1].pattern, "^gotify-linux-amd64\\.zip$");
    }

    #[test]
    fn un_binaire_nu_est_retenu_et_marque_comme_non_extractible() {
        // Cas reel de miniflux : des executables sans extension, que le paquet
        // officiel declare avec `extract = false` et `rename`.
        let r = release(&[
            "miniflux-linux-amd64",
            "miniflux-linux-amd64.sha256",
            "miniflux-linux-arm64",
            "miniflux-darwin-amd64",
            "miniflux-2.3.3-1.0.x86_64.rpm",
        ]);
        let c = select(&r);

        let noms: Vec<&str> = c.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(noms, vec!["miniflux-linux-arm64", "miniflux-linux-amd64"]);
        assert!(
            c.iter().all(|a| !a.extract),
            "un binaire nu ne s'extrait pas"
        );
        assert_eq!(c[1].pattern, "^miniflux-linux-amd64$");
    }

    #[test]
    fn une_archive_reste_marquee_comme_extractible() {
        let c = select(&release(&[
            "app-linux-amd64.tar.gz",
            "app-linux-arm64.tar.gz",
        ]));
        assert!(c.iter().all(|a| a.extract));
    }

    #[test]
    fn un_numero_d_architecture_n_est_pas_pris_pour_une_version() {
        // `amd64` contient « 64 » : sans exiger un point, le motif serait faux.
        assert_eq!(plage_de_version("gotify-linux-amd64.zip"), None);
        assert_eq!(
            plage_de_version("memos_0.31.0_linux_amd64.tar.gz"),
            Some((6, 12))
        );
    }

    #[test]
    fn une_release_sans_binaire_retombe_sur_les_sources() {
        assert!(select(&release(&[])).is_empty());
        assert!(select(&release(&["CHANGELOG.md", "config.json.example"])).is_empty());
    }
}
