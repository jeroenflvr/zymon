use super::{Ctx, Res};
use std::io::Write;
use zymon_core::proto::SectionWriter;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    // net-tools always prints one decimal, even for plain bytes: "(0.0 B)"
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn flag_names(flags: u32) -> String {
    let mut v = Vec::new();
    for (bit, n) in [
        (0x1, "UP"),
        (0x2, "BROADCAST"),
        (0x8, "LOOPBACK"),
        (0x10, "POINTOPOINT"),
        (0x40, "RUNNING"),
        (0x1000, "MULTICAST"),
    ] {
        if flags & bit != 0 {
            v.push(n);
        }
    }
    v.join(",")
}

/// /proc/net/dev in the modern `ifconfig` layout, which the legacy RRD handler reads
/// (`^name:`, then `RX packets N  bytes M ...` / `TX packets ...`). `iface_info(name)` gives (flags, mtu).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn render(proc_dev: &str, iface_info: impl Fn(&str) -> (u32, u32)) -> String {
    let mut s = String::new();
    for l in proc_dev.lines().skip(2) {
        let Some((name, rest)) = l.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let f: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        if f.len() < 10 {
            continue;
        }
        let (flags, mtu) = iface_info(name);
        s.push_str(&format!(
            "{name}: flags={flags}<{}>  mtu {mtu}\n",
            flag_names(flags)
        ));
        s.push_str(&format!(
            "        RX packets {}  bytes {} ({})\n",
            f[1],
            f[0],
            human(f[0])
        ));
        s.push_str(&format!(
            "        TX packets {}  bytes {} ({})\n\n",
            f[9],
            f[8],
            human(f[8])
        ));
    }
    s
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn sys_info(name: &str) -> (u32, u32) {
    let rd =
        |f: &str| std::fs::read_to_string(format!("/sys/class/net/{name}/{f}")).unwrap_or_default();
    let mut flags =
        u32::from_str_radix(rd("flags").trim().trim_start_matches("0x"), 16).unwrap_or(0);
    // ifconfig's RUNNING is the operational state; sysfs flags do not carry it.
    if flags & 0x1 != 0 && matches!(rd("operstate").trim(), "up" | "unknown") {
        flags |= 0x40;
    }
    let mtu = rd("mtu").trim().parse().unwrap_or(0);
    (flags, mtu)
}

pub fn ifstat(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if cfg!(target_os = "macos") {
        // `netstat -ibn` minus the lo and <Link lines, as the darwin script does.
        let out = ctx.exec("netstat", &["-ibn"])?;
        for l in out.split(|&b| b == b'\n') {
            let t = String::from_utf8_lossy(l);
            if l.is_empty() || t.starts_with("lo") || t.contains("<Link") {
                continue;
            }
            w.line(l);
        }
        Ok(())
    } else {
        let t = crate::sys::linux::read("/proc/net/dev")?;
        w.write_all(render(&t, sys_info).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: &str = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo:  1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0\n  eth0: 1829192    1500    0    0    0     0          0         0  1827320    1400    0    0    0     0       0          0\n";

    #[test]
    fn matches_legacy_regexes() {
        let t = render(DEV, |n| if n == "lo" { (73, 65536) } else { (4163, 1500) });
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "lo: flags=73<UP,LOOPBACK,RUNNING>  mtu 65536");
        assert_eq!(
            lines[4],
            "eth0: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500"
        );
        // ^\s+RX packets\s+[0-9]+\s+bytes\s+([0-9]+)<space>
        assert_eq!(lines[5], "        RX packets 1500  bytes 1829192 (1.7 MiB)");
        assert_eq!(lines[6], "        TX packets 1400  bytes 1827320 (1.7 MiB)");
        assert!(
            lines[0]
                .split(':')
                .next()
                .unwrap()
                .chars()
                .all(|c| c.is_ascii_alphanumeric())
        );
    }
}
