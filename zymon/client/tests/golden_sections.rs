//! Golden tests: captured legacy-script output per hardware class (tests/golden/<os>-<arch>/) against what
//! the zymon renderers produce from the same captured raw inputs. Line-structured comparison; volatile
//! numbers (counters, clocks) are masked or only checked for presence. Capture new classes with
//! tests/golden/capture.sh on the real hardware.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use zymon_client::sections::{
    df, free, ifstat, inode, meminfo, mount, netstat, ports, route, uptime,
};
use zymon_client::sys::common::StatVfs;
use zymon_client::sys::linux::{self, MountEnt};

fn golden_dirs(prefix: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut v: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .collect();
    v.sort();
    v
}

fn read(dir: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(rel)).ok()
}

/// Whitespace-normalised lines.
fn norm(s: &str) -> Vec<String> {
    s.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect()
}

/// Digits -> '#', plural endings folded: for lines whose numbers are volatile.
fn mask(s: &str) -> String {
    let mut m = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !m.ends_with('#') {
                m.push('#');
            }
        } else {
            m.push(c);
        }
    }
    let m = m
        .replace(" users,", " user,")
        .replace(" days,", " day,")
        .replace(" mins,", " min,");
    m.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn linux_classes() {
    let dirs = golden_dirs("linux-");
    assert!(!dirs.is_empty(), "no linux golden fixtures");
    for d in dirs {
        linux_class(&d);
    }
}

fn linux_class(d: &Path) {
    let name = d.file_name().unwrap().to_string_lossy().into_owned();
    // mount: byte-for-byte modulo whitespace
    if let (Some(m), Some(legacy)) = (read(d, "proc/mounts"), read(d, "legacy/mount.txt")) {
        assert_eq!(
            norm(&mount::render_linux(&linux::parse_mounts(&m))),
            norm(&legacy),
            "{name}: mount"
        );
    }
    // free: header exact, row labels and column counts, fixed totals
    if let (Some(mi), Some(legacy)) = (read(d, "proc/meminfo"), read(d, "legacy/free.txt")) {
        let ours = free::render(&mi);
        let (o, l) = (norm(&ours), norm(&legacy));
        assert_eq!(o[0], l[0], "{name}: free header");
        for (a, b) in o[1..].iter().zip(&l[1..]) {
            let (a, b): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
            assert_eq!((a[0], a.len()), (b[0], b.len()), "{name}: free row");
            assert_eq!(a[1], b[1], "{name}: {} total", a[0]);
        }
    }
    // route
    if let (Some(r), Some(legacy)) = (read(d, "proc/net/route"), read(d, "legacy/route.txt")) {
        assert_eq!(norm(&route::render(&r)), norm(&legacy), "{name}: route");
    }
    // ports: same (proto, local, remote, state) set as netstat -antuW
    if let Some(legacy) = read(d, "legacy/ports.txt") {
        let mut s = String::new();
        for (proto, f) in [
            ("tcp", "tcp"),
            ("tcp6", "tcp6"),
            ("udp", "udp"),
            ("udp6", "udp6"),
        ] {
            if let Some(t) = read(d, &format!("proc/net/{f}")) {
                ports::render_proto(&mut s, proto, &t);
            }
        }
        let key = |l: &str| {
            let c: Vec<&str> = l.split_whitespace().collect();
            (
                c[0].to_string(),
                c[3].to_string(),
                c[4].to_string(),
                c.get(5).unwrap_or(&"").to_string(),
            )
        };
        let ours: BTreeSet<_> = s.lines().map(key).collect();
        let theirs: BTreeSet<_> = legacy
            .lines()
            .filter(|l| l.starts_with("tcp") || l.starts_with("udp"))
            .map(key)
            .collect();
        assert_eq!(ours, theirs, "{name}: ports");
        assert!(
            !theirs.is_empty(),
            "{name}: fixture should hold listening sockets"
        );
    }
    // ifstat: the interface header line is identical when fed the same flags/mtu; counters are masked
    if let (Some(dev), Some(legacy)) = (read(d, "proc/net/dev"), read(d, "legacy/ifstat.txt")) {
        let info = |n: &str| {
            let l = legacy
                .lines()
                .find(|l| l.starts_with(&format!("{n}: ")))
                .unwrap();
            let flags: u32 = l
                .split("flags=")
                .nth(1)
                .unwrap()
                .split('<')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let mtu: u32 = l.split("mtu ").nth(1).unwrap().trim().parse().unwrap();
            (flags, mtu)
        };
        let ours = ifstat::render(&dev, info);
        let ours_iface: BTreeSet<String> = ours
            .lines()
            .filter(|l| !l.starts_with(' ') && !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        let legacy_iface: BTreeSet<String> = legacy
            .lines()
            .filter(|l| !l.starts_with(' ') && !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        assert_eq!(ours_iface, legacy_iface, "{name}: ifstat headers");
        let rx = |s: &str| -> BTreeSet<String> {
            s.lines()
                .filter(|l| {
                    l.trim_start().starts_with("RX packets")
                        || l.trim_start().starts_with("TX packets")
                })
                .map(mask)
                .collect()
        };
        assert_eq!(rx(&ours), rx(&legacy), "{name}: ifstat counters layout");
    }
    // netstat: the wording the legacy RRD handler greps for must exist in the real `netstat -s`, too
    if let (Some(snmp), Some(legacy)) = (read(d, "proc/net/snmp"), read(d, "legacy/netstat.txt")) {
        let ours = netstat::render_snmp(&snmp);
        assert!(legacy.contains("\nTcp:") && ours.contains("\nTcp:"));
        for marker in [
            "packets received",
            "packets sent",
            "packet receive errors",
            "active connection openings",
            "passive connection openings",
            "failed connection attempts",
            "connection resets received",
            "connections established",
            "segments sent out",
            "segments received",
            "segments retransmitted",
        ] {
            assert!(
                legacy.contains(marker),
                "{name}: legacy netstat lacks '{marker}'"
            );
            assert!(
                ours.contains(marker),
                "{name}: zymon netstat lacks '{marker}'"
            );
        }
    }
    // df / inode: feed the legacy numbers back in; the layout and the capacity rounding must agree
    if let Some(legacy) = read(d, "legacy/df.txt") {
        let fs: Vec<(MountEnt, StatVfs)> = legacy
            .lines()
            .skip(1)
            .map(|l| {
                let c: Vec<&str> = l.split_whitespace().collect();
                let n = |i: usize| c[i].parse::<u64>().unwrap();
                (
                    MountEnt {
                        source: c[0].into(),
                        mountpoint: c[5].into(),
                        fstype: "x".into(),
                        opts: String::new(),
                    },
                    StatVfs {
                        frsize: 1024,
                        blocks: n(1),
                        bfree: n(1) - n(2),
                        bavail: n(3),
                        files: 0,
                        ffree: 0,
                    },
                )
            })
            .collect();
        assert_eq!(
            norm(&df::render_for(&fs, false)),
            norm(&legacy),
            "{name}: df"
        );
    }
    if let Some(legacy) = read(d, "legacy/inode.txt") {
        let fs: Vec<(MountEnt, StatVfs)> = legacy
            .lines()
            .skip(1)
            .map(|l| {
                let c: Vec<&str> = l.split_whitespace().collect();
                let n = |i: usize| c[i].parse::<u64>().unwrap();
                (
                    MountEnt {
                        source: c[0].into(),
                        mountpoint: c[5].into(),
                        fstype: "x".into(),
                        opts: String::new(),
                    },
                    StatVfs {
                        frsize: 1024,
                        blocks: 1,
                        bfree: 1,
                        bavail: 1,
                        files: n(1),
                        ffree: n(3),
                    },
                )
            })
            .collect();
        assert_eq!(
            norm(&inode::render_for(&fs, false)),
            norm(&legacy),
            "{name}: inode"
        );
    }
    // uptime layout
    if let (Some(up), Some(la), Some(legacy)) = (
        read(d, "proc/uptime"),
        read(d, "proc/loadavg"),
        read(d, "legacy/uptime.txt"),
    ) {
        let secs = up
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<f64>()
            .unwrap() as u64;
        let l: Vec<f64> = la
            .split_whitespace()
            .take(3)
            .map(|v| v.parse().unwrap())
            .collect();
        let ours = uptime::format_uptime("00:00:00", secs, 0, [l[0], l[1], l[2]], false);
        assert_eq!(mask(&ours), mask(legacy.trim_end()), "{name}: uptime");
    }
}

#[test]
fn darwin_classes() {
    let dirs = golden_dirs("darwin-");
    assert!(!dirs.is_empty(), "no darwin golden fixtures");
    for d in dirs {
        let name = d.file_name().unwrap().to_string_lossy().into_owned();
        // meminfo: byte-identical to vm_stat, label set and order included
        if let Some(legacy) = read(&d, "legacy/meminfo.txt") {
            let page: u64 = legacy
                .split("page size of ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let rows: Vec<(String, u64)> = legacy
                .lines()
                .skip(1)
                .map(|l| {
                    let (k, v) = l.rsplit_once(':').unwrap();
                    (
                        k.to_string(),
                        v.trim().trim_end_matches('.').parse().unwrap(),
                    )
                })
                .collect();
            let labels: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(labels, meminfo::LABELS, "{name}: vm_stat labels");
            let r: Vec<(&str, u64)> = rows.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            assert_eq!(meminfo::render(page, &r), legacy, "{name}: meminfo bytes");
        }
        if let Some(legacy) = read(&d, "legacy/mount.txt") {
            let ents = mount::parse_mount_cmd(&legacy);
            assert_eq!(ents.len(), legacy.lines().count(), "{name}: mount lines");
            for (e, l) in ents.iter().zip(legacy.lines()) {
                let opts = if e.opts.is_empty() {
                    e.fstype.clone()
                } else {
                    format!("{}, {}", e.fstype, e.opts)
                };
                assert_eq!(format!("{} on {} ({})", e.source, e.mountpoint, opts), l);
            }
        }
        if let Some(legacy) = read(&d, "legacy/uptime.txt") {
            let ours = uptime::format_uptime("00:00", 3 * 86400 + 3600, 0, [1.0, 2.0, 3.0], true);
            assert_eq!(mask(&ours), mask(legacy.trim_end()), "{name}: uptime");
        }
    }
}
