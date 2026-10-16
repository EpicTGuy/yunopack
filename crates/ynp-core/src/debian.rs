//! Les paquets de Debian 12 (bookworm), la cible de YunoHost.
//!
//! Une image construite sur une Debian plus recente ou sur Ubuntu nomme des
//! paquets qui n'existent pas dans bookworm (`imagemagick-7.q16`, `dasel`
//! chez convertx) : un seul suffit a faire echouer l'installation entiere.
//! L'index est charge par la recuperation du depot, depuis un cache ; sans
//! lui, rien n'est filtre.

use std::collections::HashSet;
use std::sync::OnceLock;

static INDEX: OnceLock<HashSet<String>> = OnceLock::new();

/// Installe l'index des paquets de bookworm, une fois par processus.
pub fn installer_index(paquets: HashSet<String>) {
    let _ = INDEX.set(paquets);
}

/// `Some(true)` si le paquet manque a bookworm, `None` sans index.
pub fn absent(paquet: &str) -> Option<bool> {
    INDEX.get().map(|i| !i.contains(paquet))
}

/// Les noms des paquets d'un fichier `Packages` de Debian.
pub fn noms_du_fichier_packages(texte: &str) -> HashSet<String> {
    texte
        .lines()
        .filter_map(|l| l.strip_prefix("Package: "))
        .map(|n| n.trim().to_string())
        .collect()
}
