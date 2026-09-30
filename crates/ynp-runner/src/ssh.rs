//! Execution de commandes sur l'hote de test.
//!
//! On sort vers les binaires `ssh` et `rsync` plutot que d'embarquer une
//! bibliotheque : les campagnes durent plusieurs minutes, et le multiplexage
//! d'OpenSSH gere le maintien de session mieux que ce que nous ecririons.
//! L'hote est designe par un alias de `~/.ssh/config`, jamais par des
//! identifiants — aucun secret ne transite par la configuration de l'outil.

use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("impossible de lancer {programme} : {source}")]
    Lancement {
        programme: String,
        source: std::io::Error,
    },
    #[error("l'hote « {hote} » ne repond pas")]
    Injoignable { hote: String },
}

/// Ce qu'une commande distante a produit.
#[derive(Debug, Clone)]
pub struct Sortie {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Sortie {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// Sortie standard debarrassee de ses espaces, cas le plus courant.
    pub fn texte(&self) -> &str {
        self.stdout.trim()
    }

    /// Les lignes vraiment utiles pour un rapport d'echec.
    /// Toute la sortie, pour y chercher un identifiant de journal.
    pub fn diagnostic_complet(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }

    pub fn diagnostic(&self) -> String {
        let source = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        source
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub struct Hote {
    alias: String,
}

impl Hote {
    pub fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
        }
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Verifie que l'hote repond avant d'engager quoi que ce soit.
    pub async fn joignable(&self) -> Result<(), SshError> {
        let s = self.executer("true").await?;
        if s.ok() {
            Ok(())
        } else {
            Err(SshError::Injoignable {
                hote: self.alias.clone(),
            })
        }
    }

    pub async fn executer(&self, commande: &str) -> Result<Sortie, SshError> {
        let sortie = Command::new("ssh")
            .args(["-o", "ConnectTimeout=10", "-o", "BatchMode=yes"])
            .arg(&self.alias)
            .arg(commande)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|source| SshError::Lancement {
                programme: "ssh".into(),
                source,
            })?;

        Ok(Sortie {
            code: sortie.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&sortie.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&sortie.stderr).into_owned(),
        })
    }

    /// Vrai si la commande reussit, sans se soucier de sa sortie.
    pub async fn reussit(&self, commande: &str) -> bool {
        self.executer(commande)
            .await
            .map(|s| s.ok())
            .unwrap_or(false)
    }

    pub async fn envoyer(
        &self,
        local: &std::path::Path,
        distant: &str,
    ) -> Result<Sortie, SshError> {
        // Le repertoire parent doit exister : rsync ne cree pas d'arborescence.
        if let Some((parent, _)) = distant.rsplit_once('/') {
            let _ = self.executer(&format!("mkdir -p {parent}")).await;
        }

        let source = format!("{}/", local.display());
        let sortie = Command::new("rsync")
            .args(["-a", "--delete"])
            .arg(&source)
            .arg(format!("{}:{distant}/", self.alias))
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|source| SshError::Lancement {
                programme: "rsync".into(),
                source,
            })?;

        Ok(Sortie {
            code: sortie.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&sortie.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&sortie.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_diagnostic_retient_la_fin_du_journal() {
        // C'est la ou se trouve l'erreur, pas dans les mille lignes de debug.
        let s = Sortie {
            code: 1,
            stdout: (1..=30)
                .map(|i| format!("ligne {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            stderr: String::new(),
        };
        let d = s.diagnostic();
        assert!(d.contains("ligne 30"));
        assert!(!d.contains("ligne 1\n"));
        assert_eq!(d.lines().count(), 12);
    }

    #[test]
    fn stderr_prime_sur_stdout_quand_il_dit_quelque_chose() {
        let s = Sortie {
            code: 1,
            stdout: "bruit".into(),
            stderr: "la vraie erreur".into(),
        };
        assert_eq!(s.diagnostic(), "la vraie erreur");
    }
}
