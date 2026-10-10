//! /proc and /sys readers. Compiled on every host so the parsers can be unit-tested anywhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::state::Snap;
use std::collections::HashSet;
use std::io;
use std::time::Instant;

pub fn read(path: &str) -> io::Result<String> {
    // Not guaranteed UTF-8 (hostnames in mounts, etc.); lossy is fine for every /proc consumer here.
    std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Value of a `Key:   123 kB` line in /proc/meminfo, in kB.
pub fn meminfo_kb(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(key)
            && let Some(rest) = rest.strip_prefix(':')
        {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEnt {
    pub source: String,
    pub mountpoint: String,
    pub fstype: String,
    pub opts: String,
}

/// /proc/self/mounts escapes space, tab, newline and backslash as octal.
fn unescape_octal(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') as u32 * 64
                + (b[i + 2] - b'0') as u32 * 8
                + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mounts(text: &str) -> Vec<MountEnt> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some(MountEnt {
                source: unescape_octal(f.next()?),
                mountpoint: unescape_octal(f.next()?),
                fstype: f.next()?.to_string(),
                opts: f.next().unwrap_or("rw").to_string(),
            })
        })
        .collect()
}

/// Filesystem types the legacy script excludes: every `nodev` type except rootfs.
pub fn nodev_types(proc_filesystems: &str) -> HashSet<String> {
    proc_filesystems
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            if f.next()? == "nodev" {
                let t = f.next()?;
                (t != "rootfs").then(|| t.to_string())
            } else {
                None
            }
        })
        .collect()
}

pub fn parse_snap(stat: &str, vmstat: &str, at: Instant) -> Snap {
    let mut s = Snap {
        at,
        cpu: [0; 8],
        ctxt: 0,
        intr: 0,
        procs_running: 0,
        procs_blocked: 0,
        pgpgin: 0,
        pgpgout: 0,
        pswpin: 0,
        pswpout: 0,
    };
    for line in stat.lines() {
        let mut f = line.split_whitespace();
        match f.next() {
            Some("cpu") => {
                for (i, v) in f.take(8).enumerate() {
                    s.cpu[i] = v.parse().unwrap_or(0);
                }
            }
            Some("ctxt") => s.ctxt = f.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            Some("intr") => s.intr = f.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            Some("procs_running") => {
                s.procs_running = f.next().and_then(|v| v.parse().ok()).unwrap_or(0)
            }
            Some("procs_blocked") => {
                s.procs_blocked = f.next().and_then(|v| v.parse().ok()).unwrap_or(0)
            }
            _ => {}
        }
    }
    for line in vmstat.lines() {
        let mut f = line.split_whitespace();
        let (Some(k), Some(v)) = (f.next(), f.next()) else {
            continue;
        };
        let v: u64 = v.parse().unwrap_or(0);
        match k {
            "pgpgin" => s.pgpgin = v,
            "pgpgout" => s.pgpgout = v,
            "pswpin" => s.pswpin = v,
            "pswpout" => s.pswpout = v,
            _ => {}
        }
    }
    s
}

pub fn snap() -> io::Result<Snap> {
    let stat = read("/proc/stat")?;
    let vm = read("/proc/vmstat").unwrap_or_default();
    Ok(parse_snap(&stat, &vm, Instant::now()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_lookup() {
        let t = "MemTotal:       949440 kB\nMemFree:         93408 kB\nSwapTotal:      102396 kB\n";
        assert_eq!(meminfo_kb(t, "MemTotal"), Some(949440));
        assert_eq!(meminfo_kb(t, "Mem"), None);
        assert_eq!(meminfo_kb(t, "Buffers"), None);
    }

    #[test]
    fn mounts_parse_and_unescape() {
        let t = "/dev/root / ext4 rw,noatime 0 0\nproc /proc proc rw 0 0\n/dev/sdb1 /mnt/my\\040disk vfat rw 0 0\n";
        let m = parse_mounts(t);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].fstype, "ext4");
        assert_eq!(m[2].mountpoint, "/mnt/my disk");
    }

    #[test]
    fn nodev_excludes_rootfs() {
        let t = "nodev\tsysfs\nnodev\trootfs\nnodev\ttmpfs\n\text4\n\tiso9660\n";
        let s = nodev_types(t);
        assert!(s.contains("sysfs") && s.contains("tmpfs"));
        assert!(!s.contains("rootfs") && !s.contains("ext4"));
    }

    #[test]
    fn snap_parse() {
        let stat = "cpu  100 5 50 800 20 1 2 3 0 0\ncpu0 1 1 1 1 1 1 1 1 0 0\nintr 9000 1 2\nctxt 777\nprocs_running 3\nprocs_blocked 1\n";
        let vm = "pgpgin 10\npgpgout 20\npswpin 1\npswpout 2\nfoo 9\n";
        let s = parse_snap(stat, vm, Instant::now());
        assert_eq!(s.cpu, [100, 5, 50, 800, 20, 1, 2, 3]);
        assert_eq!(
            (s.ctxt, s.intr, s.procs_running, s.procs_blocked),
            (777, 9000, 3, 1)
        );
        assert_eq!((s.pgpgin, s.pgpgout, s.pswpin, s.pswpout), (10, 20, 1, 2));
    }
}
