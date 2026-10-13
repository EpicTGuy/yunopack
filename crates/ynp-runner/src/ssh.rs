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
        let fin: Vec<&str> = source
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // La ligne decisive est souvent loin de la fin : « FATAL ERROR:
        // Reached heap limit » de Node precede une pile de quarante lignes,
        // et les douze dernieres ne disaient que « Aborted ».
        const DECISIFS: &[&str] = &[
            "FATAL ERROR",
            "Traceback",
            "ERR!",
            "error:",
            "Error:",
            "command not found",
            "commande introuvable",
            "No such file",
            "Permission denied",
            "Killed",
        ];
        let mut decisives: Vec<&str> = Vec::new();
        for l in source.lines() {
            let t = l.trim();
            if decisives.len() < 4
                && DECISIFS.iter().any(|d| t.contains(d))
                && !fin.contains(&l)
                && !decisives.contains(&t)
            {
                decisives.push(t);
            }
        }
        decisives
            .into_iter()
            .chain(fin)
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

    /// Execute une operation longue (installation, mise a jour, sauvegarde)
    /// detachee de la session SSH : elle tourne sur l'hote jusqu'au bout,
    /// et l'on relit son code de sortie a intervalles reguliers. Une coupure
    /// de la connexion pendant une installation de vingt minutes laissait
    /// sinon le test sans verdict — et l'hote au milieu d'une operation.
    pub async fn executer_detache(&self, commande: &str) -> Result<Sortie, SshError> {
        let id = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let base = format!("/tmp/yunopack/operations/{id}");
        // L'operation tourne dans un cgroup borne a 80 % de la memoire, avec
        // sans echange : une construction trop gourmande y est tuee aussitot, au lieu
        // de faire echanger l'hote jusqu'a ne plus repondre — le Dell de test,
        // 3,7 Go, est reste injoignable plus d'une heure pendant celle de cap.
        // Avec de l'echange permis et celui de l'hote plein, le noyau recuperait
        // de la memoire en boucle une heure durant sans rien tuer.
        // L'operation ecrit elle-meme son code de sortie, depuis son cgroup :
        // ecrit par une enveloppe restee dans la session ssh, il s'est perdu
        // une fois (la sauvegarde de flaresolverr avait reussi, le runner l'a
        // attendue un jour entier).
        let operation =
            format!("bash {base}.sh > {base}.out 2> {base}.err\necho $? > {base}.code\n");
        let enveloppe = format!(
            "lim=$(awk '/MemTotal/ {{print int($2 * 0.8)}}' /proc/meminfo)\n\
             if [ \"$(id -u)\" = 0 ] && command -v systemd-run > /dev/null 2>&1; then\n\
             exec systemd-run --scope --quiet -p MemoryMax=${{lim}}K -p MemorySwapMax=0 bash {base}.op\n\
             fi\n\
             exec bash {base}.op\n"
        );
        // Les scripts passent en base64 : aucune citation a echapper.
        let lancement = format!(
            "mkdir -p /tmp/yunopack/operations && echo {} | base64 -d > {base}.sh && \
             echo {} | base64 -d > {base}.op && \
             echo {} | base64 -d > {base}.run && \
             setsid nohup bash {base}.run > /dev/null 2>&1 < /dev/null &",
            base64(commande.as_bytes()),
            base64(operation.as_bytes()),
            base64(enveloppe.as_bytes())
        );
        // Le lancement lui-meme peut tomber sur une coupure : on reessaie,
        // sans relancer une operation qui aurait demarre.
        let mut lance = false;
        for _ in 0..30 {
            if self.executer(&format!("test -e {base}.sh")).await?.ok() {
                lance = true;
                break;
            }
            if self.executer(&lancement).await?.code != 255 {
                lance = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
        if !lance {
            return Err(SshError::Injoignable {
                hote: self.alias.clone(),
            });
        }
        // Au plus trois heures ; une heure sans reponse de l'hote met fin a
        // l'attente.
        let debut = std::time::Instant::now();
        let mut derniere_reponse = std::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let s = self
                .executer(&format!("cat {base}.code 2>/dev/null"))
                .await?;
            if s.code == 255 {
                if derniere_reponse.elapsed() > std::time::Duration::from_secs(3600) {
                    return Err(SshError::Injoignable {
                        hote: self.alias.clone(),
                    });
                }
                continue;
            }
            derniere_reponse = std::time::Instant::now();
            if let Ok(code) = s.stdout.trim().parse::<i32>() {
                let lire = |f: &str| format!("cat {base}.{f} 2>/dev/null");
                let stdout = self.executer(&lire("out")).await?.stdout;
                let stderr = self.executer(&lire("err")).await?.stdout;
                let _ = self.executer(&format!("rm -f {base}.*")).await;
                return Ok(Sortie {
                    code,
                    stdout,
                    stderr,
                });
            }
            if debut.elapsed() > std::time::Duration::from_secs(3 * 3600) {
                return Ok(Sortie {
                    code: 124,
                    stdout: String::new(),
                    stderr: "operation interrompue apres trois heures".into(),
                });
            }
        }
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

/// Encodage base64 standard, sans dependance de plus.
fn base64(octets: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(octets.len().div_ceil(3) * 4);
    for bloc in octets.chunks(3) {
        let n = (u32::from(bloc[0]) << 16)
            | (u32::from(*bloc.get(1).unwrap_or(&0)) << 8)
            | u32::from(*bloc.get(2).unwrap_or(&0));
        for (i, decalage) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= bloc.len() {
                out.push(A[((n >> decalage) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn le_diagnostic_remonte_la_ligne_decisive() {
        // Cas de bytestash : la pile de Node masquait la cause.
        let mut sortie = String::from("WARNING FATAL ERROR: Reached heap limit\n");
        for i in 0..30 {
            sortie.push_str(&format!("WARNING {i}: v8::internal\n"));
        }
        sortie.push_str("WARNING Aborted\n");
        let s = Sortie {
            code: 1,
            stdout: String::new(),
            stderr: sortie,
        };
        assert!(s
            .diagnostic()
            .starts_with("WARNING FATAL ERROR: Reached heap limit"));
        assert!(s.diagnostic().ends_with("WARNING Aborted"));
    }

    use super::*;

    #[test]
    fn l_encodage_base64_suit_la_norme() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(
            base64(b"yunohost app install"),
            "eXVub2hvc3QgYXBwIGluc3RhbGw="
        );
    }

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
