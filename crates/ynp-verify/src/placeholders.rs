//! Jetons `__MAJUSCULE__` des fichiers de configuration.
//!
//! YunoHost remplace tout jeton de cette forme par la valeur du reglage
//! homonyme en minuscules, **y compris dans les commentaires**. Un jeton sans
//! reglage correspondant fait echouer l'installation avec « Variable $x wasn't
//! initialized » — ce qui nous est arrive en ecrivant `__MAJUSCULES__` dans un
//! commentaire a titre d'exemple.
//!
//! Ce controle est donc ne d'une installation reelle, et c'est le genre de
//! defaut qu'aucune relecture ne rattrape.

use std::collections::BTreeSet;
use ynp_core::AppSpec;

/// Reglages que le coeur de YunoHost fournit toujours.
const TOUJOURS_FOURNIS: &[&str] = &["APP", "DOMAIN", "PATH", "INSTALL_DIR"];

/// Reglages fournis seulement si la ressource correspondante est declaree.
const SELON_RESSOURCE: &[(&str, &str)] = &[
    ("PORT", "ports"),
    ("DATA_DIR", "data_dir"),
    ("DB_NAME", "database"),
    ("DB_USER", "database"),
    ("DB_PWD", "database"),
    ("PHP_VERSION", "phpfpm"),
    ("NODEJS_DIR", "nodejs"),
    ("PATH_WITH_NODEJS", "nodejs"),
    ("RUBY_DIR", "ruby"),
    ("PATH_WITH_RUBY", "ruby"),
    ("GO_DIR", "go"),
    ("PATH_WITH_GO", "go"),
];

/// Reglages definis par le script d'installation lui-meme.
const DEFINIS_PAR_LE_SCRIPT: &[&str] = &["SECRET", "MAIL_PWD"];

/// Vrai pour un reglage que le script d'installation doit definir lui-meme.
pub fn est_defini_par_le_script(jeton: &str) -> bool {
    DEFINIS_PAR_LE_SCRIPT.contains(&jeton)
}

/// Les variables qu'un script definit, en jetons : `redis_db=$(…)` et
/// `ynh_app_setting_set --key=redis_db` rendent __REDIS_DB__ disponible.
///
/// C'est la regle meme de YunoHost : un jeton est remplace par la variable du
/// meme nom visible dans le script. Sans elle, `verify` refusait tout paquet
/// retouche a la main — celui de Buzz, qui tire ses cles a l'installation.
pub fn definis_par_les_scripts(scripts: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for ligne in scripts.lines() {
        let l = ligne.trim();
        if l.starts_with('#') {
            continue;
        }
        for (i, _) in l.match_indices("--key=") {
            let nom: String = l[i + 6..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !nom.is_empty() {
                out.insert(nom.to_ascii_uppercase());
            }
        }
        let l = l.strip_prefix("local ").unwrap_or(l);
        if let Some((nom, _)) = l.split_once('=') {
            let valide = !nom.is_empty()
                && nom.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                && nom
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if valide {
                out.insert(nom.to_ascii_uppercase());
            }
        }
    }
    out
}

/// Jetons presents dans un contenu, dans l'ordre d'apparition.
pub fn extraire(contenu: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let octets = contenu.as_bytes();
    let mut i = 0;

    while i + 4 < octets.len() {
        if octets[i] != b'_' || octets[i + 1] != b'_' {
            i += 1;
            continue;
        }
        let debut = i + 2;
        let mut j = debut;
        while j < octets.len()
            && (octets[j].is_ascii_uppercase() || octets[j].is_ascii_digit() || octets[j] == b'_')
        {
            // Un double tiret bas ferme le jeton.
            if octets[j] == b'_' && j + 1 < octets.len() && octets[j + 1] == b'_' {
                break;
            }
            j += 1;
        }
        if j > debut && j + 1 < octets.len() && octets[j] == b'_' && octets[j + 1] == b'_' {
            out.insert(contenu[debut..j].to_string());
            i = j + 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Jetons qu'un paquet peut resoudre, d'apres son seul manifest.
///
/// Permet de controler un paquet ecrit a la main, sans specification — y
/// compris ceux d'autres auteurs. Le verificateur devient ainsi utilisable
/// comme un linter autonome.
pub fn disponibles_depuis_manifest(manifest: &toml::Value) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = TOUJOURS_FOURNIS.iter().map(|s| s.to_string()).collect();
    out.extend(DEFINIS_PAR_LE_SCRIPT.iter().map(|s| s.to_string()));

    let declare = |chemin: &[&str]| -> bool {
        let mut courant = manifest;
        for c in chemin {
            match courant.get(c) {
                Some(v) => courant = v,
                None => return false,
            }
        }
        true
    };

    let actif = |ressource: &str| match ressource {
        "ports" => declare(&["resources", "ports"]),
        "data_dir" => declare(&["resources", "data_dir"]),
        "database" => declare(&["resources", "database"]),
        "phpfpm" => manifest
            .get("resources")
            .and_then(|r| r.get("apt"))
            .and_then(|a| a.get("packages"))
            .map(|p| p.to_string().contains("php"))
            .unwrap_or(false),
        "nodejs" => declare(&["resources", "nodejs"]),
        "ruby" => declare(&["resources", "ruby"]),
        "go" => declare(&["resources", "go"]),
        _ => false,
    };
    for (jeton, ressource) in SELON_RESSOURCE {
        if actif(ressource) {
            out.insert((*jeton).to_string());
        }
    }

    // Un port nomme devient `port_<nom>` : `health` donne __PORT_HEALTH__.
    if let Some(toml::Value::Table(ports)) = manifest.get("resources").and_then(|r| r.get("ports"))
    {
        for nom in ports.keys().filter(|n| *n != "main") {
            out.insert(format!("PORT_{}", nom.to_ascii_uppercase()));
        }
    }

    // Chaque question d'installation devient un reglage du meme nom.
    if let Some(toml::Value::Table(questions)) = manifest.get("install") {
        for nom in questions.keys() {
            out.insert(nom.to_ascii_uppercase());
        }
    }
    out
}

/// Jetons qu'un paquet peut resoudre, d'apres ce que sa specification declare.
pub fn disponibles(spec: &AppSpec) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = TOUJOURS_FOURNIS.iter().map(|s| s.to_string()).collect();
    out.extend(DEFINIS_PAR_LE_SCRIPT.iter().map(|s| s.to_string()));

    let actif = |ressource: &str| match ressource {
        "ports" => spec.resources.ports,
        "data_dir" => spec.resources.data_dir,
        "database" => spec.resources.database.manifest_type().is_some(),
        "phpfpm" => spec.features.phpfpm,
        "nodejs" => spec.resources.nodejs_version.is_some(),
        "ruby" => spec.resources.ruby_version.is_some(),
        "go" => spec.resources.go_version.is_some(),
        _ => false,
    };
    for (jeton, ressource) in SELON_RESSOURCE {
        if actif(ressource) {
            out.insert((*jeton).to_string());
        }
    }

    // Les reponses aux questions d'installation deviennent des reglages.
    for q in &spec.install.extra {
        out.insert(q.name.to_ascii_uppercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_jetons_sont_extraits_y_compris_dans_un_commentaire() {
        // C'est precisement ce cas qui a fait echouer une installation.
        let c = "# Les __MAJUSCULES__ sont remplacees\nPORT=__PORT__\n";
        let j = extraire(c);
        assert!(j.contains("MAJUSCULES"));
        assert!(j.contains("PORT"));
    }

    #[test]
    fn un_mot_sans_double_tiret_bas_n_est_pas_un_jeton() {
        let j = extraire("UPPER=value\n_x_=1\nA__B=2\n");
        assert!(j.is_empty(), "{j:?}");
    }

    #[test]
    fn les_jetons_composes_sont_reconnus_entiers() {
        let j = extraire("PATH=__PATH_WITH_NODEJS__\ndir=__INSTALL_DIR__\n");
        assert!(j.contains("PATH_WITH_NODEJS"));
        assert!(j.contains("INSTALL_DIR"));
        assert!(
            !j.contains("PATH"),
            "le jeton long ne doit pas etre coupe : {j:?}"
        );
    }

    #[test]
    fn deux_jetons_colles_sont_separes() {
        // `https://__DOMAIN____PATH__` est une ecriture courante.
        let j = extraire("url=https://__DOMAIN____PATH__\n");
        assert!(j.contains("DOMAIN"), "{j:?}");
        assert!(j.contains("PATH"), "{j:?}");
    }
}
