//! Comment le code d'une application choisit son port d'ecoute.
//!
//! Faute de variable d'environnement, les conventions (`PORT`, `HTTP_PORT`...)
//! etaient proposees au hasard. Le code dit souvent mieux : une option de
//! ligne de commande (`--port`, `--listen`), ou un port ecrit en dur. RSSPub
//! ecoute sur `SocketAddr::from(([0, 0, 0, 0], 3000))`, sans aucun reglage :
//! aucune variable ne pouvait fonctionner.

use regex::Regex;
use std::sync::OnceLock;

/// Un indice releve dans le code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Indice {
    /// Option de ligne de commande, avec ses tirets : `--port`, `--listen`.
    Option(String),
    /// Port litteral passe a un appel d'ecoute.
    PortEnDur(u16),
    /// Option dont la valeur par defaut est une adresse `hote:port` :
    /// `flag.String("bind", ":8923", ...)` chez anubis. Elle attend donc une
    /// adresse complete, ce que le nom seul ne disait pas.
    OptionAdresse(String),
}

fn options_adresse() -> &'static [Regex] {
    static M: OnceLock<Vec<Regex>> = OnceLock::new();
    M.get_or_init(|| {
        [
            // Go, paquet flag ou un FlagSet du projet : `flag.String("bind",
            // ":8923", ...)`, `fs.StringVar(&a, "listen", ":8080", ...)`. La
            // valeur par defaut, une adresse, garantit le sens.
            r#"\b\w+\.String(?:Var)?\((?:&?[\w.]+\s*,\s*)?"([\w-]+)"\s*,\s*"([^"]*)""#,
            // Go, cobra et pflag : `StringVarP(&a, "bind", "b", ":8080", ...)`.
            r#"\.String(?:Var)?P\((?:&[\w.]+\s*,\s*)?"([\w-]+)"\s*,\s*"\w?"\s*,\s*"([^"]*)""#,
        ]
        .iter()
        .map(|m| Regex::new(m).expect("motif d'option invalide"))
        .collect()
    })
}

/// `:8923`, `0.0.0.0:8080`, `127.0.0.1:3000` : une adresse d'ecoute.
fn est_une_adresse(v: &str) -> bool {
    v.rsplit_once(':').is_some_and(|(h, p)| {
        !p.is_empty()
            && p.len() <= 5
            && p.chars().all(|c| c.is_ascii_digit())
            && matches!(
                h,
                "" | "0.0.0.0" | "127.0.0.1" | "localhost" | "[::]" | "::"
            )
    })
}

fn options() -> &'static [Regex] {
    static M: OnceLock<Vec<Regex>> = OnceLock::new();
    M.get_or_init(|| {
        [
            // Rust, clap : `#[arg(long = "port")]`.
            r#"long\s*=\s*"(port|listen|bind|addr|address|http-port|listen-addr|listen-address|http-addr)""#,
            // Rust, clap : `#[arg(short, long)] port: u16`.
            r#"#\[(?:arg|clap)\([^\]]*\blong\b[^\]]*\)\]\s*(?:pub\s+)?(port|listen|bind|addr|address)\s*:"#,
            // Go, paquet flag : `flag.Int("port", ...)`, `flag.StringVar(&a, "listen", ...)`.
            r#"flag\.(?:String|Int|Uint|Int64)(?:Var)?\((?:&?[\w.]+\s*,\s*)?"(port|listen|addr|bind|http|listen-addr|http-addr|address)""#,
            // Python, argparse et click.
            r#"add_argument\([^)]*["']--(port|listen|bind|listen-address|http-port)["']"#,
            r#"@click\.option\([^)]*["']--(port|listen|bind)["']"#,
            // JavaScript, commander et yargs.
            r#"\.option\(\s*["'][^"']*--(port|listen|bind)\b"#,
            // JavaScript, optimist et yargs : `.options('p', { alias: 'port' })`
            // (federated-wiki).
            r#"\balias\s*:\s*["'](port|listen|bind)["']"#,
            // JavaScript, yargs sans tirets : `.option('port', {...})`.
            r#"\.option\(\s*["'](port|listen|bind)["']\s*,"#,
            // Ruby, OptionParser : `opts.on('-p', '--port PORT')`.
            r#"\.on\([^)]*["']--(port|listen|bind)\b"#,
            // Go, cobra et pflag : `Flags().StringVarP(&a, "port", "p", ...)`.
            // Pas l'option d'un sous-ensemble de flag (`settingCmd.IntVar(&port,
            // "port", ...)` chez 3x-ui, qui regle le port enregistre et que la
            // commande principale refuse : « flag provided but not defined »).
            r#"(?:Flags\(\)|\bpflag)\.(?:String|Int|Uint|Uint16)(?:Var)?P?\((?:&[\w.]+\s*,\s*)?"(port|listen|addr|bind|address|listen-addr|http-addr)""#,
            // Go, urfave/cli : `&cli.IntFlag{Name: "port"}`.
            r#"cli\.(?:String|Int|Uint)Flag\{\s*Name:\s*"(port|listen|addr|bind|address)""#,
        ]
        .iter()
        .map(|m| Regex::new(m).expect("motif d'option invalide"))
        .collect()
    })
}

fn ports_en_dur() -> &'static [Regex] {
    static M: OnceLock<Vec<Regex>> = OnceLock::new();
    M.get_or_init(|| {
        [
            // Rust : `SocketAddr::from(([0, 0, 0, 0], 3000))`.
            r"\(\s*\[\s*(?:0\s*,\s*0\s*,\s*0\s*,\s*0|127\s*,\s*0\s*,\s*0\s*,\s*1)\s*\]\s*,\s*(\d{2,5})\s*\)",
            // Go, Rust, Python : `ListenAndServe(":8080"`, `bind("0.0.0.0:8080")`.
            r#"(?:ListenAndServe|ListenAndServeTLS|bind|Listen|listen)\(\s*(?:[\w:]+\s*,\s*)?["'](?:[\w.\[\]:-]*)?:(\d{2,5})["']"#,
            // Node : `app.listen(3000)`.
            r"\.listen\(\s*(\d{2,5})\s*[,)]",
            // Python : `app.run(host=..., port=5000)`, `uvicorn.run(app, port=8000)`.
            r"\.run\([^)]*\bport\s*=\s*(\d{2,5})\b",
        ]
        .iter()
        .map(|m| Regex::new(m).expect("motif de port invalide"))
        .collect()
    })
}

/// Les indices d'ecoute d'un fichier source, dans l'ordre, sans doublon.
pub fn indices(contenu: &str) -> Vec<Indice> {
    let mut out = Vec::new();
    // Prefiltre : la plupart des fichiers n'ecoutent rien.
    if !(contenu.contains("port")
        || contenu.contains("listen")
        || contenu.contains("Listen")
        || contenu.contains("bind")
        || contenu.contains("addr"))
    {
        return out;
    }
    // Une option de sous-commande commander (`program.command("daemon")
    // .option("-p, --port <port>")` chez cross-seed) : la commande principale
    // la refuse (« unknown option '--port' »). L'indice porte la
    // sous-commande : `daemon --port`. Elle se cree aussi par une fonction
    // du projet (`createCommandWithSharedOptions("daemon", …)`).
    static SOUS: OnceLock<Regex> = OnceLock::new();
    let sous = SOUS.get_or_init(|| {
        Regex::new(r#"\b\w*[cC]ommand\w*\(\s*["']([\w-]+)"#).expect("motif valide")
    });
    for m in options() {
        for c in m.captures_iter(contenu) {
            let debut = c.get(0).map_or(0, |x| x.start());
            let js = c[0].starts_with(".option");
            let commande = js
                .then(|| sous.captures_iter(&contenu[..debut]).last())
                .flatten()
                .filter(|s| {
                    // La meme chaine d'appels : pas d'instruction terminee entre.
                    let fin = s.get(0).map_or(0, |x| x.end());
                    !contenu[fin..debut].contains(';')
                })
                .map(|s| format!("{} ", &s[1]))
                .unwrap_or_default();
            let i = Indice::Option(format!("{commande}--{}", &c[1]));
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    // zli (goatcounter) : la valeur par defaut d'abord, `f.String(":8080",
    // "listen")`.
    static INVERSE: OnceLock<Regex> = OnceLock::new();
    let inverse = INVERSE.get_or_init(|| {
        Regex::new(r#"\b\w+\.String\(\s*"([^"]*)"\s*,\s*"([\w-]+)"\s*\)"#).expect("motif valide")
    });
    for c in inverse.captures_iter(contenu) {
        if est_une_adresse(&c[1]) {
            let i = Indice::OptionAdresse(format!("--{}", &c[2]));
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    for m in options_adresse() {
        for c in m.captures_iter(contenu) {
            if est_une_adresse(&c[2]) {
                let i = Indice::OptionAdresse(format!("--{}", &c[1]));
                if !out.contains(&i) {
                    out.push(i);
                }
            }
        }
    }
    for m in ports_en_dur() {
        for c in m.captures_iter(contenu) {
            if let Ok(p) = c[1].parse::<u16>() {
                let i = Indice::PortEnDur(p);
                if p >= 80 && !out.contains(&i) {
                    out.push(i);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_option_d_adresse_d_un_flagset_ou_de_zli_est_reconnue() {
        let go = r#"listen = f.String(":8080", "listen")"#;
        assert!(indices(go).contains(&Indice::OptionAdresse("--listen".into())));
        let go = r#"fs.StringVar(&cfg.Addr, "addr", "127.0.0.1:3000", "address")"#;
        assert!(indices(go).contains(&Indice::OptionAdresse("--addr".into())));
        let go = r#"dbConn = f.String("16,4", "dbconn")"#;
        assert!(
            indices(go).is_empty()
                || !indices(go).contains(&Indice::OptionAdresse("--dbconn".into()))
        );
    }

    #[test]
    fn l_option_d_une_sous_commande_porte_la_sous_commande() {
        let js = r#"program.command("daemon").description("run").option("-p, --port <port>", "port").action(f);"#;
        assert!(indices(js).contains(&Indice::Option("daemon --port".into())));
        let js = r#"createCommandWithSharedOptions("daemon", "Start")
	.option("-p, --port <port>", "port", (n) => parseInt(n))"#;
        assert!(indices(js).contains(&Indice::Option("daemon --port".into())));
        let js = r#"program.command("search").action(f);
program.option("-p, --port <port>", "port");"#;
        assert!(indices(js).contains(&Indice::Option("--port".into())));
    }

    #[test]
    fn une_option_a_adresse_par_defaut_attend_une_adresse() {
        // Cas d'anubis.
        let go = r#"bind = flag.String("bind", ":8923", "network address to bind HTTP to")"#;
        assert!(indices(go).contains(&Indice::OptionAdresse("--bind".into())));
        // Une valeur qui n'est pas une adresse : rien de plus.
        let go = r#"name = flag.String("bind", "eth0", "interface")"#;
        assert!(!indices(go).contains(&Indice::OptionAdresse("--bind".into())));
    }

    #[test]
    fn un_port_ecrit_en_dur_se_releve() {
        // Cas reel de RSSPub.
        let rs = "let addr = SocketAddr::from(([0, 0, 0, 0], 3000));";
        assert_eq!(indices(rs), vec![Indice::PortEnDur(3000)]);
        assert_eq!(
            indices(r#"log.Fatal(http.ListenAndServe(":8080", nil))"#),
            vec![Indice::PortEnDur(8080)]
        );
        assert_eq!(
            indices("app.listen(4000, () => {})"),
            vec![Indice::PortEnDur(4000)]
        );
        assert_eq!(
            indices("app.run(host='0.0.0.0', port=5000)"),
            vec![Indice::PortEnDur(5000)]
        );
    }

    #[test]
    fn une_option_de_port_se_releve_dans_chaque_langage() {
        let cas = [
            (r#"#[arg(long = "listen")] listen: String"#, "--listen"),
            (
                "#[arg(short, long, default_value_t = 8080)]\n    port: u16,",
                "--port",
            ),
            (r#"port := flag.Int("port", 8080, "port")"#, "--port"),
            (
                r#"flag.StringVar(&addr, "listen", ":8080", "")"#,
                "--listen",
            ),
            (
                r#"parser.add_argument("--port", type=int, default=8000)"#,
                "--port",
            ),
            (r#"@click.option("--bind", default="0.0.0.0")"#, "--bind"),
            (r#"program.option('-p, --port <n>', 'port')"#, "--port"),
            (r#"yargs.option('port', { default: 3000 })"#, "--port"),
            (
                r#"opts.on('-p', '--port [PORT]', 'Bind port') do |p|"#,
                "--port",
            ),
            (
                r#"cmd.Flags().StringVarP(&listen, "listen", "l", ":8080", "")"#,
                "--listen",
            ),
            (
                r#"rootCmd.PersistentFlags().IntVar(&port, "port", 8080, "")"#,
                "--port",
            ),
            (r#"&cli.IntFlag{Name: "port", Value: 3000}"#, "--port"),
        ];
        for (code, attendu) in cas {
            let options: Vec<Indice> = indices(code)
                .into_iter()
                .filter(|i| matches!(i, Indice::Option(_)))
                .collect();
            assert_eq!(options, vec![Indice::Option(attendu.into())], "{code}");
        }
    }

    #[test]
    fn ce_qui_n_ecoute_pas_ne_donne_rien() {
        assert!(indices("let report = support.export();").is_empty());
        assert!(indices(r#"client.connect("db:5432")"#).is_empty());
    }
}
