//! A Rust port of the *checks* xymond performs on a client message (xymond_client.c, client/linux.c,
//! client/darwin.c, rrd/do_vmstat.c), applied to `once --dry-run` on the host running the tests.

mod common;
use common::*;

fn host_message() -> Msg {
    let dir = tmpdir("legacy");
    let cfg = dir.join("c.toml");
    init(&cfg, &["--server", "127.0.0.1"]);
    let o = run(&["once", "--config", cfg.to_str().unwrap(), "--dry-run"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    Msg::parse(&o.stdout)
}

/// selectcolumn(): case-insensitive position of a header word.
fn col(header: &str, word: &str) -> Option<usize> {
    header
        .split_whitespace()
        .position(|h| h.eq_ignore_ascii_case(word))
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

#[test]
fn header_and_order() {
    let m = host_message();
    let mut f = m.first.split(' ');
    assert_eq!(f.next(), Some("client"));
    let (machine, os) = f.next().unwrap().rsplit_once('.').expect("MACHINE.OSTAG");
    assert!(
        !machine.is_empty() && !machine.contains('.'),
        "machine must be commafied: {machine}"
    );
    assert_eq!(
        os,
        if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        }
    );
    assert_eq!(f.next(), Some(os), "class defaults to the OS tag");

    let order = expected_order();
    let mut last = None;
    for n in m.names() {
        let i = order
            .iter()
            .position(|o| *o == n)
            .unwrap_or_else(|| panic!("unexpected section {n}"));
        assert!(last.is_none_or(|l| i > l), "section {n} out of order");
        last = Some(i);
    }
    let n = m.names();
    assert_eq!(&n[n.len() - 2..], ["clientversion", "clock"]);
}

#[test]
fn uptime_parses_like_the_server() {
    let m = host_message();
    let u = m.get("uptime").unwrap();
    let p = u.find(" up ").expect("' up '");
    assert!(p < u.len());
    let key = if u.contains("load averages: ") {
        "load averages: "
    } else {
        "load average: "
    };
    let l = &u[u.find(key).expect("load average") + key.len()..];
    let nums: Vec<f32> = l
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .take(3)
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(nums.len(), 3);
    // uptime parse: "N days" or "H:MM" or "N min"
    let rest = &u[p + 3..];
    assert!(
        rest.contains(':') || rest.contains(" min") || rest.contains(" day"),
        "{u}"
    );
}

#[test]
fn disk_and_inode_columns() {
    let m = host_message();
    let (free, capa, mnt, ifree, iuse) = if cfg!(target_os = "macos") {
        ("Avail", "Capacity", "Mounted", "ifree", "%iused")
    } else {
        ("Available", "Capacity", "Mounted", "IFree", "IUse%")
    };
    let df = m.get("df").unwrap();
    let h = first_line(df);
    let (fc, cc, mc) = (
        col(h, free).unwrap(),
        col(h, capa).unwrap(),
        col(h, mnt).unwrap(),
    );
    assert!(df.lines().count() > 1, "no filesystem rows");
    for l in df.lines().skip(1) {
        let c: Vec<&str> = l.split_whitespace().collect();
        assert!(c.len() > mc, "short row: {l}");
        assert!(c[fc].parse::<u64>().is_ok(), "avail: {l}");
        assert!(
            c[cc].trim_end_matches('%').parse::<u64>().is_ok(),
            "capacity: {l}"
        );
        assert!(c[mc].contains('/'), "do_disk.c needs a '/' in the row: {l}");
        // do_disk.c reads fixed columns: 2=used 4=capacity 5=mount
        assert!(c[2].parse::<u64>().is_ok() && c[4].ends_with('%') && c[5] == c[mc]);
    }
    let inode = m.get("inode").unwrap();
    let h = first_line(inode);
    let (fc, cc, mc) = (
        col(h, ifree).unwrap(),
        col(h, iuse).unwrap(),
        col(h, mnt).unwrap(),
    );
    for l in inode.lines().skip(1) {
        let c: Vec<&str> = l.split_whitespace().collect();
        assert!(c.len() > mc, "short inode row: {l}");
        assert!(c[fc].parse::<u64>().is_ok());
        assert!(c[cc] == "-" || c[cc].trim_end_matches('%').parse::<u64>().is_ok());
    }
}

#[test]
fn procs_header() {
    let m = host_message();
    let ps = m.get("ps").unwrap();
    let h = first_line(ps);
    let want = if cfg!(target_os = "macos") {
        "COMMAND"
    } else {
        "CMD"
    };
    assert!(h.contains(want), "{h}");
    let cmdofs = h.find(want).unwrap();
    assert!(ps.lines().count() > 1);
    assert!(ps.lines().nth(1).unwrap().len() > cmdofs);
}

#[test]
fn ports_columns() {
    let m = host_message();
    let ports = m.get("ports").unwrap();
    for l in ports
        .lines()
        .filter(|l| l.starts_with("tcp") || l.starts_with("udp"))
    {
        let c: Vec<&str> = l.split_whitespace().collect();
        assert!(c.len() >= 5, "{l}");
        assert!(
            c[3].contains(':') || c[3].contains('.'),
            "local column: {l}"
        );
        assert!(
            c[4].contains(':') || c[4].contains('.') || c[4] == "*.*",
            "remote column: {l}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_memory_free_and_vmstat() {
    let m = host_message();
    let free = m.get("free").unwrap();
    // linux.c: new-style detection and sscanf formats
    assert!(free.contains("available\n"));
    let p = &free[free.find("\nMem:").map(|i| i + 1).expect("Mem:")..];
    let n: Vec<i64> = p
        .split_whitespace()
        .skip(1)
        .take(6)
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(n.len(), 6);
    assert!(n[0] > 0);
    let sp = &free[free.find("\nSwap:").map(|i| i + 1).expect("Swap:")..];
    assert_eq!(
        sp.split_whitespace()
            .skip(1)
            .take(3)
            .filter(|s| s.parse::<i64>().is_ok())
            .count(),
        3
    );
    // first cycle: no [vmstat] (needs a previous snapshot)
    assert!(m.get("vmstat").is_none());
    assert!(m.get("nproc").unwrap().trim().parse::<u32>().unwrap() >= 1);
}

#[cfg(target_os = "macos")]
#[test]
fn darwin_meminfo() {
    let m = host_message();
    let mem = m.get("meminfo").unwrap();
    // darwin.c: "page size of %lu bytes" and four Pages lines
    let p = mem.find("page size of").unwrap();
    let ps: u64 = mem[p + 13..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(ps >= 4096);
    for k in [
        "Pages free:",
        "Pages active:",
        "Pages inactive:",
        "Pages wired down:",
    ] {
        let l = mem
            .lines()
            .find(|l| l.starts_with(k))
            .unwrap_or_else(|| panic!("{k}"));
        assert!(
            l[k.len()..]
                .trim()
                .trim_end_matches('.')
                .parse::<u64>()
                .is_ok(),
            "{l}"
        );
    }
    assert!(m.get("free").is_none() && m.get("vmstat").is_none());
}

#[test]
fn clock_has_three_lines() {
    let m = host_message();
    let c: Vec<&str> = m.get("clock").unwrap().lines().collect();
    assert_eq!(c.len(), 3);
    assert!(
        c[0].starts_with("epoch: ")
            && c[0][7..]
                .split_once('.')
                .is_some_and(|(s, us)| s.parse::<u64>().is_ok() && us.len() == 6)
    );
    assert!(c[1].starts_with("local: ") && c[2].starts_with("UTC: "));
    assert!(m.get("clientversion").unwrap().starts_with("zymon-client/"));
}

#[test]
fn cpuinfo_never_leaks_serial() {
    let m = host_message();
    let c = m.get("cpuinfo").unwrap();
    assert!(!c.to_ascii_lowercase().contains("serial"));
    for l in c.lines() {
        assert!(l.split_once(": ").is_some(), "{l}");
    }
}
