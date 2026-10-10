use std::path::PathBuf;
use std::process::ExitCode;
use zymon_client::config::{self, Config};
use zymon_client::{init, schedule, sections, service, state, sys};
use zymon_core::log::{self, Level};

const USAGE: &str = "usage:
  zymon-client init  [--config PATH] [--server HOST[:PORT]]... [--hostname NAME] [--os TAG] [--class CLS] [--force]
  zymon-client once  [--config PATH] [--dry-run] [--sections a,b,c] [-v]
  zymon-client run   [--config PATH] [--dry-run] [-v]
  zymon-client restart                     restart the installed service (systemd / launchd; uses sudo when not root)
  zymon-client version

config: --config, else $ZYMON_CLIENT_CONFIG, else the platform default (/etc/zymon/client.toml, macOS: /usr/local/etc/zymon/client.toml)
";

enum Cmd {
    Init {
        config: Option<PathBuf>,
        args: init::Args,
        force: bool,
    },
    Once {
        config: Option<PathBuf>,
        dry_run: bool,
        sections: Option<Vec<String>>,
        verbose: bool,
    },
    Run {
        config: Option<PathBuf>,
        dry_run: bool,
        verbose: bool,
    },
    Restart,
    Version,
    Help,
}

fn parse_args() -> Result<Cmd, lexopt::Error> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_env();
    let Some(Value(sub)) = p.next()? else {
        return Ok(Cmd::Help);
    };
    let sub = sub.string()?;
    let mut config = None;
    let mut dry_run = false;
    let mut verbose = false;
    let mut force = false;
    let mut sections = None;
    let mut args = init::Args {
        servers: Vec::new(),
        hostname: None,
        os: None,
        class: None,
    };
    match sub.as_str() {
        "version" | "--version" | "-V" => return Ok(Cmd::Version),
        "help" | "--help" | "-h" => return Ok(Cmd::Help),
        "restart" => return Ok(Cmd::Restart),
        "init" | "once" | "run" => {}
        other => return Err(lexopt::Error::UnexpectedArgument(other.into())),
    }
    while let Some(arg) = p.next()? {
        match (sub.as_str(), arg) {
            (_, Long("config")) => config = Some(PathBuf::from(p.value()?)),
            ("init", Long("server")) => args.servers.push(p.value()?.string()?),
            ("init", Long("hostname")) => args.hostname = Some(p.value()?.string()?),
            ("init", Long("os")) => args.os = Some(p.value()?.string()?),
            ("init", Long("class")) => args.class = Some(p.value()?.string()?),
            ("init", Long("force")) => force = true,
            ("once" | "run", Long("dry-run")) => dry_run = true,
            ("once" | "run", Short('v') | Long("verbose")) => verbose = true,
            ("once", Long("sections")) => {
                sections = Some(
                    p.value()?
                        .string()?
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                )
            }
            (_, Short('h') | Long("help")) => return Ok(Cmd::Help),
            (_, other) => return Err(other.unexpected()),
        }
    }
    Ok(match sub.as_str() {
        "init" => Cmd::Init {
            config,
            args,
            force,
        },
        "once" => Cmd::Once {
            config,
            dry_run,
            sections,
            verbose,
        },
        _ => Cmd::Run {
            config,
            dry_run,
            verbose,
        },
    })
}

fn load(config: Option<PathBuf>) -> Result<(Config, PathBuf), ExitCode> {
    let path = config::resolve_path(config);
    match Config::load(&path) {
        Ok(c) => Ok((c, path)),
        Err(e) => {
            zymon_core::error!("{e}");
            if matches!(e, config::ConfigError::Io(..)) {
                zymon_core::error!("run `zymon-client init` first");
            }
            Err(ExitCode::from(2))
        }
    }
}

fn do_init(config: Option<PathBuf>, args: init::Args, force: bool) -> ExitCode {
    let path = config::resolve_path(config);
    if path.exists() && !force {
        zymon_core::error!("{} exists; use --force to overwrite", path.display());
        return ExitCode::from(2);
    }
    let (cfg, summary) = match init::detect(&args) {
        Ok(x) => x,
        Err(e) => {
            zymon_core::error!("detection failed: {e}");
            return ExitCode::from(2);
        }
    };
    let (now, _) = sys::common::now();
    let text = cfg.render(&sys::common::strftime("%Y-%m-%d %H:%M:%S %Z", now, false));
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        zymon_core::error!("{}: {e}", dir.display());
        return ExitCode::from(2);
    }
    if let Err(e) = std::fs::write(&path, text) {
        zymon_core::error!("{}: {e}", path.display());
        return ExitCode::from(2);
    }
    for l in summary {
        println!("{l}");
    }
    if cfg.servers.list.is_empty() {
        eprintln!(
            "note: no --server given; add one to [servers].list in {}",
            path.display()
        );
    }
    println!("wrote {}", path.display());
    ExitCode::SUCCESS
}

fn do_once(config: Option<PathBuf>, dry_run: bool, sections: Option<Vec<String>>) -> ExitCode {
    let (cfg, _) = match load(config) {
        Ok(x) => x,
        Err(c) => return c,
    };
    if let Some(list) = &sections
        && let Some(bad) = list
            .iter()
            .find(|s| !sections::order().contains(&s.as_str()))
    {
        zymon_core::error!(
            "unknown section '{bad}' (valid: {})",
            sections::order().join(",")
        );
        return ExitCode::from(2);
    }
    if !dry_run && cfg.servers.list.is_empty() {
        zymon_core::error!("no servers configured ([servers].list is empty)");
        return ExitCode::from(2);
    }
    let dir = if dry_run {
        std::env::temp_dir()
    } else {
        config::state_dir(&cfg)
    };
    let mut state = state::State::default();
    let opts = schedule::CycleOpts {
        dry_run,
        only: sections.as_deref(),
        state_dir: &dir,
    };
    if schedule::cycle(&cfg, &mut state, &opts) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn main() -> ExitCode {
    let cmd = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("zymon-client: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match cmd {
        Cmd::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Cmd::Restart => {
            eprintln!("zymon-client: {}", service::restart());
            ExitCode::from(1)
        }
        Cmd::Version => {
            println!("zymon-client {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Cmd::Init {
            config,
            args,
            force,
        } => do_init(config, args, force),
        Cmd::Once {
            config,
            dry_run,
            sections,
            verbose,
        } => {
            if verbose {
                log::set_level(Level::Debug);
            }
            do_once(config, dry_run, sections)
        }
        Cmd::Run {
            config,
            dry_run,
            verbose,
        } => {
            if verbose {
                log::set_level(Level::Debug);
            }
            let (cfg, path) = match load(config) {
                Ok(x) => x,
                Err(c) => return c,
            };
            if !dry_run && cfg.servers.list.is_empty() {
                zymon_core::error!("no servers configured ([servers].list is empty)");
                return ExitCode::from(2);
            }
            ExitCode::from(schedule::run(cfg, path, dry_run) as u8)
        }
    }
}
