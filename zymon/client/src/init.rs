//! `zymon-client init`: detect what this host can report and write the config.
//! A section is `true` only if its data source exists, is readable and (exec-backed) the binary is on the fixed
//! search path. Source missing = not applicable (key left out); present but unusable = `false`. No network traffic.

use crate::config::{Config, Docker, Identity, Info};
use crate::sections;
use crate::sys::{common, exec};
use std::fs::File;
use std::io;

pub enum Probe {
    On,
    Off(&'static str),
    Absent(&'static str),
}

fn src(path: &str) -> Probe {
    match File::open(path) {
        Ok(_) => Probe::On,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Probe::Absent("source missing"),
        Err(_) => Probe::Off("source not readable"),
    }
}

fn bin(cmd: &str) -> Probe {
    if exec::which(cmd).is_some() {
        Probe::On
    } else {
        Probe::Absent("command not found")
    }
}

fn either(a: Probe, b: Probe) -> Probe {
    if matches!(a, Probe::On) { a } else { b }
}

fn docker_socket(path: &str) -> Probe {
    if std::fs::metadata(path).is_err() {
        Probe::Absent("no docker socket")
    } else if common::can_rw(path) {
        Probe::On
    } else {
        Probe::Off("socket not readable")
    }
}

/// Linux thermal zones or hwmon sensors; present-but-unreadable means disabled, none at all means not applicable.
fn cputemp_probe() -> Probe {
    use crate::sections::cputemp;
    if !cputemp::read_zones().is_empty() {
        return Probe::On;
    }
    let exists = |root: &str, prefix: &str| {
        std::fs::read_dir(root)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .any(|e| e.file_name().to_string_lossy().starts_with(prefix))
            })
            .unwrap_or(false)
    };
    if exists("/sys/class/thermal", "thermal_zone") || exists("/sys/class/hwmon", "hwmon") {
        Probe::Off("sensors not readable")
    } else {
        Probe::Absent("no temperature sensor")
    }
}

/// smartmontools installed and runnable: as root directly, otherwise through a NOPASSWD sudoers rule.
fn smart_probe() -> Probe {
    let Some(path) = exec::which("smartctl") else {
        return Probe::Absent("smartctl not found");
    };
    if common::euid() == 0 {
        return Probe::On;
    }
    let ok = exec::run(
        "sudo",
        &["-n", &path.to_string_lossy(), "--version"],
        std::time::Duration::from_secs(5),
    )
    .map(|o| o.success)
    .unwrap_or(false);
    if ok {
        Probe::On
    } else {
        Probe::Off("needs root or a sudoers rule for smartctl")
    }
}

pub fn probe(name: &str, docker_sock: &str) -> Probe {
    let mac = cfg!(target_os = "macos");
    match name {
        "date" | "uname" | "nproc" | "clientversion" | "clock" | "cpuinfo" => Probe::On,
        "osversion" => {
            let any = [
                "/etc/os-release",
                "/usr/lib/os-release",
                "/etc/debian_version",
                "/etc/redhat-release",
            ]
            .iter()
            .any(|p| File::open(p).is_ok());
            if any {
                Probe::On
            } else {
                Probe::Absent("no release file")
            }
        }
        "uptime" => {
            if mac {
                Probe::On
            } else {
                src("/proc/uptime")
            }
        }
        "who" => bin("who"),
        "df" | "inode" | "mount" => {
            if mac {
                bin("mount")
            } else {
                src("/proc/self/mounts")
            }
        }
        "free" => src("/proc/meminfo"),
        "meminfo" => Probe::On,
        "ifconfig" => bin("ifconfig"),
        "route" => {
            if mac {
                bin("netstat")
            } else {
                src("/proc/net/route")
            }
        }
        "netstat" => {
            if mac {
                bin("netstat")
            } else {
                either(bin("netstat"), src("/proc/net/snmp"))
            }
        }
        "ports" => {
            if mac {
                bin("netstat")
            } else {
                src("/proc/net/tcp")
            }
        }
        "ifstat" => {
            if mac {
                bin("netstat")
            } else {
                src("/proc/net/dev")
            }
        }
        "mdstat" => src("/proc/mdstat"),
        "smart" => smart_probe(),
        "ps" => bin("ps"),
        "top" => {
            if mac {
                Probe::On
            } else {
                src("/proc/stat")
            }
        }
        "vmstat" => {
            if mac {
                Probe::Absent("not on macOS")
            } else {
                src("/proc/vmstat")
            }
        }
        "cputemp" => cputemp_probe(),
        "crontab" => {
            let any = File::open("/etc/crontab").is_ok()
                || std::path::Path::new("/etc/cron.d").is_dir()
                || exec::which("crontab").is_some();
            if any {
                Probe::On
            } else {
                Probe::Absent("no crontab source")
            }
        }
        "docker" => docker_socket(docker_sock),
        _ => Probe::Absent("unknown"),
    }
}

pub struct Args {
    pub servers: Vec<String>,
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub class: Option<String>,
}

/// Detect everything and return the config plus one summary line per section.
pub fn detect(args: &Args) -> io::Result<(Config, Vec<String>)> {
    let uname = common::uname()?;
    let mut cfg = Config::default();
    let os = args
        .os
        .clone()
        .unwrap_or_else(|| zymon_core::host::os_tag(&uname.sysname));
    cfg.identity = Identity {
        hostname: args.hostname.clone().unwrap_or_else(common::hostname),
        class: args.class.clone().unwrap_or_else(|| os.clone()),
        os,
    };
    cfg.servers.list = args.servers.clone();
    cfg.docker = Docker::default();

    let mut summary = Vec::new();
    for name in sections::order() {
        match probe(name, &cfg.docker.socket) {
            Probe::On => {
                cfg.sections.insert((*name).to_string(), true);
                summary.push(format!("{name}: ok"));
            }
            Probe::Off(why) => {
                cfg.sections.insert((*name).to_string(), false);
                summary.push(format!("{name}: skipped ({why})"));
            }
            Probe::Absent(why) => summary.push(format!("{name}: not applicable ({why})")),
        }
    }

    let kv = sections::cpuinfo::gather();
    let get = |k: &str| kv.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
    cfg.info = Info {
        model: get("model"),
        cpu_model: get("cpu_model"),
        cpu_arch: get("cpu_arch"),
        cores: get("cores").and_then(|c| c.parse().ok()),
        hardware: get("hardware"),
        revision: get("revision"),
        arch: get("arch"),
    };
    Ok((cfg, summary))
}
