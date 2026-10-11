#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_zymon-client")
}

static N: AtomicUsize = AtomicUsize::new(0);

/// A fresh directory per call; tests run in parallel inside one process.
pub fn tmpdir(name: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("zymon-test-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("spawn zymon-client")
}

pub fn init(cfg: &Path, extra: &[&str]) {
    let mut a = vec!["init", "--config", cfg.to_str().unwrap()];
    a.extend_from_slice(extra);
    let o = run(&a);
    assert!(
        o.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Sections in wire order, as the legacy parser expects them (kept here independently of the code under test).
pub fn expected_order() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &[
            "date",
            "uname",
            "uptime",
            "who",
            "df",
            "inode",
            "mount",
            "meminfo",
            "ifconfig",
            "route",
            "netstat",
            "ifstat",
            "ports",
            "smart",
            "ps",
            "nproc",
            "top",
            "cpuinfo",
            "crontab",
            "docker",
            "clientversion",
            "clock",
        ]
    } else {
        &[
            "date",
            "uname",
            "osversion",
            "uptime",
            "who",
            "df",
            "inode",
            "mount",
            "free",
            "ifconfig",
            "route",
            "netstat",
            "ports",
            "ifstat",
            "mdstat",
            "smart",
            "ps",
            "nproc",
            "top",
            "vmstat",
            "cpuinfo",
            "cputemp",
            "gpu",
            "crontab",
            "docker",
            "clientversion",
            "clock",
        ]
    }
}

pub struct Msg {
    pub first: String,
    pub sections: Vec<(String, String)>,
}

impl Msg {
    pub fn parse(raw: &[u8]) -> Msg {
        let text = String::from_utf8_lossy(raw).into_owned();
        let mut it = text.lines();
        let first = it.next().unwrap_or("").to_string();
        let mut sections: Vec<(String, String)> = Vec::new();
        for line in it {
            if line.starts_with('[') && line.ends_with(']') {
                sections.push((line[1..line.len() - 1].to_string(), String::new()));
            } else if let Some(s) = sections.last_mut() {
                s.1.push_str(line);
                s.1.push('\n');
            }
        }
        Msg { first, sections }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.as_str())
    }

    pub fn names(&self) -> Vec<&str> {
        self.sections.iter().map(|(n, _)| n.as_str()).collect()
    }
}
