//! Config struct, TOML load/render, defaults and paths.

use crate::sections;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zymon_core::net::NetOpts;

#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, String),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            ConfigError::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            ConfigError::Invalid(e) => write!(f, "invalid config: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Identity {
    pub hostname: String,
    pub os: String,
    pub class: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Servers {
    pub list: Vec<String>,
    pub connect_timeout_s: u64,
    pub read_timeout_s: u64,
    pub retries: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub interval_s: u64,
    pub jitter_s: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Docker {
    pub socket: String,
    pub must_run: Vec<String>,
    pub status_column: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Cputemp {
    pub warn_c: f64,
    pub panic_c: f64,
    pub status_column: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Exec {
    pub timeout_s: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Info {
    pub model: Option<String>,
    pub cpu_model: Option<String>,
    pub cpu_arch: Option<String>,
    pub cores: Option<u32>,
    pub hardware: Option<String>,
    pub revision: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub identity: Identity,
    pub servers: Servers,
    pub schedule: Schedule,
    /// Present key = applicable on this host; `false` = detected but disabled.
    pub sections: BTreeMap<String, bool>,
    pub docker: Docker,
    pub exec: Exec,
    pub cputemp: Cputemp,
    pub info: Info,
    /// Optional override of where the server reply is kept (default: see `state_dir`).
    pub state_dir: Option<String>,
}

impl Default for Servers {
    fn default() -> Self {
        Servers {
            list: Vec::new(),
            connect_timeout_s: 10,
            read_timeout_s: 15,
            retries: 3,
        }
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            interval_s: 300,
            jitter_s: 30,
        }
    }
}

impl Default for Docker {
    fn default() -> Self {
        Docker {
            socket: "/var/run/docker.sock".into(),
            must_run: Vec::new(),
            status_column: "docker".into(),
        }
    }
}

impl Default for Cputemp {
    fn default() -> Self {
        // The Pi starts throttling at 80 C; warn well before that.
        Cputemp {
            warn_c: 70.0,
            panic_c: 80.0,
            status_column: "cputemp".into(),
        }
    }
}

impl Default for Exec {
    fn default() -> Self {
        Exec { timeout_s: 20 }
    }
}

/// TOML basic string with proper escaping.
fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn qlist(v: &[String]) -> String {
    let items: Vec<String> = v.iter().map(|s| q(s)).collect();
    format!("[{}]", items.join(", "))
}

impl Config {
    pub fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
        let cfg: Config =
            toml::from_str(text).map_err(|e| ConfigError::Parse(path.into(), e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.into(), e))?;
        Config::parse(&text, path)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: &str| Err(ConfigError::Invalid(m.into()));
        if self.identity.hostname.trim().is_empty() {
            return bad("identity.hostname is empty");
        }
        if self.identity.os.trim().is_empty() {
            return bad("identity.os is empty");
        }
        if self.schedule.interval_s == 0 {
            return bad("schedule.interval_s must be > 0");
        }
        if self.cputemp.warn_c >= self.cputemp.panic_c {
            return bad("cputemp.warn_c must be below cputemp.panic_c");
        }
        if self
            .docker
            .must_run
            .iter()
            .any(|n| n.contains(char::is_whitespace))
        {
            return bad("docker.must_run names must not contain spaces");
        }
        Ok(())
    }

    pub fn class(&self) -> &str {
        zymon_core::host::class_or_default(&self.identity.class, &self.identity.os)
    }

    pub fn machine(&self) -> String {
        zymon_core::host::machine(&self.identity.hostname)
    }

    pub fn net_opts(&self) -> NetOpts {
        NetOpts {
            connect_timeout: Duration::from_secs(self.servers.connect_timeout_s.max(1)),
            read_timeout: Duration::from_secs(self.servers.read_timeout_s.max(1)),
            retries: self.servers.retries.max(1),
            backoff: Duration::from_secs(1),
        }
    }

    pub fn exec_timeout(&self) -> Duration {
        Duration::from_secs(self.exec.timeout_s.max(1))
    }

    /// `[clientversion]` and `[clock]` are part of every message unless explicitly switched off.
    pub fn section_enabled(&self, name: &str) -> bool {
        let always = matches!(name, "clientversion" | "clock");
        self.sections.get(name).copied().unwrap_or(always)
    }

    /// The commented file written by `init`. Section keys come out in wire order.
    pub fn render(&self, date: &str) -> String {
        let mut s = String::with_capacity(2048);
        s.push_str(&format!(
            "# generated by zymon-client init on {date}; edit freely\n"
        ));
        s.push_str("[identity]\n");
        s.push_str(&format!(
            "hostname = {}   # dots are commafied on the wire\n",
            q(&self.identity.hostname)
        ));
        s.push_str(&format!(
            "os = {}   # OSTAG sent to the server (selects the server-side parser)\n",
            q(&self.identity.os)
        ));
        s.push_str(&format!("class = {}\n\n", q(&self.identity.class)));
        s.push_str("[servers]\n");
        s.push_str(&format!(
            "list = {}   # host or host:port; every server gets every message\n",
            qlist(&self.servers.list)
        ));
        s.push_str(&format!(
            "connect_timeout_s = {}\n",
            self.servers.connect_timeout_s
        ));
        s.push_str(&format!(
            "read_timeout_s = {}\n",
            self.servers.read_timeout_s
        ));
        s.push_str(&format!("retries = {}\n\n", self.servers.retries));
        s.push_str("[schedule]\n");
        s.push_str(&format!("interval_s = {}\n", self.schedule.interval_s));
        s.push_str(&format!(
            "jitter_s = {}   # random 0..jitter added once at startup\n\n",
            self.schedule.jitter_s
        ));
        s.push_str("[exec]\n");
        s.push_str(&format!(
            "timeout_s = {}   # hard limit per external command\n\n",
            self.exec.timeout_s
        ));
        s.push_str(
            "[sections]   # false = detected but disabled; absent = not applicable on this host\n",
        );
        for name in sections::order() {
            if let Some(v) = self.sections.get(*name) {
                s.push_str(&format!("{name} = {v}\n"));
            }
        }
        s.push_str("\n[docker]\n");
        s.push_str(&format!("socket = {}\n", q(&self.docker.socket)));
        s.push_str(&format!("must_run = {}   # container names; a missing/stopped one turns the status column red\n", qlist(&self.docker.must_run)));
        s.push_str(&format!(
            "status_column = {}\n\n",
            q(&self.docker.status_column)
        ));
        s.push_str("[cputemp]   # client-side `cputemp` status column (Linux), from the [cputemp] section's sensors\n");
        s.push_str(&format!(
            "warn_c = {:?}   # yellow at or above\n",
            self.cputemp.warn_c
        ));
        s.push_str(&format!(
            "panic_c = {:?}   # red at or above\n",
            self.cputemp.panic_c
        ));
        s.push_str(&format!(
            "status_column = {}\n\n",
            q(&self.cputemp.status_column)
        ));
        s.push_str("[info]   # snapshot taken at init for reference only; [cpuinfo] re-reads /proc/cpuinfo each cycle\n");
        let i = &self.info;
        for (k, v) in [
            ("model", &i.model),
            ("cpu_model", &i.cpu_model),
            ("cpu_arch", &i.cpu_arch),
        ] {
            if let Some(v) = v {
                s.push_str(&format!("{k} = {}\n", q(v)));
            }
        }
        if let Some(c) = i.cores {
            s.push_str(&format!("cores = {c}\n"));
        }
        for (k, v) in [
            ("hardware", &i.hardware),
            ("revision", &i.revision),
            ("arch", &i.arch),
        ] {
            if let Some(v) = v {
                s.push_str(&format!("{k} = {}\n", q(v)));
            }
        }
        s
    }
}

pub const ENV_CONFIG: &str = "ZYMON_CLIENT_CONFIG";

pub fn default_config_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        "/usr/local/etc/zymon/client.toml".into()
    } else {
        "/etc/zymon/client.toml".into()
    }
}

/// `--config` beats `ZYMON_CLIENT_CONFIG` beats the platform default.
pub fn resolve_path(arg: Option<PathBuf>) -> PathBuf {
    arg.or_else(|| {
        std::env::var_os(ENV_CONFIG)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
    .unwrap_or_else(default_config_path)
}

/// `state_dir` from the config, else `/var/lib/zymon`, else `$XDG_STATE_HOME/zymon`,
/// else `/tmp/zymon-<uid>`; created on demand.
pub fn state_dir(cfg: &Config) -> PathBuf {
    let mut cands: Vec<PathBuf> = cfg.state_dir.iter().map(PathBuf::from).collect();
    cands.push(PathBuf::from("/var/lib/zymon"));
    if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        cands.push(PathBuf::from(x).join("zymon"));
    }
    for c in &cands {
        if std::fs::create_dir_all(c).is_ok() && crate::sys::common::can_rw(&c.to_string_lossy()) {
            return c.clone();
        }
    }
    let fallback = PathBuf::from(format!("/tmp/zymon-{}", crate::sys::common::euid()));
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        let mut c = Config {
            identity: Identity {
                hostname: "pi4.lan".into(),
                os: "linux".into(),
                class: "linux".into(),
            },
            ..Config::default()
        };
        c.servers.list = vec!["192.168.1.10".into(), "b:1985".into()];
        for n in sections::order() {
            c.sections.insert((*n).to_string(), true);
        }
        c.sections.insert("docker".into(), false);
        c.docker.must_run = vec!["web".into()];
        c.info = Info {
            model: Some("Raspberry Pi \"Zero\" W".into()),
            cpu_model: Some("ARMv6".into()),
            cpu_arch: Some("7".into()),
            cores: Some(1),
            hardware: Some("BCM2835".into()),
            revision: Some("9000c1".into()),
            arch: Some("armv6l".into()),
        };
        c
    }

    #[test]
    fn render_round_trip() {
        let c = sample();
        let text = c.render("today");
        let back = Config::parse(&text, Path::new("x")).unwrap();
        assert_eq!(c, back);
        assert!(text.starts_with("# generated by zymon-client init on today; edit freely\n"));
    }

    #[test]
    fn defaults_fill_in() {
        let c = Config::parse("[identity]\nhostname='a'\nos='linux'\n", Path::new("x")).unwrap();
        assert_eq!(c.schedule.interval_s, 300);
        assert_eq!(c.servers.read_timeout_s, 15);
        assert_eq!(c.class(), "linux");
        assert!(!c.section_enabled("df"));
        assert!(c.section_enabled("clock") && c.section_enabled("clientversion"));
    }

    #[test]
    fn rejects_bad_config() {
        assert!(Config::parse("[identity]\nhostname=''\nos='linux'\n", Path::new("x")).is_err());
        assert!(Config::parse("[[", Path::new("x")).is_err());
        assert!(
            Config::parse(
                "[identity]\nhostname='a'\nos='l'\n[schedule]\ninterval_s=0\n",
                Path::new("x")
            )
            .is_err()
        );
    }
}
